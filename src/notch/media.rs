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
use std::time::{Duration, Instant};

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
    pub album: String,
    /// The app playing it, in words: "Spotify", "Chrome".
    pub source: String,
    /// Track length in seconds; 0 when the app does not report one (live
    /// streams, some browsers).
    pub duration_secs: f64,
    /// Position in seconds as of `position_at`. The painter runs it forward
    /// from there while playing, so it never needs a poll to tick.
    pub position_secs: f64,
    pub position_at: Option<Instant>,
}

impl NowPlaying {
    /// Where playback is now, in seconds, clamped to the track.
    pub fn position_now(&self) -> f64 {
        let mut pos = self.position_secs;
        if self.playing {
            if let Some(at) = self.position_at {
                pos += at.elapsed().as_secs_f64();
            }
        }
        if self.duration_secs > 0.0 {
            pos.clamp(0.0, self.duration_secs)
        } else {
            pos.max(0.0)
        }
    }
}

/// A media app's id as a name people know.
pub fn source_name(app_id: &str) -> String {
    let lower = app_id.to_ascii_lowercase();
    let known = [
        ("spotify", "Spotify"),
        ("chrome", "Chrome"),
        ("msedge", "Edge"),
        ("firefox", "Firefox"),
        // Firefox registers under a hash of its install path.
        ("308046b0af4a39cb", "Firefox"),
        ("brave", "Brave"),
        ("opera", "Opera"),
        ("vivaldi", "Vivaldi"),
        ("thebrowsercompany", "Arc"),
        ("zunemusic", "Media Player"),
        ("zunevideo", "Films & TV"),
        ("vlc", "VLC"),
        ("applemusic", "Apple Music"),
        ("itunes", "iTunes"),
        ("tidal", "TIDAL"),
        ("deezer", "Deezer"),
        ("youtube", "YouTube"),
    ];
    if let Some((_, name)) = known.iter().find(|(key, _)| lower.contains(key)) {
        return (*name).to_owned();
    }
    // "Publisher.App_hash!App" or "app.exe": keep the readable part.
    let tail = app_id.rsplit('!').next().unwrap_or(app_id);
    let tail = tail.rsplit(['\\', '/']).next().unwrap_or(tail);
    let tail = tail.strip_suffix(".exe").unwrap_or(tail);
    let tail = tail.rsplit('.').next().unwrap_or(tail);
    let mut chars = tail.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
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

    /// Whether something is playing, without cloning the snapshot: read on
    /// every frame for the music wave.
    pub fn is_playing(&self) -> bool {
        self.snapshot.read().playing
    }

    /// Monotonic counter over every change to what is playing. Equal
    /// revisions mean the notch would draw the same Now Playing it drew last
    /// time, without having to clone and compare the snapshot.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// The playback position to the second, for the open player's frame
    /// key: it has to repaint once a second while the time counts up, and
    /// not otherwise.
    pub fn position_key(&self) -> u64 {
        let now = self.snapshot.read();
        if !now.has_session || now.duration_secs <= 0.0 {
            return 0;
        }
        now.position_now() as u64 + 1
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
        let same = current.has_session == next.has_session
            && current.playing == next.playing
            && current.title == next.title
            && current.artist == next.artist
            && current.art_generation == next.art_generation
            && current.album == next.album
            && current.source == next.source
            && (current.duration_secs - next.duration_secs).abs() < 0.5;
        // Position moves on its own while playing; only a jump (a seek)
        // is news.
        let seeked = (current.position_now() - next.position_now()).abs() > 2.0;
        if same && !seeked {
            drop(current);
            // Keep the anchor fresh without calling it a change.
            let mut write = snapshot.write();
            write.position_secs = next.position_secs;
            write.position_at = next.position_at;
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
    let (title, artist, album, thumb_ref) = match &props {
        Ok(p) => (
            p.Title().map(|h| h.to_string_lossy()).unwrap_or_default(),
            p.Artist().map(|h| h.to_string_lossy()).unwrap_or_default(),
            p.AlbumTitle()
                .map(|h| h.to_string_lossy())
                .unwrap_or_default(),
            p.Thumbnail().ok(),
        ),
        Err(_) => (String::new(), String::new(), String::new(), None),
    };
    let source = session
        .SourceAppUserModelId()
        .map(|id| source_name(&id.to_string_lossy()))
        .unwrap_or_default();

    // Timeline: where the app says it is, as of when it said so. Ticks are
    // 100 ns; the two times are in the same clock, so their difference is
    // how long ago the app last reported.
    let (duration_secs, position_secs) = session
        .GetTimelineProperties()
        .ok()
        .map(|tl| {
            let ticks = |span: windows::core::Result<windows::Foundation::TimeSpan>| {
                span.map(|s| s.Duration as f64 / 1e7).unwrap_or(0.0)
            };
            let start = ticks(tl.StartTime());
            let end = ticks(tl.EndTime());
            let mut position = ticks(tl.Position()) - start;
            if playing {
                if let Ok(updated) = tl.LastUpdatedTime() {
                    let now = now_universal_ticks();
                    if now > updated.UniversalTime {
                        let since = (now - updated.UniversalTime) as f64 / 1e7;
                        // A stale report is better ignored than trusted.
                        if since < 600.0 {
                            position += since;
                        }
                    }
                }
            }
            ((end - start).max(0.0), position.max(0.0))
        })
        .unwrap_or((0.0, 0.0));

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
            album,
            source,
            duration_secs,
            position_secs,
            position_at: Some(Instant::now()),
        },
    );
}

/// Now, in the 100 ns ticks since 1601 that WinRT's `DateTime` uses.
fn now_universal_ticks() -> i64 {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (unix.as_nanos() / 100) as i64 + 116_444_736_000_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_apps_get_names_people_know() {
        assert_eq!(source_name("Spotify.exe"), "Spotify");
        assert_eq!(source_name("Chrome"), "Chrome");
        assert_eq!(source_name("MSEdge"), "Edge");
        assert_eq!(source_name("308046B0AF4A39CB"), "Firefox");
        assert_eq!(
            source_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"),
            "Media Player"
        );
        assert_eq!(source_name("foobar2000.exe"), "Foobar2000");
    }

    #[test]
    fn position_runs_forward_only_while_playing() {
        let mut now = NowPlaying {
            duration_secs: 100.0,
            position_secs: 10.0,
            position_at: Some(Instant::now() - Duration::from_secs(5)),
            ..Default::default()
        };
        assert!((now.position_now() - 10.0).abs() < 0.01);
        now.playing = true;
        assert!((now.position_now() - 15.0).abs() < 0.2);
        now.position_secs = 99.0;
        assert_eq!(now.position_now(), 100.0);
    }
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
