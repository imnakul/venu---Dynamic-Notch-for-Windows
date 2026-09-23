pub mod d2d_renderer;
pub mod win32_window;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::config::{AppConfig, EdgeSelection, PaddingConfig};
use d2d_renderer::Edge;
use win32_window::Win32OverlayWindow;

/// How often the strips are re-laid-out when nothing in the settings has
/// changed. Only the screen resolution can move them from underneath us, and
/// that is not a per-frame event — but it does have to be picked up without a
/// restart.
const RELAYOUT_INTERVAL: Duration = Duration::from_secs(2);

/// The settings that decide where the strips sit and how they behave. Compared
/// against the last applied copy so the layout is not rebuilt — four Win32
/// calls per strip — on every frame.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LayoutKey {
    enabled: bool,
    edges: EdgeSelection,
    padding: PaddingConfig,
    thickness: u32,
    always_on_top: bool,
    click_through: bool,
}

impl LayoutKey {
    fn of(config: &AppConfig) -> Self {
        Self {
            enabled: config.overlay_enabled,
            edges: config.edges,
            padding: config.padding,
            thickness: config.thickness,
            always_on_top: config.always_on_top,
            click_through: config.click_through,
        }
    }
}

pub struct OverlayManager {
    windows: HashMap<Edge, Win32OverlayWindow>,
    layout: Option<LayoutKey>,
    laid_out_at: Instant,
}

impl OverlayManager {
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
            layout: None,
            laid_out_at: Instant::now(),
        }
    }

    pub fn sync_windows(&mut self, config: &AppConfig) {
        // The master switch is folded in here rather than short-circuiting the
        // caller, so turning it off takes the existing strips down through the
        // same path that unticking every edge would.
        let on = config.overlay_enabled;
        let edges_state = [
            (Edge::Top, on && config.edges.top),
            (Edge::Bottom, on && config.edges.bottom),
            (Edge::Left, on && config.edges.left),
            (Edge::Right, on && config.edges.right),
        ];

        for (edge, enabled) in edges_state {
            if enabled {
                if let Some(win) = self.windows.get_mut(&edge) {
                    win.recalculate_geometry(config);
                } else {
                    match Win32OverlayWindow::create(edge, config) {
                        Ok(win) => {
                            eprintln!("[OverlayManager] Created window for {:?}", edge);
                            self.windows.insert(edge, win);
                        }
                        Err(e) => {
                            eprintln!(
                                "[OverlayManager] Failed to create window for {:?}: {:?}",
                                edge, e
                            );
                        }
                    }
                }
            } else if self.windows.remove(&edge).is_some() {
                eprintln!("[OverlayManager] Removed window for {:?}", edge);
            }
        }
    }

    /// Render one frame of every strip that is up. Returns whether any of them
    /// are: the strips scroll continuously, so while one is on screen the
    /// overlay thread has to keep running at full rate — but when none are,
    /// this costs nothing.
    pub fn render_tick(&mut self, config: &AppConfig, dt: f32) -> bool {
        // Laying the strips out means `SetWindowLongW` and `SetWindowPos` per
        // strip. Neither the settings nor the screen resolution change between
        // frames, so this runs on change and on a slow heartbeat instead.
        let layout = LayoutKey::of(config);
        if self.layout != Some(layout) || self.laid_out_at.elapsed() >= RELAYOUT_INTERVAL {
            self.sync_windows(config);
            self.layout = Some(layout);
            self.laid_out_at = Instant::now();
        }

        for (edge, win) in self.windows.iter_mut() {
            if let Err(e) = win.update_and_render(config, dt) {
                eprintln!("[OverlayManager] Render error for {:?}: {:?}", edge, e);
            }
        }

        !self.windows.is_empty()
    }
}
