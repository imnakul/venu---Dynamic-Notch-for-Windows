use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::OnceLock;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DestroyWindow, FindWindowW, GetCursorPos, GetWindowLongPtrW, PostMessageW,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetWindowLongPtrW, TrackPopupMenu,
    GWLP_USERDATA, HICON, ICONINFO, MF_STRING, TPM_BOTTOMALIGN, TPM_LEFTALIGN, WM_COMMAND,
    WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP, WM_USER, WNDCLASSW, WS_POPUP,
};

const WM_TRAY_ICON: u32 = WM_USER + 101;
const WM_VENU_OPEN_SETTINGS: u32 = WM_USER + 102;
const ID_TRAY_SETTINGS: usize = 2001;
const ID_TRAY_EXIT: usize = 2002;

static TRAY_CLASS_NAME: &str = "VenuTrayClass";

/// Explorer broadcasts this registered message to every top-level window
/// whenever it (re)starts — after a crash, an `explorer.exe` restart, or
/// because it simply wasn't up yet when this app registered its icon (the
/// case when Venu is set to launch at sign-in). Every tray icon on the
/// system is gone at that point; the fix is to listen for it and re-add.
static WM_TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

fn debug_log(msg: &str) {
    let wide: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        OutputDebugStringW(PCWSTR(wide.as_ptr()));
    }
}

/// Set by the tray thread when an existing Settings window should be shown.
/// `SettingsApp::update` consumes this and uses eframe viewport commands so
/// winit's visibility and focus state stay in sync.
pub static SHOW_REQUESTED: AtomicBool = AtomicBool::new(false);
static SETTINGS_REQUEST_SENDER: OnceLock<SyncSender<()>> = OnceLock::new();

/// Install the main thread's wake channel before the tray thread starts. A
/// one-slot channel coalesces repeated clicks while the Settings window is
/// still being constructed.
pub fn install_settings_request_channel() -> Receiver<()> {
    let (sender, receiver) = sync_channel(1);
    let _ = SETTINGS_REQUEST_SENDER.set(sender);
    receiver
}

/// Request Settings to open. Before eframe exists this wakes the main thread;
/// afterward it asks the existing eframe window to restore itself.
pub fn request_settings_window() {
    crate::startup_log::record_event("settings_request_queued");
    SHOW_REQUESTED.store(true, Ordering::SeqCst);
    if let Some(ctx) = crate::gui::get_egui_context() {
        crate::startup_log::record_event("settings_request_egui_repaint_requested");
        if crate::gui::wake_settings_window() {
            crate::startup_log::record_event("settings_request_native_restore_posted");
        } else {
            crate::startup_log::record_event("settings_request_native_target_missing");
        }
        ctx.request_repaint();
    } else if let Some(sender) = SETTINGS_REQUEST_SENDER.get() {
        crate::startup_log::record_event("settings_request_channel_send_attempted");
        let _ = sender.try_send(());
    }
}

pub struct SystemTray {
    hwnd: HWND,
    nid: NOTIFYICONDATAW,
    hicon: HICON,
}

unsafe impl Send for SystemTray {}
unsafe impl Sync for SystemTray {}

fn create_app_icon() -> windows::core::Result<HICON> {
    let width: i32 = 32;
    let height: i32 = 32;

    unsafe {
        let mem_dc = CreateCompatibleDC(None);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // Top-down DIB
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let hcolor = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;

        if !bits.is_null() {
            let pixel_slice =
                std::slice::from_raw_parts_mut(bits as *mut u8, (width * height * 4) as usize);

            for y in 0..height {
                for x in 0..width {
                    let idx = ((y * width + x) * 4) as usize;

                    // Minimal, professional outline frame shape (2px stroke line)
                    let is_stroke = (x >= 4 && x <= 27 && (y == 4 || y == 5 || y == 26 || y == 27))
                        || (y >= 4 && y <= 27 && (x == 4 || x == 5 || x == 26 || x == 27));

                    let is_corner = (x <= 8 || x >= 23) && (y <= 8 || y >= 23);

                    if is_stroke {
                        if is_corner {
                            // Electric Cyan Accent Corners
                            pixel_slice[idx] = 248; // B
                            pixel_slice[idx + 1] = 189; // G
                            pixel_slice[idx + 2] = 56; // R
                            pixel_slice[idx + 3] = 255; // A
                        } else {
                            // Minimal Crisp Silver Outline Stroke
                            pixel_slice[idx] = 230; // B
                            pixel_slice[idx + 1] = 230; // G
                            pixel_slice[idx + 2] = 230; // R
                            pixel_slice[idx + 3] = 255; // A
                        }
                    } else {
                        // Fully transparent interior
                        pixel_slice[idx] = 0;
                        pixel_slice[idx + 1] = 0;
                        pixel_slice[idx + 2] = 0;
                        pixel_slice[idx + 3] = 0;
                    }
                }
            }
        }

        // Mask bitmap
        let hmask = CreateDIBSection(
            mem_dc,
            &bmi,
            DIB_RGB_COLORS,
            &mut std::ptr::null_mut(),
            None,
            0,
        )?;

        let icon_info = ICONINFO {
            fIcon: windows::Win32::Foundation::BOOL(1),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hmask,
            hbmColor: hcolor,
        };

        let hicon = CreateIconIndirect(&icon_info)?;

        let _ = DeleteObject(hcolor);
        let _ = DeleteObject(hmask);
        let _ = DeleteDC(mem_dc);

        Ok(hicon)
    }
}

