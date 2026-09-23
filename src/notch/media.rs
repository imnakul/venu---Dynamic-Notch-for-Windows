//! Reads the system's "Now Playing" session — whichever app currently owns
//! Windows' System Media Transport Controls — on a dedicated background
//! thread, and publishes a snapshot the painter can read without blocking.
//!
//! The WinRT calls here block on `.get()` rather than awaiting. That is safe
//! only because this thread does nothing else: no message pump, no UI, so a
//! stalled wait starves nothing but itself.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use windows::core::Interface;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Storage::Streams::{DataReader, IInputStream, IRandomAccessStreamReference};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

const POLL_INTERVAL: Duration = Duration::from_millis(900);
/// Album art is a thumbnail, never anything close to this; a stream this big
/// is a sign to bail rather than block on reading it.
const MAX_ART_BYTES: u64 = 8 * 1024 * 1024;

/// What the painter reads each frame. Cheap to clone — art bytes live behind
/// an `Arc` so an unchanged frame costs nothing to hand back.
#[derive(Clone, Default)]
pub struct NowPlaying {
    pub has_session: bool,
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub art: Option<Arc<[u8]>>,
    /// Bumped whenever `art` changes, so the painter's decode cache knows to
    /// re-decode without hashing or comparing the byte slice itself.
    pub art_generation: u64,
}

enum Command {
    PlayPause,
    Next,
    Previous,
}

/// Owns the background poller. Dropping this stops the thread — the notch
/// feature can be toggled off and on freely without leaking one.
pub struct MediaWatcher {
    snapshot: Arc<RwLock<NowPlaying>>,
    /// Bumped only when a poll actually found something different. The notch
    /// reads it every tick to decide whether the frame on screen is still
    /// current, so a poll that finds the same track playing must not look
    /// like a change.
    revision: Arc<AtomicU64>,
    tx: Sender<Command>,
    running: Arc<AtomicBool>,
}

impl MediaWatcher {
    pub fn spawn() -> Self {
        let snapshot = Arc::new(RwLock::new(NowPlaying::default()));
        let revision = Arc::new(AtomicU64::new(0));
        let running = Arc::new(AtomicBool::new(true));
        let (tx, rx) = mpsc::channel();

        let worker_snapshot = snapshot.clone();
        let worker_revision = revision.clone();
        let worker_running = running.clone();
        std::thread::spawn(move || run(worker_snapshot, worker_revision, worker_running, rx));

        Self {
            snapshot,
            revision,
            tx,
            running,
        }
    }

    pub fn snapshot(&self) -> NowPlaying {
        self.snapshot.read().clone()
    }

    /// Monotonic counter over every change to what is playing. Equal
    /// revisions mean the notch would draw the same Now Playing it drew last
    /// time, without having to clone and compare the snapshot.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub fn play_pause(&self) {
        let _ = self.tx.send(Command::PlayPause);
    }

    pub fn next(&self) {
        let _ = self.tx.send(Command::Next);
    }

    pub fn previous(&self) {
        let _ = self.tx.send(Command::Previous);
    }
}

impl Drop for MediaWatcher {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

fn run(
    snapshot: Arc<RwLock<NowPlaying>>,
    revision: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
    rx: Receiver<Command>,
) {
    // COM apartment for this thread only; the overlay's own STA thread is
    // initialized separately and neither knows about the other.
    if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
        return;
    }

    let mut art_generation: u64 = 0;
    let mut last_track_key: Option<String> = None;

    while running.load(Ordering::Relaxed) {
        poll_once(
            &snapshot,
            &revision,
            &mut art_generation,
            &mut last_track_key,
        );

        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(cmd) => handle_command(cmd),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    unsafe { CoUninitialize() };
}

fn current_session() -> Option<windows::Media::Control::GlobalSystemMediaTransportControlsSession> {
    let manager = SessionManager::RequestAsync().ok()?.get().ok()?;
    manager.GetCurrentSession().ok()
}

fn handle_command(cmd: Command) {
    let Some(session) = current_session() else {
        return;
    };
    let _ = match cmd {
        Command::PlayPause => session.TryTogglePlayPauseAsync().and_then(|op| op.get()),
        Command::Next => session.TrySkipNextAsync().and_then(|op| op.get()),
        Command::Previous => session.TrySkipPreviousAsync().and_then(|op| op.get()),
    };
}

/// Publish `next` and bump the revision, but only if it differs from what is
/// already there. Most polls find the same track in the same state, and the
/// notch treats a bumped revision as a reason to repaint.
fn publish(snapshot: &Arc<RwLock<NowPlaying>>, revision: &Arc<AtomicU64>, next: NowPlaying) {
    {
        let current = snapshot.read();
        // `art` is compared through `art_generation`, which the poller bumps
        // whenever it reads new bytes — cheaper than comparing the bytes.
        if current.has_session == next.has_session
            && current.playing == next.playing
            && current.title == next.title
            && current.artist == next.artist
            && current.art_generation == next.art_generation
        {
            return;
        }
    }

    *snapshot.write() = next;
    revision.fetch_add(1, Ordering::Release);
}

fn poll_once(
    snapshot: &Arc<RwLock<NowPlaying>>,
    revision: &Arc<AtomicU64>,
    art_generation: &mut u64,
    last_track_key: &mut Option<String>,
) {
    let Some(session) = current_session() else {
        *last_track_key = None;
        publish(snapshot, revision, NowPlaying::default());
        return;
    };

    let playing = session
        .GetPlaybackInfo()
        .and_then(|info| info.PlaybackStatus())
        .map(|s| s == PlaybackStatus::Playing)
        .unwrap_or(false);

    let props = session.TryGetMediaPropertiesAsync().and_then(|op| op.get());
    let (title, artist, thumb_ref) = match &props {
        Ok(p) => (
            p.Title().map(|h| h.to_string_lossy()).unwrap_or_default(),
            p.Artist().map(|h| h.to_string_lossy()).unwrap_or_default(),
            p.Thumbnail().ok(),
        ),
        Err(_) => (String::new(), String::new(), None),
    };

    // The thumbnail read is the expensive part of a poll; only redo it when
    // the track has actually changed.
    let key = format!("{title}\u{0}{artist}");
    let track_changed = Some(&key) != last_track_key.as_ref();
    *last_track_key = Some(key);

    let art = if track_changed {
        let bytes = thumb_ref.and_then(|r| read_thumbnail(&r));
        if bytes.is_some() {
            *art_generation += 1;
        }
        bytes.map(Arc::from)
    } else {
        snapshot.read().art.clone()
    };

    publish(
        snapshot,
        revision,
        NowPlaying {
            has_session: true,
            playing,
            title,
            artist,
            art,
            art_generation: *art_generation,
        },
    );
}

fn read_thumbnail(reference: &IRandomAccessStreamReference) -> Option<Vec<u8>> {
    let stream = reference.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > MAX_ART_BYTES {
        return None;
    }

    let input: IInputStream = stream.cast().ok()?;
    let reader = DataReader::CreateDataReader(&input).ok()?;
    reader.LoadAsync(size as u32).ok()?.get().ok()?;

    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}
