//! Clipboard history, and putting things on the clipboard.
//!
//! History is opt-in and lives in memory only. It is fed by Windows itself:
//! a hidden message-only window registers with `AddClipboardFormatListener`
//! and is told about every change, so nothing polls. Content that other apps
//! mark as private — password managers set the formats below — is never
//! read, and turning the feature off drops everything collected.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;

use parking_lot::{Mutex, RwLock};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Foundation::{
    GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BI_RGB};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnLoad,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    RemoveClipboardFormatListener, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Shell::DROPFILES;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW,
    RegisterClassW, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WM_CLIPBOARDUPDATE, WM_CLOSE, WNDCLASSW,
    WS_OVERLAPPED,
};

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;

/// Longest entry kept, in characters. A copied log file is not a snippet.
const MAX_CHARS: usize = 4000;
/// Largest image put on the clipboard as a bitmap, per side.
const MAX_IMAGE_SIDE: u32 = 8192;

#[derive(Default)]
struct History {
    items: VecDeque<String>,
}

static HISTORY: RwLock<History> = RwLock::new(History {
    items: VecDeque::new(),
});
static REVISION: AtomicU64 = AtomicU64::new(0);
static LIMIT: AtomicU32 = AtomicU32::new(15);
/// The listener window, while it runs.
static LISTENER: Mutex<Option<isize>> = Mutex::new(None);

/// Revision of the history, for the notch's frame key.
pub fn revision() -> u64 {
    REVISION.load(Ordering::Acquire)
}

/// The history, newest first.
pub fn items() -> Vec<String> {
    HISTORY.read().items.iter().cloned().collect()
}

/// How many entries there are, without cloning them.
pub fn len() -> usize {
    HISTORY.read().items.len()
}

/// The newest entry, if any.
pub fn latest() -> Option<String> {
    HISTORY.read().items.front().cloned()
}

pub fn clear() {
    HISTORY.write().items.clear();
    REVISION.fetch_add(1, Ordering::AcqRel);
}