pub fn restore_settings_window() {
    request_settings_window();
}

/// Ask the already-running Venu instance to open its Settings window. This is
/// used when a second launch explicitly supplies `--settings`.
pub fn request_existing_settings() {
    unsafe {
        let title = HSTRING::from("VenuTray");
        // The primary process may still be creating its hidden tray window
        // after acquiring the single-instance mutex.
        for _ in 0..20 {
            if let Ok(hwnd) = FindWindowW(None, PCWSTR(title.as_ptr())) {
                if !hwnd.is_invalid() {
                    if PostMessageW(hwnd, WM_VENU_OPEN_SETTINGS, WPARAM(0), LPARAM(0)).is_ok() {
                        crate::startup_log::record_event("settings_request_posted_to_primary");
                    } else {
                        crate::startup_log::record_event("settings_request_post_failed");
                    }
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        crate::startup_log::record_event("settings_request_target_missing");
    }
}

impl SystemTray {
    pub fn new() -> windows::core::Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class_hstr = HSTRING::from(TRAY_CLASS_NAME);

            let wnd_class = WNDCLASSW {
                hInstance: instance.into(),
                lpszClassName: PCWSTR(class_hstr.as_ptr()),
                lpfnWndProc: Some(Self::wnd_proc),
                ..Default::default()
            };
            RegisterClassW(&wnd_class);

            let hwnd = CreateWindowExW(
                Default::default(),
                PCWSTR(class_hstr.as_ptr()),
                PCWSTR(HSTRING::from("VenuTray").as_ptr()),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                instance,
                None,
            )?;
            crate::startup_log::record_event("tray_owner_window_created");

            let hicon = create_app_icon()?;

            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: WM_TRAY_ICON,
                hIcon: hicon,
                ..Default::default()
            };

            let tip = "Venu - Dynamic Notch for Windows\0"
                .encode_utf16()
                .collect::<Vec<u16>>();
            let len = tip.len().min(nid.szTip.len());
            nid.szTip[..len].copy_from_slice(&tip[..len]);

            // Stashed for `wnd_proc`, which is a bare extern "system" fn with
            // no access to `self` — this is how it re-adds the icon when
            // Explorer (re)starts. Leaked deliberately: it must outlive the
            // window, and the window outlives the app.
            let nid_ptr = Box::into_raw(Box::new(nid));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, nid_ptr as isize);

            let taskbar_created =
                RegisterWindowMessageW(PCWSTR(HSTRING::from("TaskbarCreated").as_ptr()));
            WM_TASKBAR_CREATED.store(taskbar_created, Ordering::SeqCst);

            if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                debug_log("Venu: Shell_NotifyIconW(NIM_ADD) failed — tray icon not registered yet, will retry on TaskbarCreated");
            }

            Ok(Self { hwnd, nid, hicon })
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_VENU_OPEN_SETTINGS => {
                crate::startup_log::record_event("settings_request_message_received");
                restore_settings_window();
                LRESULT(0)
            }
            WM_TRAY_ICON => {
                let msg_type = lparam.0 as u32;
                if msg_type == WM_LBUTTONDBLCLK || msg_type == WM_LBUTTONUP {
                    restore_settings_window();
                } else if msg_type == WM_RBUTTONUP {
                    let mut pos = POINT::default();
                    let _ = GetCursorPos(&mut pos);

                    let menu = CreatePopupMenu().unwrap();
                    let _ = AppendMenuW(
                        menu,
                        MF_STRING,
                        ID_TRAY_SETTINGS,
                        PCWSTR(HSTRING::from("⚙ Open Settings").as_ptr()),
                    );
                    let _ = AppendMenuW(
                        menu,
                        MF_STRING,
                        ID_TRAY_EXIT,
                        PCWSTR(HSTRING::from("❌ Exit Application").as_ptr()),
                    );

                    let _ = SetForegroundWindow(hwnd);
                    let _ = TrackPopupMenu(
                        menu,
                        TPM_LEFTALIGN | TPM_BOTTOMALIGN,
                        pos.x,
                        pos.y,
                        0,
                        hwnd,
                        None,
                    );
                    let _ = DestroyMenu(menu);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xffff) as usize;
                if id == ID_TRAY_SETTINGS {
                    restore_settings_window();
                } else if id == ID_TRAY_EXIT {
                    std::process::exit(0);
                }
                LRESULT(0)
            }
            other if other != 0 && other == WM_TASKBAR_CREATED.load(Ordering::SeqCst) => {
                // The whole notification area just came back — every icon on
                // the system, including ours, is gone until re-added.
                let nid_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const NOTIFYICONDATAW;
                if !nid_ptr.is_null() && !Shell_NotifyIconW(NIM_ADD, &*nid_ptr).as_bool() {
                    debug_log("MovingText: tray icon re-add after TaskbarCreated failed");
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

impl Drop for SystemTray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.nid);
            if !self.hicon.is_invalid() {
                let _ = DestroyIcon(self.hicon);
            }
            if !self.hwnd.is_invalid() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{install_settings_request_channel, request_settings_window};

    #[test]
    fn request_before_gui_startup_is_buffered_for_main_thread() {
        let requests = install_settings_request_channel();
        request_settings_window();

        assert_eq!(requests.try_recv(), Ok(()));
        assert!(requests.try_recv().is_err());
    }
}
