mod autostart;
mod config;
mod flash;
mod gui;
mod notch;
mod overlay;
mod tray;

use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Instant;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Console::FreeConsole;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, QS_ALLINPUT,
};

/// Target frame interval for the overlay thread, in milliseconds.
const FRAME_MS: u32 = 16;

use config::AppConfig;
use flash::FlashManager;
use gui::SettingsApp;
use notch::NotchManager;
use overlay::OverlayManager;
use tray::SystemTray;

fn create_app_icon_data() -> Option<egui::IconData> {
    let width = 32;
    let height = 32;
    let mut rgba = vec![0u8; width * height * 4];

    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 4;
            let is_stroke = (x >= 4 && x <= 27 && (y == 4 || y == 5 || y == 26 || y == 27))
                || (y >= 4 && y <= 27 && (x == 4 || x == 5 || x == 26 || x == 27));

            let is_corner = (x <= 8 || x >= 23) && (y <= 8 || y >= 23);

            if is_stroke {
                if is_corner {
                    // Cyan Accent Corners
                    rgba[idx] = 56; // R
                    rgba[idx + 1] = 189; // G
                    rgba[idx + 2] = 248; // B
                    rgba[idx + 3] = 255; // A
                } else {
                    // Minimal Crisp Silver Outline Stroke
                    rgba[idx] = 230; // R
                    rgba[idx + 1] = 230; // G
                    rgba[idx + 2] = 230; // B
                    rgba[idx + 3] = 255; // A
                }
            } else {
                // Fully transparent interior
                rgba[idx] = 0;
                rgba[idx + 1] = 0;
                rgba[idx + 2] = 0;
                rgba[idx + 3] = 0;
            }
        }
    }

    Some(egui::IconData {
        rgba,
        width: width as u32,
        height: height as u32,
    })
}

/// Named mutex held for the life of the process. A second launch — say, the
/// user double-clicking the exe while the tray copy is already running —
/// finds it taken and quietly exits instead of fighting over the notch.
fn acquire_single_instance() -> bool {
    unsafe {
        let name = HSTRING::from("Local\\Venu.SingleInstance");
        match CreateMutexW(None, false, PCWSTR(name.as_ptr())) {
            Ok(handle) => {
                // Bound but never closed: the mutex must outlive `main`, and
                // HANDLE carries no destructor of its own.
                let _ = handle;
                GetLastError() != ERROR_ALREADY_EXISTS
            }
            // If the mutex cannot be created, assume we are alone rather than
            // refusing to start for everyone.
            Err(_) => true,
        }
    }
}

/// Keep the registry autostart entry and the saved preference in step.
///
/// - Preference on: rewrite the entry every run, so an updated or moved
///   `venu.exe` never leaves a stale path registered for sign-in.
/// - Preference off but an entry exists: the installer wrote it, or the
///   config file was reset. Adopt it, so the toggle in Preferences matches
///   what actually happens at sign-in.
fn reconcile_autostart(config: &RwLock<AppConfig>) {
    let mut cfg = config.write();
    if cfg.launch_on_startup {
        if let Err(e) = autostart::enable() {
            eprintln!("[startup] could not refresh the autostart entry: {e}");
        }
    } else if autostart::is_registered() {
        match autostart::enable() {
            Ok(()) => {
                cfg.launch_on_startup = true;
                cfg.save();
            }
            Err(e) => eprintln!("[startup] could not adopt the autostart entry: {e}"),
        }
    }
}

fn main() {
    // Immediately detach console when launched from Windows Explorer
    unsafe {
        let _ = FreeConsole();
    }

    // A second copy would draw a second notch in the same place.
    if !acquire_single_instance() {
        return;
    }

    let config = Arc::new(RwLock::new(AppConfig::load()));

    // The autostart entry is owned by the saved preference (the toggle under
    // Settings > App > Preferences); reconcile before anything renders.
    reconcile_autostart(&config);

    // Spawn Overlay Render, System Tray & Win32 Message Loop thread
    let overlay_config = Arc::clone(&config);
    std::thread::spawn(move || {
        // WIC (used for notch wallpapers) needs an initialised apartment on
        // whichever thread decodes the image.
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
        }

        let _tray = SystemTray::new().ok();
        let mut manager = OverlayManager::new();
        let mut notch = NotchManager::new(Arc::clone(&overlay_config));
        let mut flash = FlashManager::new();
        let mut last_instant = Instant::now();

        loop {
            // Process Win32 Message Queue for layered windows & System Tray
            unsafe {
                let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
                while windows::Win32::UI::WindowsAndMessaging::PeekMessageW(
                    &mut msg,
                    None,
                    0,
                    0,
                    windows::Win32::UI::WindowsAndMessaging::PM_REMOVE,
                )
                .as_bool()
                {
                    let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
                    windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
                }
            }

            let now = Instant::now();
            let dt = now.duration_since(last_instant).as_secs_f32();
            last_instant = now;

            {
                let cfg = overlay_config.read();
                manager.render_tick(&cfg, dt);
                flash.tick(&cfg, dt);
            }

            // Takes its own lock: inline editing writes back into the config.
            notch.tick(dt);

            // Wait for the next frame *or* the next input message, whichever
            // comes first. A plain sleep here would hold mouse messages for up
            // to a frame before answering them — and since the notch window
            // covers a wide strip along the top of the screen, that shows up as
            // a cursor that drags whenever it crosses that strip.
            unsafe {
                let elapsed = last_instant.elapsed().as_millis() as u32;
                let wait = FRAME_MS.saturating_sub(elapsed);
                if wait > 0 {
                    MsgWaitForMultipleObjectsEx(None, wait, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                }
            }
        }
    });

    // `--startup` is what the sign-in entry appends: at boot Venu should come
    // up in the tray, not open the settings window over the desktop. The tray
    // menu restores the window through the same path as "Open Settings".
    // `--minimized` is an alias for launching it by hand the same way.
    let quiet = std::env::args()
        .skip(1)
        .any(|a| a == "--startup" || a == "--minimized");

    let mut viewport_builder = eframe::egui::ViewportBuilder::default()
        .with_title("Venu - Settings")
        .with_inner_size([760.0, 600.0])
        .with_min_inner_size([620.0, 480.0])
        .with_visible(!quiet)
        .with_active(!quiet);

    if let Some(icon) = create_app_icon_data() {
        viewport_builder = viewport_builder.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport: viewport_builder,
        ..Default::default()
    };

    let gui_config = Arc::clone(&config);
    let _ = eframe::run_native(
        "Venu",
        native_options,
        Box::new(|cc| Ok(Box::new(SettingsApp::new(cc, gui_config)))),
    );
}
