//! The notch overlay: a collapsed pill fused to the top bezel that springs
//! open on hover into a wide panel of slides.
//!
//! Module layout:
//!
//! * [`anim`]     — springs and easing, no Win32
//! * [`backdrop`] — the desktop capture behind the frosted theme
//! * [`state`]   — what is shown, how open it is, what is being typed
//! * [`geom`]    — the silhouette and its path
//! * [`hook`]    — the wheel hook, parked on its own thread
//! * [`media`]   — the Now Playing poller, on its own thread
//! * [`theme`]   — colour and type tokens
//! * [`text`]    — DirectWrite, including the bundled private fonts
//! * [`surface`] — the Direct2D target behind the layered window
//! * [`paint`]   — every slide, drawn
//! * [`window`]  — the layered window, hit-testing and input

pub mod anim;
pub mod backdrop;
pub mod geom;
pub mod hook;
pub mod media;
pub mod notify;
pub mod paint;
pub mod state;
pub mod surface;
pub mod text;
pub mod theme;
pub mod window;

use std::sync::Arc;

use parking_lot::RwLock;

use crate::config::AppConfig;
use window::NotchWindow;

/// Quiet period after the last change before the config is written to disk, so
/// a burst of typing or scrolling produces one write rather than dozens.
const SAVE_DEBOUNCE: f32 = 0.9;

pub struct NotchManager {
    config: Arc<RwLock<AppConfig>>,
    window: Option<NotchWindow>,
    save_countdown: Option<f32>,
    /// Creating the window is only retried on the next config change, not every
    /// frame, so a hard failure does not spam the log at 60Hz.
    create_failed: bool,
}

impl NotchManager {
    pub fn new(config: Arc<RwLock<AppConfig>>) -> Self {
        let (port, allowed, windows_listener_enabled, toast_duration_secs) = {
            let cfg = config.read();
            (
                cfg.notch.notifications.webhook_port,
                cfg.notch.notifications.allowed_apps.clone(),
                cfg.notch.notifications.windows_listener_enabled,
                cfg.notch.notifications.toast_duration_secs,
            )
        };
        notify::start_webhook_server(port, allowed);
        crate::native_notifications::set_toast_duration(toast_duration_secs);
        crate::native_notifications::set_enabled(windows_listener_enabled);

        Self {
            config,
            window: None,
            save_countdown: None,
            create_failed: false,
        }
    }

    /// Advance the notch by one frame. Returns whether it wants the next one
    /// at full rate; when it does not, the overlay thread drops to a slow
    /// poll instead of driving sixty frames a second into a picture that is
    /// not changing.
    pub fn tick(&mut self, dt: f32) -> bool {
        let enabled = self.config.read().notch.enabled;

        if !enabled {
            if self.window.is_some() {
                self.window = None;
            }
            self.create_failed = false;
            // The hook thread outlives the window, so tell it there is nothing
            // on screen; otherwise a stale bound rect would keep swallowing
            // chords over empty desktop.
            hook::silence();
            return false;
        }

        if self.window.is_none() && !self.create_failed {
            let cfg = self.config.read();
            match NotchWindow::create(&cfg) {
                Ok(w) => self.window = Some(w),
                Err(e) => {
                    eprintln!("[notch] failed to create window: {e:?}");
                    self.create_failed = true;
                }
            }
        }

        let Some(window) = self.window.as_mut() else {
            return false;
        };

        // The write lock is held for the frame because inline editing and slide
        // changes mutate the config in place. Contention is with the settings
        // window only, which touches it at human speed.
        let outcome = {
            let mut cfg = self.config.write();
            window.tick(&mut cfg, dt)
        };

        // Raising the Settings window happens here, with the lock released.
        //
        // `window.tick` runs while this thread holds the config write lock, and
        // the foreground calls in `tray::restore_settings_window` are
        // synchronous: they make the Settings window's thread service a message
        // before returning. That thread takes the same config write lock at the
        // top of its own update, so calling them under the lock closed a cycle
        // -- the overlay thread waiting on the Settings thread, the Settings
        // thread waiting on the lock the overlay thread held -- and the window
        // hung as "Not Responding" until the app was killed.
        if outcome.open_settings {
            crate::tray::restore_settings_window();
        }

        if outcome.config_dirty {
            self.save_countdown = Some(SAVE_DEBOUNCE);
        }

        if let Some(remaining) = self.save_countdown.as_mut() {
            *remaining -= dt;
            if *remaining <= 0.0 {
                self.save_countdown = None;
                self.config.read().save();
            }
        }

        outcome.animating
    }
}
