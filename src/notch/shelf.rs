//! The Shelf: files, folders, apps and scripts pinned to the notch, plus the
//! newest few files of one folder (Screenshots, Downloads…).
//!
//! Venu stores paths only. Nothing is copied, moved or indexed: opening an
//! item hands its path to Windows exactly as a double-click in Explorer
//! would.
//!
//! Everything that touches the disk or the shell runs on one background
//! thread: listing the folder, checking pinned paths still exist, and asking
//! the shell for icons and thumbnails, any of which can stall on a slow or
//! network drive. The painter only ever reads the finished snapshot. The
//! folder is re-listed when Windows reports a change in it
//! (`FindFirstChangeNotificationW`), not on a timer, and the thread only
//! exists while the Shelf is in the deck.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use parking_lot::{Mutex, RwLock};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{HANDLE, SIZE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::Storage::FileSystem::{
    FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, ShellExecuteW, SIIGBF_RESIZETOFIT,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::config::ShelfConfig;

/// Icon edge, in pixels. Drawn at 32 px, so this is crisp at 150% scaling.
const ICON_PX: i32 = 48;
/// How long the worker waits on a folder change before checking for new
/// instructions. Only the wait's timeout; nothing is re-read on it.
const WAIT_SLICE: Duration = Duration::from_millis(400);

/// One tile on the shelf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShelfEntry {
    pub path: String,
    /// What the tile says: the file name, minus `.lnk`/`.exe`/`.url`.
    pub name: String,
    pub pinned: bool,
    /// Pinned paths that were moved or deleted stay on the shelf, greyed,
    /// until the user removes them.
    pub exists: bool,
}

/// An icon or thumbnail, premultiplied BGRA, top-down.
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// What the painter reads.
#[derive(Default)]
pub struct Snapshot {
    pub pinned: Vec<ShelfEntry>,
    pub recent: Vec<ShelfEntry>,
    /// The peeked folder's own name, for the row heading.
    pub folder_name: String,
    pub icons: HashMap<String, Arc<Icon>>,
}

enum Command {
    Configure(ShelfConfig),
    Refresh,
    Stop,
}

struct Worker {
    tx: Sender<Command>,
    config: ShelfConfig,
    visible: bool,
}

static SNAPSHOT: OnceLock<RwLock<Snapshot>> = OnceLock::new();
static REVISION: AtomicU64 = AtomicU64::new(0);
static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

fn snapshot_cell() -> &'static RwLock<Snapshot> {
    SNAPSHOT.get_or_init(|| RwLock::new(Snapshot::default()))
}

/// Read the current shelf.
pub fn snapshot() -> parking_lot::RwLockReadGuard<'static, Snapshot> {
    snapshot_cell().read()
}

/// Bumped whenever the snapshot changes, for the notch's frame key.
pub fn revision() -> u64 {
    REVISION.load(Ordering::Acquire)
}

/// Keep the worker in step with the settings. Called every tick; costs a
/// comparison unless something changed. `in_deck` starts or stops the
/// worker; `visible` re-checks pinned paths each time the page comes into
/// view.
pub fn sync(cfg: &ShelfConfig, in_deck: bool, visible: bool) {
    let mut worker = WORKER.lock();
    if !in_deck {
        if let Some(w) = worker.take() {
            let _ = w.tx.send(Command::Stop);
        }
        return;
    }

    match worker.as_mut() {
        None => {
            let (tx, rx) = mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("Venu shelf".to_owned())
                .spawn(move || run(rx))
                .is_ok();
            if !spawned {
                eprintln!("[shelf] could not start the shelf thread");
                return;
            }
            let _ = tx.send(Command::Configure(cfg.clone()));
            *worker = Some(Worker {
                tx,
                config: cfg.clone(),
                visible,
            });
        }
        Some(w) => {
            if w.config != *cfg {
                w.config = cfg.clone();
                let _ = w.tx.send(Command::Configure(cfg.clone()));
            } else if visible && !w.visible {
                let _ = w.tx.send(Command::Refresh);
            }
            w.visible = visible;
        }
    }
}

/// The tile's caption.
pub fn display_name(path: &str) -> String {
    let trimmed = path.trim_end_matches(['\\', '/']);
    let file = Path::new(trimmed)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        // A bare drive ("C:\") has no file name.
        .unwrap_or_else(|| path.to_owned());
    for ext in [".lnk", ".exe", ".url"] {
        if file.len() > ext.len() && file.to_ascii_lowercase().ends_with(ext) {
            return file[..file.len() - ext.len()].to_owned();
        }
    }
    file
}

