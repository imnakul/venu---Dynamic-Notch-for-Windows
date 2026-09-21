//! The FlashScreen overlay: every so often, for a few seconds, a line of text
//! or an image appears in the middle of the screen — then leaves on its own.
//!
//! Module layout:
//!
//! * [`renderer`] — the Direct2D painter and the animation math, no Win32
//! * [`window`]   — the full-monitor layered window a flash plays on
//!
//! The manager owns the clock. Between flashes the window is hidden and
//! nothing is rendered at all; a flash is *entry animation → hold → exit
//! animation*, with the hold and the animation lengths coming from the
//! config, and then the interval starts counting again.

pub mod renderer;
pub mod window;

use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::{AppConfig, FlashConfig};
use window::FlashWindow;

static PREVIEW_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Ask the overlay thread to play one flash right now. Works with the master
/// switch off: you should be able to see what you are building before you
/// commit to living with it.
pub fn request_preview() {
    PREVIEW_REQUESTED.store(true, Ordering::SeqCst);
}

/// One flash in flight: how far through its hold and exit it is, and which
/// turn of the rotations (texts, images, colours) it is showing.
struct Showing {
    elapsed: f32,
    turn: u32,
}

pub struct FlashManager {
    window: Option<FlashWindow>,
    showing: Option<Showing>,
    /// Seconds until the next flash. `None` while nothing is scheduled —
    /// disabled, or mid-flash.
    countdown: Option<f32>,
    /// The flash counter. Everything that rotates — messages, images, colours
    /// — indexes by this, so they all advance together, one step per flash.
    turn: u32,
}

impl FlashManager {
    pub fn new() -> Self {
        Self {
            window: None,
            showing: None,
            countdown: None,
            turn: 0,
        }
    }

    /// Advance the flash by one frame. Returns whether one is on screen: the
    /// flash animates, so while it is up the overlay thread has to keep
    /// running at full rate. Between flashes this is a countdown and nothing
    /// else.
    pub fn tick(&mut self, cfg: &AppConfig, dt: f32) -> bool {
        let flash = &cfg.flash;
        let preview = PREVIEW_REQUESTED.swap(false, Ordering::SeqCst);

        // With the switch off, nothing is scheduled and the window is torn
        // down — but only once nothing is showing: a flash already on screen
        // (usually a preview, which deliberately works with the switch off)
        // finishes its few seconds rather than being snatched away
        // mid-animation.
        if !flash.enabled && self.showing.is_none() {
            self.window = None;
            self.countdown = None;
            if !preview {
                return false;
            }
        }

        if preview && self.showing.is_none() {
            self.begin(flash);
        }

        if flash.enabled && self.showing.is_none() {
            match self.countdown {
                None => self.countdown = Some(flash.safe_interval()),
                Some(ref mut c) => {
                    // Shortening the interval should take effect within the
                    // new interval, not the old one.
                    if *c > flash.safe_interval() {
                        *c = flash.safe_interval();
                    }
                    *c -= dt;
                    if *c <= 0.0 {
                        self.begin(flash);
                    }
                }
            }
        }

        let Some(show) = self.showing.as_mut() else {
            return false;
        };

        show.elapsed += dt;
        if show.elapsed >= flash.safe_duration() + flash.safe_anim_secs() {
            self.showing = None;
            self.countdown = if flash.enabled {
                Some(flash.safe_interval())
            } else {
                None
            };
            if let Some(w) = self.window.as_mut() {
                w.hide();
            }
            if !flash.enabled {
                self.window = None;
            }
            return false;
        }

        if let Some(w) = self.window.as_mut() {
            if let Err(e) = w.render(flash, show.elapsed, show.turn) {
                eprintln!("[flash] render error: {e:?}");
                self.window = None;
                self.showing = None;
                return false;
            }
        }

        true
    }

    fn begin(&mut self, flash: &FlashConfig) {
        self.countdown = None;

        if self.window.is_none() {
            match FlashWindow::create(flash) {
                Ok(w) => self.window = Some(w),
                Err(e) => {
                    // Creation is retried on the next flash, not every frame:
                    // starts are minutes apart, which is not a log-flood risk.
                    eprintln!("[flash] failed to create window: {e:?}");
                    return;
                }
            }
        }

        let turn = self.turn;
        self.turn = self.turn.wrapping_add(1);
        self.showing = Some(Showing { elapsed: 0.0, turn });

        if let Some(w) = self.window.as_mut() {
            w.sync_geometry(flash);
            w.show();
            if let Err(e) = w.render(flash, 0.0, turn) {
                eprintln!("[flash] render error: {e:?}");
                self.window = None;
                self.showing = None;
            }
        }
    }
}