/// Start or stop listening to match the settings. Called every tick; a lock
/// and a comparison unless something changed.
pub fn sync(enabled: bool, max_items: u32) {
    LIMIT.store(
        max_items.clamp(1, crate::config::ClipboardConfig::MAX),
        Ordering::Relaxed,
    );
    let mut listener = LISTENER.lock();
    match (enabled, *listener) {
        (true, None) => {
            // Marked as running before the thread starts, so the next tick
            // does not start a second one.
            *listener = Some(0);
            let _ = std::thread::Builder::new()
                .name("Venu clipboard".to_owned())
                .spawn(run);
        }
        (false, Some(hwnd)) => {
            *listener = None;
            if hwnd != 0 {
                unsafe {
                    let _ = PostMessageW(HWND(hwnd as *mut _), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            // Off means forgotten.
            clear();
        }
        _ => {}
    }
}

/// Add an entry, newest first, moving a repeat to the front.
fn remember(text: String) {
    let limit = LIMIT.load(Ordering::Relaxed) as usize;
    let mut history = HISTORY.write();
    if history.items.front() == Some(&text) {
        return;
    }
    history.items.retain(|item| item != &text);
    history.items.push_front(text);
    history.items.truncate(limit);
    drop(history);
    REVISION.fetch_add(1, Ordering::AcqRel);
}

fn run() {
    unsafe {
        let Ok(instance) = GetModuleHandleW(None) else {
            *LISTENER.lock() = None;
            return;
        };
        let class = HSTRING::from("VenuClipboardListener");
        let wc = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            lpfnWndProc: Some(listener_proc),
            ..Default::default()
        };
        // A second registration after a restart fails harmlessly.
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            instance,
            None,
        ) else {
            *LISTENER.lock() = None;
            return;
        };

        {
            let mut listener = LISTENER.lock();
            if listener.is_none() {
                // Turned off before the window existed.
                let _ = DestroyWindow(hwnd);
                return;
            }
            *listener = Some(hwnd.0 as isize);
        }

        if AddClipboardFormatListener(hwnd).is_err() {
            eprintln!("[clipboard] Windows refused the clipboard listener");
            let _ = DestroyWindow(hwnd);
            *LISTENER.lock() = None;
            return;
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn listener_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            if let Some(text) = read_text(hwnd) {
                remember(text);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = RemoveClipboardFormatListener(hwnd);
            let _ = DestroyWindow(hwnd);
            windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Whether the clipboard's owner asked monitors like this one to look away.
fn private() -> bool {
    static FORMATS: OnceLock<[u32; 2]> = OnceLock::new();
    let formats = FORMATS.get_or_init(|| unsafe {
        [
            RegisterClipboardFormatW(PCWSTR(
                HSTRING::from("ExcludeClipboardContentFromMonitorProcessing").as_ptr(),
            )),
            RegisterClipboardFormatW(PCWSTR(HSTRING::from("Clipboard Viewer Ignore").as_ptr())),
        ]
    });
    formats
        .iter()
        .any(|&format| format != 0 && unsafe { IsClipboardFormatAvailable(format) }.is_ok())
}

/// "Do not put this in clipboard history", as Windows' own history reads it:
/// a `CanIncludeInClipboardHistory` DWORD of zero.
unsafe fn history_declined() -> bool {
    static FORMAT: OnceLock<u32> = OnceLock::new();
    let format = *FORMAT.get_or_init(|| {
        RegisterClipboardFormatW(PCWSTR(
            HSTRING::from("CanIncludeInClipboardHistory").as_ptr(),
        ))
    });
    if format == 0 || IsClipboardFormatAvailable(format).is_err() {
        return false;
    }
    let Ok(handle) = GetClipboardData(format) else {
        return false;
    };
    let global = HGLOBAL(handle.0);
    let ptr = GlobalLock(global) as *const u32;
    if ptr.is_null() {
        return false;
    }
    let value = *ptr;
    let _ = GlobalUnlock(global);
    value == 0
}

/// The clipboard's text, unless it is private or empty.
fn read_text(hwnd: HWND) -> Option<String> {
    if private() || unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) }.is_err() {
        return None;
    }
    unsafe {
        // Another app may hold the clipboard for a moment after writing.
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(hwnd).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        if !opened {
            return None;
        }
        let text = if history_declined() {
            None
        } else {
            GetClipboardData(CF_UNICODETEXT)
                .ok()
                .and_then(|handle| read_wide(HGLOBAL(handle.0)))
        };
        let _ = CloseClipboard();
        text.map(|t| bounded(&t)).filter(|t| !t.trim().is_empty())
    }
}

unsafe fn read_wide(global: HGLOBAL) -> Option<String> {
    let ptr = GlobalLock(global) as *const u16;
    if ptr.is_null() {
        return None;
    }
    let mut len = 0usize;
    // Bounded scan: never trust a missing terminator.
    while len < MAX_CHARS * 2 && *ptr.add(len) != 0 {
        len += 1;
    }
    let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
    let _ = GlobalUnlock(global);
    Some(text)
}

fn bounded(text: &str) -> String {
    text.chars().take(MAX_CHARS).collect()
}

/// One line for a list row: whitespace runs collapsed, newlines shown as a
/// return mark so a multi-line snippet is still recognisable.
pub fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(200));
    let mut space = false;
    for ch in text.trim().chars().take(200) {
        if ch == '\n' {
            out.push_str(" \u{21B5} ");
            space = true;
        } else if ch.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.push(ch);
            space = false;
        }
    }
    out
}

// -- writing -----------------------------------------------------------------

/// Owns a block of global memory until the clipboard takes it.
struct Global(HGLOBAL);

impl Global {
    fn with_bytes(bytes: &[u8]) -> Option<Self> {
        unsafe {
            let global = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).ok()?;
            let ptr = GlobalLock(global) as *mut u8;
            if ptr.is_null() {
                let _ = GlobalFree(global);
                return None;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            let _ = GlobalUnlock(global);
            Some(Self(global))
        }
    }

    /// Hand the block to the clipboard, which frees it from now on.
    unsafe fn give(self, format: u32) -> bool {
        let handle = HANDLE(self.0 .0);
        let given = SetClipboardData(format, handle).is_ok();
        if !given {
            let _ = GlobalFree(self.0);
        }
        std::mem::forget(self);
        given
    }
}

impl Drop for Global {
    fn drop(&mut self) {
        unsafe {
            let _ = GlobalFree(self.0);
        }
    }
}

/// Replace the clipboard with `blocks`, each `(format, bytes)`.
fn put(blocks: Vec<(u32, Vec<u8>)>) -> bool {
    let globals: Vec<(u32, Global)> = blocks
        .into_iter()
        .filter_map(|(format, bytes)| Global::with_bytes(&bytes).map(|g| (format, g)))
        .collect();
    if globals.is_empty() {
        return false;
    }
    unsafe {
        // The clipboard needs an owner window: opened without one, Windows
        // empties it and then refuses the new data. A throwaway message-only
        // window on this thread is the cheapest owner there is.
        let class = HSTRING::from("STATIC");
        let Ok(owner) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            None,
            None,
        ) else {
            return false;
        };
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(owner).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        let mut any = false;
        if opened {
            let _ = EmptyClipboard();
            for (format, global) in globals {
                any |= global.give(format);
            }
            let _ = CloseClipboard();
        }
        let _ = DestroyWindow(owner);
        any
    }
}