fn run(rx: Receiver<Command>) {
    // The shell's image factory is apartment-threaded.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();

    let mut config = ShelfConfig::default();
    let mut watch = FolderWatch::default();
    let mut icons: HashMap<String, Arc<Icon>> = HashMap::new();
    let mut dirty = true;

    loop {
        // Block on instructions, or on the folder, whichever comes first.
        let command = if watch.handle.is_some() {
            if watch.changed(WAIT_SLICE) {
                dirty = true;
            }
            match rx.try_recv() {
                Ok(command) => Some(command),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        } else if dirty {
            None
        } else {
            match rx.recv_timeout(Duration::from_secs(3600)) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        };

        match command {
            Some(Command::Stop) => break,
            Some(Command::Configure(next)) => {
                if next.folder != config.folder {
                    watch = FolderWatch::open(&next.folder);
                }
                config = next;
                dirty = true;
            }
            Some(Command::Refresh) => dirty = true,
            None => {}
        }

        if !dirty {
            continue;
        }
        dirty = false;

        let pinned: Vec<ShelfEntry> = config
            .items
            .iter()
            .map(|path| ShelfEntry {
                path: path.clone(),
                name: display_name(path),
                pinned: true,
                exists: Path::new(path).exists(),
            })
            .collect();
        let recent = newest_files(&config.folder, config.folder_count as usize);
        let folder_name = if config.folder.trim().is_empty() {
            String::new()
        } else {
            display_name(&config.folder)
        };

        // Icons for whatever is on the shelf now; drop the rest.
        let wanted: Vec<&ShelfEntry> = pinned.iter().chain(recent.iter()).collect();
        icons.retain(|path, _| wanted.iter().any(|e| &e.path == path));
        if com {
            for entry in wanted.iter().filter(|e| e.exists) {
                if !icons.contains_key(&entry.path) {
                    if let Some(icon) = extract_icon(&entry.path) {
                        icons.insert(entry.path.clone(), Arc::new(icon));
                    }
                }
            }
        }

        {
            let mut snap = snapshot_cell().write();
            snap.pinned = pinned;
            snap.recent = recent;
            snap.folder_name = folder_name;
            snap.icons = icons.clone();
        }
        REVISION.fetch_add(1, Ordering::AcqRel);
    }

    drop(watch);
    *snapshot_cell().write() = Snapshot::default();
    REVISION.fetch_add(1, Ordering::AcqRel);
    if com {
        unsafe { CoUninitialize() };
    }
}

/// The newest `count` files directly in `folder`, newest first. Hidden and
/// system files (desktop.ini and friends) are skipped.
fn newest_files(folder: &str, count: usize) -> Vec<ShelfEntry> {
    if folder.trim().is_empty() || count == 0 {
        return Vec::new();
    }
    let Ok(dir) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files: Vec<(SystemTime, String)> = dir
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_file() || hidden(&meta) {
                return None;
            }
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            Some((modified, entry.path().to_string_lossy().into_owned()))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files
        .into_iter()
        .take(count)
        .map(|(_, path)| ShelfEntry {
            name: display_name(&path),
            path,
            pinned: false,
            exists: true,
        })
        .collect()
}

fn hidden(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    meta.file_attributes() & (HIDDEN | SYSTEM) != 0
}

/// A change-notification handle on the peeked folder.
#[derive(Default)]
struct FolderWatch {
    handle: Option<HANDLE>,
}

impl FolderWatch {
    fn open(folder: &str) -> Self {
        if folder.trim().is_empty() {
            return Self::default();
        }
        let path = HSTRING::from(folder);
        let handle = unsafe {
            FindFirstChangeNotificationW(
                PCWSTR(path.as_ptr()),
                false,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE,
            )
        }
        .ok()
        .filter(|h| !h.is_invalid());
        Self { handle }
    }

    /// Wait up to `timeout` for a change. Re-arms the notification after one.
    fn changed(&self, timeout: Duration) -> bool {
        let Some(handle) = self.handle else {
            return false;
        };
        let result = unsafe { WaitForSingleObject(handle, timeout.as_millis() as u32) };
        if result == WAIT_OBJECT_0 {
            let _ = unsafe { FindNextChangeNotification(handle) };
            true
        } else {
            false
        }
    }
}

impl Drop for FolderWatch {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = unsafe { FindCloseChangeNotification(handle) };
        }
    }
}

