mod autostart;
mod config;
mod flash;
mod font_families;
mod fonts;
mod gui;
mod launch;
mod notch;
mod overlay;
mod startup_log;
mod stats;
mod stats_math;
mod tray;

use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Console::FreeConsole;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, QS_ALLINPUT,
};

/// Frame interval for the overlay thread while something is actually moving.
const FRAME_MS: u32 = 16;

/// Interval the overlay thread falls back to once nothing is animating.
///
/// Venu spends nearly all of its life like this: a collapsed pill showing the
/// same thing it showed a second ago. A tick at this rate reads the cursor,
/// drains the input bus and advances the springs — all of it arithmetic and a
/// syscall or two — and paints nothing, so the notch still opens the instant
/// the cursor reaches it without the process drawing sixty identical frames a
/// second to sit still.
const IDLE_POLL_MS: u32 = 32;

use config::AppConfig;
use flash::FlashManager;
use gui::SettingsApp;
use launch::settings_visible_from_args;
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

fn run_overlay_thread(overlay_config: Arc<RwLock<AppConfig>>) {
    startup_log::record_event("overlay_thread_started");
    // WIC (used for notch wallpapers) needs an initialised apartment on
    // whichever thread decodes the image.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }

    let _tray = match SystemTray::new() {
        Ok(tray) => Some(tray),
        Err(_) => {
            // A tray window is the normal Settings entry point. If Windows
            // refuses to create it, show Settings so the failure is visible
            // and the user can still reach the app controls.
            tray::request_settings_window();
            None
        }
    };
    let mut manager = OverlayManager::new();
    let mut notch = NotchManager::new(Arc::clone(&overlay_config));
    let mut flash = FlashManager::new();
    let mut last_instant = Instant::now();
    let mut next_tick = last_instant;

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
        // Input wakes the message pump, not the renderer. Rendering on
        // every wake lets mouse traffic (or our own window messages) run
        // the animation faster than the intended frame rate.
        if now < next_tick {
            let wait = (next_tick - now).as_millis() as u32 + 1;
            unsafe {
                MsgWaitForMultipleObjectsEx(None, wait, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            }
            continue;
        }
        let dt = now.duration_since(last_instant).as_secs_f32();
        last_instant = now;

        // Each of these reports whether it still has something in flight.
        // None of them do while Venu sits in the tray with the notch shut,
        // which is the state it is in almost all of the time.
        let mut animating = {
            let cfg = overlay_config.read();
            let overlays = manager.render_tick(&cfg, dt);
            let flashing = flash.tick(&cfg, dt);
            overlays || flashing
        };

        // Takes its own lock: inline editing writes back into the config.
        animating |= notch.tick(dt);

        let budget = if animating { FRAME_MS } else { IDLE_POLL_MS };
        next_tick = now + Duration::from_millis(budget as u64);
    }
}

fn main() {
    startup_log::install_panic_hook();

    // Immediately detach console when launched from Windows Explorer
    unsafe {
        let _ = FreeConsole();
    }

    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let show_settings = settings_visible_from_args(args.iter());
    startup_log::record_event("process_starting");

    // A second copy would draw a second notch in the same place.
    if !acquire_single_instance() {
        if show_settings {
            tray::request_existing_settings();
        }
        startup_log::record_event("secondary_instance_exiting");
        return;
    }

    let config = Arc::new(RwLock::new(AppConfig::load()));

    // The autostart entry is owned by the saved preference (the toggle under
    // Settings > App > Preferences); reconcile before anything renders.
    reconcile_autostart(&config);

    // Install the wake channel before starting the tray thread so early tray
    // or notch requests remain queued until the main thread is ready.
    let settings_requests = tray::install_settings_request_channel();

    // Spawn Overlay Render, System Tray & Win32 Message Loop thread.
    let overlay_config = Arc::clone(&config);
    let overlay_thread = std::thread::Builder::new()
        .name("Venu overlay".to_owned())
        .spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_overlay_thread(overlay_config)
            }))
            .is_err()
            {
                startup_log::record_event("overlay_thread_panicked");
                // The overlay thread's message loop is the tray's lifetime.
                // If it exits unexpectedly, wake Settings rather than leave
                // the main thread blocked with no visible way to recover.
                tray::request_settings_window();
            }
        });
    let overlay_started = overlay_thread.is_ok();
    startup_log::record_event(if overlay_started {
        "overlay_thread_spawned"
    } else {
        "overlay_thread_spawn_failed"
    });

    // Do not create eframe or a Settings window on a normal launch. eframe
    // forces its native viewport visible after the first rendered frame even
    // when NativeOptions requested a hidden window. The tray/notch wake channel
    // creates the GUI only when someone asks to open Settings.
    let show_settings = if show_settings {
        startup_log::record_event("settings_requested_by_launch_argument");
        true
    } else if !overlay_started {
        startup_log::record_event("settings_opening_after_overlay_spawn_failure");
        true
    } else {
        match settings_requests.recv() {
            Ok(()) => {
                startup_log::record_event("settings_request_received_by_main_thread");
                true
            }
            Err(_) => {
                startup_log::record_event("settings_request_channel_closed");
                false
            }
        }
    };
    if !show_settings {
        startup_log::record_event("process_exiting_without_settings");
        return;
    }
    startup_log::record_event("settings_gui_starting");

    let mut viewport_builder = eframe::egui::ViewportBuilder::default()
        .with_title("Venu - Settings")
        .with_inner_size([760.0, 600.0])
        .with_min_inner_size([620.0, 480.0])
        .with_visible(true)
        .with_active(true);

    if let Some(icon) = create_app_icon_data() {
        viewport_builder = viewport_builder.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport: viewport_builder,
        ..Default::default()
    };

    let gui_config = Arc::clone(&config);
    if let Err(error) = eframe::run_native(
        "Venu",
        native_options,
        Box::new(move |cc| Ok(Box::new(SettingsApp::new(cc, gui_config, show_settings)))),
    ) {
        startup_log::record_eframe_error(&error);
    }
    startup_log::record_event("settings_gui_event_loop_returned");
}