fn wide_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// Put text on the clipboard.
pub fn copy_text(text: &str) -> bool {
    put(vec![(CF_UNICODETEXT, wide_bytes(text))])
}

/// Put a file on the clipboard the way Explorer's Copy does, so it pastes
/// into Explorer, chat apps and browsers. Images also go on as a bitmap (and
/// PNGs as PNG), so they paste straight into an image editor or a message.
/// Runs on its own thread: decoding a large image takes a moment.
pub fn copy_file(path: &str) {
    let path = path.to_owned();
    let _ = std::thread::Builder::new()
        .name("Venu copy".to_owned())
        .spawn(move || {
            let mut blocks = vec![(CF_HDROP, drop_files(&path))];
            let lower = path.to_ascii_lowercase();
            if lower.ends_with(".png") {
                if let Ok(bytes) = std::fs::read(&path) {
                    let png =
                        unsafe { RegisterClipboardFormatW(PCWSTR(HSTRING::from("PNG").as_ptr())) };
                    if png != 0 {
                        blocks.push((png, bytes));
                    }
                }
            }
            let image = [
                ".png", ".jpg", ".jpeg", ".bmp", ".gif", ".webp", ".tif", ".tiff",
            ]
            .iter()
            .any(|ext| lower.ends_with(ext));
            if image {
                if let Some(dib) = decode_dib(&path) {
                    blocks.push((CF_DIB, dib));
                }
            }
            put(blocks);
        });
}

/// A `CF_HDROP` block naming one file.
fn drop_files(path: &str) -> Vec<u8> {
    let header = DROPFILES {
        pFiles: std::mem::size_of::<DROPFILES>() as u32,
        pt: POINT::default(),
        fNC: false.into(),
        fWide: true.into(),
    };
    let mut bytes = Vec::with_capacity(std::mem::size_of::<DROPFILES>() + path.len() * 2 + 4);
    // SAFETY: DROPFILES is plain old data; viewing it as bytes is sound.
    let raw = unsafe {
        std::slice::from_raw_parts(
            &header as *const DROPFILES as *const u8,
            std::mem::size_of::<DROPFILES>(),
        )
    };
    bytes.extend_from_slice(raw);
    // The file list ends with an extra terminator.
    bytes.extend(wide_bytes(path));
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

/// Decode an image file to a bottom-up 32-bit `CF_DIB`.
fn decode_dib(path: &str) -> Option<Vec<u8>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let result = (|| {
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let wide = HSTRING::from(path);
            let decoder = wic
                .CreateDecoderFromFilename(
                    PCWSTR(wide.as_ptr()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnLoad,
                )
                .ok()?;
            let frame = decoder.GetFrame(0).ok()?;
            let converter = wic.CreateFormatConverter().ok()?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeMedianCut,
                )
                .ok()?;
            let (mut w, mut h) = (0u32, 0u32);
            converter.GetSize(&mut w, &mut h).ok()?;
            if w == 0 || h == 0 || w > MAX_IMAGE_SIDE || h > MAX_IMAGE_SIDE {
                return None;
            }
            let stride = w * 4;
            let mut pixels = vec![0u8; (stride * h) as usize];
            converter
                .CopyPixels(std::ptr::null(), stride, &mut pixels)
                .ok()?;

            let header = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                // Positive: bottom-up, which every app reads correctly.
                biHeight: h as i32,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let mut dib =
                Vec::with_capacity(std::mem::size_of::<BITMAPINFOHEADER>() + pixels.len());
            // SAFETY: BITMAPINFOHEADER is plain old data.
            dib.extend_from_slice(std::slice::from_raw_parts(
                &header as *const BITMAPINFOHEADER as *const u8,
                std::mem::size_of::<BITMAPINFOHEADER>(),
            ));
            for row in pixels.chunks_exact(stride as usize).rev() {
                dib.extend_from_slice(row);
            }
            Some(dib)
        })();
        CoUninitialize();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_read_as_one_line() {
        assert_eq!(one_line("  hello\n  world  "), "hello \u{21B5} world");
        assert_eq!(one_line("a\t\tb"), "a b");
    }

    #[test]
    fn history_keeps_newest_first_without_repeats() {
        clear();
        LIMIT.store(3, Ordering::Relaxed);
        for t in ["a", "b", "c", "a", "d"] {
            remember(t.to_owned());
        }
        assert_eq!(items(), vec!["d", "a", "c"]);
        clear();
    }
}