/// The shell's own picture for a path: a thumbnail for images and videos,
/// the app's icon for programs and shortcuts, the folder icon for folders.
fn extract_icon(path: &str) -> Option<Icon> {
    unsafe {
        let wide = HSTRING::from(path);
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).ok()?;
        let bitmap = factory
            .GetImage(
                SIZE {
                    cx: ICON_PX,
                    cy: ICON_PX,
                },
                SIIGBF_RESIZETOFIT,
            )
            .ok()?;
        let icon = read_bitmap(bitmap);
        let _ = DeleteObject(bitmap);
        icon
    }
}

/// Copy an `HBITMAP` out as premultiplied, top-down BGRA.
unsafe fn read_bitmap(bitmap: HBITMAP) -> Option<Icon> {
    let mut info = BITMAP::default();
    let got = GetObjectW(
        bitmap,
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut info as *mut BITMAP as *mut std::ffi::c_void),
    );
    if got == 0 || info.bmWidth <= 0 || info.bmHeight == 0 {
        return None;
    }
    let (w, h) = (info.bmWidth, info.bmHeight.abs());
    let mut header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    let dc = GetDC(None);
    let rows = GetDIBits(
        dc,
        bitmap,
        0,
        h as u32,
        Some(bgra.as_mut_ptr() as *mut std::ffi::c_void),
        &mut header,
        DIB_RGB_COLORS,
    );
    ReleaseDC(None, dc);
    if rows == 0 {
        return None;
    }
    normalise_alpha(&mut bgra);
    Some(Icon {
        width: w as u32,
        height: h as u32,
        bgra,
    })
}

/// Shell bitmaps arrive either with no alpha at all (photo thumbnails) or
/// with straight alpha (icons). Direct2D wants premultiplied.
fn normalise_alpha(bgra: &mut [u8]) {
    if bgra.chunks_exact(4).all(|px| px[3] == 0) {
        for px in bgra.chunks_exact_mut(4) {
            px[3] = 255;
        }
        return;
    }
    let straight = bgra
        .chunks_exact(4)
        .any(|px| px[0] > px[3] || px[1] > px[3] || px[2] > px[3]);
    if straight {
        for px in bgra.chunks_exact_mut(4) {
            let a = px[3] as u32;
            for c in &mut px[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
}

/// Open a path the way a double-click in Explorer would. On its own thread:
/// the shell can take a moment to start an app, and the notch must not wait.
pub fn open(path: &str) {
    let path = path.to_owned();
    let _ = std::thread::Builder::new()
        .name("Venu open".to_owned())
        .spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let file = HSTRING::from(path.as_str());
            let verb = HSTRING::from("open");
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
            CoUninitialize();
        });
}

/// Show a path selected in its folder.
pub fn reveal(path: &str) {
    let path = path.to_owned();
    let _ = std::thread::Builder::new()
        .name("Venu reveal".to_owned())
        .spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let verb = HSTRING::from("open");
            let explorer = HSTRING::from("explorer.exe");
            let args = HSTRING::from(format!("/select,\"{path}\""));
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(explorer.as_ptr()),
                PCWSTR(args.as_ptr()),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
            CoUninitialize();
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_drop_shortcut_and_program_extensions_only() {
        assert_eq!(display_name(r"C:\Users\me\Desktop\Chrome.lnk"), "Chrome");
        assert_eq!(display_name(r"C:\tools\build.exe"), "build");
        assert_eq!(display_name(r"C:\tools\deploy.bat"), "deploy.bat");
        assert_eq!(
            display_name(r"C:\Users\me\Pictures\Screenshots\"),
            "Screenshots"
        );
        assert_eq!(display_name(r"D:\"), r"D:\");
    }

    #[test]
    fn opaque_thumbnails_get_alpha_and_icons_get_premultiplied() {
        let mut photo = vec![10, 20, 30, 0, 40, 50, 60, 0];
        normalise_alpha(&mut photo);
        assert_eq!(photo, vec![10, 20, 30, 255, 40, 50, 60, 255]);

        let mut icon = vec![200, 100, 50, 128];
        normalise_alpha(&mut icon);
        assert_eq!(icon, vec![100, 50, 25, 128]);

        let mut already = vec![50, 40, 30, 128];
        normalise_alpha(&mut already);
        assert_eq!(already, vec![50, 40, 30, 128]);
    }
}
