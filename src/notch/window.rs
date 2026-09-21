//! The notch's layered Win32 window.
//!
//! Two things here are worth knowing:
//!
//! * The window is permanently sized to the *expanded* footprint plus a margin
//!   for the shadow, and never resized. Only the painted silhouette animates.
//!   Resizing a layered window every frame would mean rebuilding the DIB and
//!   the Direct2D target sixty times a second; this way the expensive objects
//!   are created once. `WM_NCHITTEST` reports `HTTRANSPARENT` for every pixel
//!   outside the silhouette, so the empty margin stays click-through.
//!
//! * A frame is only painted when it would differ from the one already on
//!   screen. The notch spends nearly all of its life collapsed and unchanged,
//!   and a layered window costs a full Direct2D scene plus a
//!   `UpdateLayeredWindow` blit every time it is redrawn — so repainting it
//!   sixty times a second to show the same pill is the single most expensive
//!   thing the process can do while idle. See [`FrameKey`].
//!
//! * Wheel events are captured with a low-level mouse hook rather than
//!   `WM_MOUSEWHEEL`. The notch is a `WS_EX_NOACTIVATE` window and never takes
//!   focus, so it would only receive wheel messages if the user happens to have
//!   "scroll inactive windows on hover" enabled. The hook makes scroll-to-
//!   change-slide work unconditionally, and only swallows the event while the
//!   cursor is genuinely inside the open panel.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetDC, GetMonitorInfoW, ReleaseDC, AC_SRC_ALPHA, AC_SRC_OVER,
    BLENDFUNCTION, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_BACK, VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, RegisterClassW,
    SetForegroundWindow, SetWindowLongW, SetWindowPos, ShowWindow, UpdateLayeredWindow,
    GWL_EXSTYLE, HMENU, HWND_NOTOPMOST, HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_CHAR, WM_KEYDOWN, WM_KILLFOCUS,
    WM_LBUTTONDOWN, WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::config::{AppConfig, NotchAlign, NotchTheme, SlideKind};
use crate::notch::anim::lerp;
use crate::notch::backdrop;
use crate::notch::geom::NotchShape;
use crate::notch::hook;
use crate::notch::paint::{clock_key, Motion, Painter};
use crate::notch::state::NotchState;
use crate::notch::surface::D2DSurface;
use crate::notch::theme;

const CLASS_NAME: &str = "VenuNotchClass";
const HTTRANSPARENT: LRESULT = LRESULT(-1);
const HTCLIENT: LRESULT = LRESULT(1);

/// Extra slop around the drawn silhouette that still counts as a hover, so the
/// notch opens as the cursor arrives rather than after it has landed.
const HOVER_SLOP: f32 = 4.0;

/// How often a frame whose only motion is ambient — a breathing indicator
/// dot, or the desktop showing through a glass theme — is repainted.
///
/// A pulse takes the best part of four seconds to breathe in and out, so
/// about ten frames a second is some forty steps per cycle: indistinguishable
/// from sixty, at a sixth of the cost. The overlay thread polls faster than
/// this while idle, so the real cadence is the next poll after the interval
/// rather than the interval exactly.
const AMBIENT_FRAME: Duration = Duration::from_millis(90);

/// How often the monitor layout is re-read.
///
/// `EnumDisplayMonitors` is a syscall per display and a monitor arriving or
/// leaving is not a per-frame event — but it does have to be picked up
/// without restarting Venu, so it is polled rather than cached forever.
const MONITOR_REFRESH: Duration = Duration::from_secs(2);

/// How often the notch re-asserts its place in the z-order while it is idle.
///
/// `WS_EX_TOPMOST` keeps it in the topmost band, but another topmost window
/// that activates still lands above it. Painting used to re-assert the order
/// as a side effect sixty times a second; now that idle frames are skipped,
/// this is what keeps the notch from being left buried — at a cost of one
/// `SetWindowPos` a second instead of sixty.
const Z_ORDER_REFRESH: Duration = Duration::from_secs(1);

/// Longest the notch will go without repainting, however still it is.
///
/// [`FrameKey`] is meant to be exhaustive, but it is a list that has to be
/// kept in step with the painter by hand, and there are ways for a layered
/// window's contents to be dropped from underneath it that nothing in this
/// process sees — a lost Direct2D device, a session reconnect. A frame every
/// few seconds is close enough to free that it is worth spending to make any
/// of that self-correcting rather than permanent.
const KEEPALIVE_FRAME: Duration = Duration::from_secs(5);

static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Input collected by the window procedure and the mouse hook, drained once per
/// frame by [`NotchWindow::tick`].
///
/// Thread-local rather than a global: the window, its message pump, and the
/// hook all live on the single overlay thread, so this needs no locking and
/// cannot be touched from the egui thread by accident.
#[derive(Default)]
struct InputBus {
    click: Option<(f32, f32)>,
    chars: Vec<char>,
    commit: bool,
    cancel: bool,
    backspace: bool,
    lost_focus: bool,
    /// Published every frame so hit-testing matches what is on screen *now*.
    shape: Option<NotchShape>,
    /// Layout origin in screen coordinates, so `WM_NCHITTEST` can map the
    /// screen point it is given back into the shape's own space.
    origin: (i32, i32),
    /// Where the window's top-left sits inside the layout frame. The window is
    /// trimmed to what is actually drawn, so client coordinates — which is what
    /// `WM_LBUTTONDOWN` reports — are offset from the shape's own space by
    /// this much.
    client_offset: (f32, f32),
}

thread_local! {
    static INPUT: RefCell<InputBus> = RefCell::new(InputBus::default());
}

fn with_input<R>(f: impl FnOnce(&mut InputBus) -> R) -> Option<R> {
    // `try_with` + `try_borrow_mut` because the window procedure can be
    // re-entered from inside SetWindowPos while `tick` holds the borrow.
    INPUT
        .try_with(|cell| cell.try_borrow_mut().ok().map(|mut bus| f(&mut bus)))
        .ok()
        .flatten()
}

// ---------------------------------------------------------------------------
// Monitors
// ---------------------------------------------------------------------------

/// Work with full monitor rectangles, not work areas: the notch is meant to
/// sit against the physical top edge, over the top of maximised windows.
pub fn monitor_rects() -> Vec<RECT> {
    let mut rects: Vec<RECT> = Vec::new();

    unsafe extern "system" fn cb(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> windows::Win32::Foundation::BOOL {
        let rects = &mut *(data.0 as *mut Vec<RECT>);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            rects.push(info.rcMonitor);
        }
        true.into()
    }

    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(cb),
            LPARAM(&mut rects as *mut Vec<RECT> as isize),
        );
    }

    if rects.is_empty() {
        unsafe {
            rects.push(RECT {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            });
        }
    }

    rects
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

/// Where the window sits on screen and where the slab sits inside it.
/// Resolve one override against the value it falls back to. Zero means
/// "inherit", which is what every per-slide size field stores when untouched.
fn override_or(value: u32, base: f32, floor: f32) -> f32 {
    if value > 0 {
        (value as f32).max(floor)
    } else {
        base
    }
}

/// Collapsed size a single slide wants, in pixels.
///
/// The moving-text slide can ask for its own: a scrolling line needs width to
/// be readable in a way a clock does not. Because this is blended across the
/// carousel, scrolling between slides while the notch is open resizes it
/// smoothly rather than snapping when it finally closes.
fn slide_collapsed_size(cfg: &AppConfig, slide: SlideKind) -> (f32, f32) {
    let base_w = cfg.notch.collapsed_width as f32;
    let base_h = cfg.notch.collapsed_height as f32;

    match slide {
        SlideKind::Marquee => (
            override_or(cfg.marquee.collapsed_width, base_w, 40.0),
            override_or(cfg.marquee.collapsed_height, base_h, 16.0),
        ),
        _ => (base_w, base_h),
    }
}

/// Expanded size a single slide wants, in pixels.
///
/// A photo wants a different aspect ratio than a line of text does, and a long
/// line of text wants a different one again. `0` means "inherit". The floor is
/// the slide's own collapsed size, so an override can never ask the panel to
/// open smaller than the pill it grew out of.
fn slide_expanded_size(cfg: &AppConfig, slide: SlideKind) -> (f32, f32) {
    let (min_w, min_h) = slide_collapsed_size(cfg, slide);
    let base_w = (cfg.notch.expanded_width as f32).max(min_w);
    let base_h = (cfg.notch.expanded_height as f32).max(min_h);

    match slide {
        SlideKind::Wallpaper => (
            override_or(cfg.wallpaper.panel_width, base_w, min_w),
            override_or(cfg.wallpaper.panel_height, base_h, min_h),
        ),
        SlideKind::Marquee => (
            override_or(cfg.marquee.panel_width, base_w, min_w),
            override_or(cfg.marquee.panel_height, base_h, min_h),
        ),
        _ => (base_w, base_h),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Placement {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    /// Centre of the slab, in window-local coordinates.
    center_x: f32,
    top: f32,
}

/// Everything a painted frame is made of.
///
/// Two ticks with equal keys would put the same pixels on screen, so the
/// second one can skip the paint, the blit and all the Direct2D and
/// DirectWrite work behind them. Anything a slide reads has to be represented
/// here — leaving something out means the notch would go on showing a stale
/// version of it until something else happened to change.
///
/// The one thing deliberately absent is the free-running clock behind the
/// ambient pulses and the notification glow. That is reported by the painter
/// as [`Motion`] instead, so a slide can animate without the gate having to
/// know which slides do.
#[derive(PartialEq)]
struct FrameKey {
    placement: Placement,
    shape: NotchShape,
    expand: f32,
    carousel: f32,
    toast: f32,
    active: usize,
    pinned: bool,
    editing: bool,
    caret_on: bool,
    edit_buffer: String,
    marquee_offset: f32,
    click_through_flash: f32,
    selected_notification: Option<u64>,
    capture_excluded: bool,
    /// Local time, to the second.
    clock: u64,
    /// Bumped by the media poller when what is playing actually changes.
    media: u64,
    /// Bumped by the notification centre on every change to it.
    notifications: u64,
}

/// What one frame of the notch asked for.
pub struct TickOutcome {
    /// Something the user changed in the notch itself needs writing to disk.
    pub config_dirty: bool,
    /// The notch is mid-motion and wants the next frame at full rate. When
    /// this is false the overlay thread drops to a slow poll that watches for
    /// hover and input without drawing anything.
    pub animating: bool,
}

pub struct NotchWindow {
    hwnd: HWND,
    surface: D2DSurface,
    painter: Painter,
    pub state: NotchState,
    placement: Placement,
    /// Mirrors the ex-style currently on the window so we only call
    /// `SetWindowLongW` when something actually changed.
    ex_style: u32,
    /// Whether the window is currently hidden from screen capture. Only the
    /// frosted theme needs it, and it is a visible trade-off for the user, so
    /// it is turned back off the moment they leave that theme.
    capture_excluded: bool,
    last_wallpaper: String,
    /// Monitor rectangles, re-read every [`MONITOR_REFRESH`] rather than on
    /// every frame.
    monitors: Vec<RECT>,
    monitors_read_at: Instant,
    /// The frame currently on screen, and what it asked for. `None` until the
    /// first paint, which is why the notch always draws once on creation.
    last_key: Option<FrameKey>,
    last_cfg: Option<AppConfig>,
    last_motion: Motion,
    last_paint: Instant,
    z_asserted_at: Instant,
}

unsafe impl Send for NotchWindow {}

impl NotchWindow {
    fn register_class() -> windows::core::Result<()> {
        if CLASS_REGISTERED.load(Ordering::SeqCst) {
            return Ok(());
        }
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let name = HSTRING::from(CLASS_NAME);
            let class = WNDCLASSW {
                hInstance: instance.into(),
                lpszClassName: PCWSTR(name.as_ptr()),
                lpfnWndProc: Some(Self::wnd_proc),
                ..Default::default()
            };
            RegisterClassW(&class);
            CLASS_REGISTERED.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    pub fn create(cfg: &AppConfig) -> windows::core::Result<Self> {
        Self::register_class()?;

        let monitors = monitor_rects();
        let placement = Self::compute_placement(cfg, &monitors);
        let ex_style = Self::ex_style_for(cfg, false);

        let hwnd = unsafe {
            let name = HSTRING::from(CLASS_NAME);
            let title = HSTRING::from("Venu Notch");
            CreateWindowExW(
                windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(ex_style),
                PCWSTR(name.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                placement.x,
                placement.y,
                placement.w,
                placement.h,
                None,
                HMENU::default(),
                GetModuleHandleW(None)?,
                None,
            )?
        };

        let surface = D2DSurface::new()?;
        let painter = Painter::new()?;

        // Runs on its own thread; see the module docs in `hook.rs` for why.
        hook::install();

        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        Ok(Self {
            hwnd,
            surface,
            painter,
            state: NotchState::new(cfg),
            placement,
            ex_style,
            capture_excluded: false,
            last_wallpaper: cfg.wallpaper.path.clone(),
            monitors,
            monitors_read_at: Instant::now(),
            last_key: None,
            last_cfg: None,
            last_motion: Motion::Still,
            last_paint: Instant::now(),
            z_asserted_at: Instant::now(),
        })
    }

    fn ex_style_for(cfg: &AppConfig, editing: bool) -> u32 {
        let mut style = WS_EX_LAYERED | WS_EX_TOOLWINDOW;
        if cfg.notch.always_on_top {
            style |= WS_EX_TOPMOST;
        }
        // The window refuses activation except while the user is typing into
        // it, which is the only time it needs the keyboard.
        if !editing {
            style |= WS_EX_NOACTIVATE;
        }
        // Click-through drops the window out of hit-testing entirely, so
        // clicks land on whatever is behind it. Hover still works — that is
        // read from the cursor position, not from mouse messages — and so does
        // the wheel, which arrives through the low-level hook. Editing needs
        // clicks back, so the flag is suspended for as long as that lasts.
        if cfg.notch.click_through && !editing {
            style |= WS_EX_TRANSPARENT;
        }
        style.0
    }

    /// Re-read the monitor layout if it has been long enough, so a display
    /// being plugged in or unplugged is picked up without a restart.
    fn refresh_monitors(&mut self) {
        if self.monitors_read_at.elapsed() >= MONITOR_REFRESH {
            self.monitors = monitor_rects();
            self.monitors_read_at = Instant::now();
        }
    }

    fn compute_placement(cfg: &AppConfig, monitors: &[RECT]) -> Placement {
        let m = monitors
            .get(cfg.notch.monitor_index)
            .copied()
            .unwrap_or(monitors[0]);

        let mon_w = (m.right - m.left).max(1);
        let pad = theme::SHADOW_PAD;

        // The *layout* frame is the largest footprint any slide can ask for, so
        // the DIB and the Direct2D target are built once. The window itself is
        // trimmed to whatever is actually drawn — see `visible_rect`.
        let mut expanded_w = 0.0f32;
        let mut expanded_h = 0.0f32;
        for &slide in cfg.notch.effective_slides() {
            let (w, h) = slide_expanded_size(cfg, slide);
            let (cw, ch) = slide_collapsed_size(cfg, slide);
            expanded_w = expanded_w.max(w).max(cw);
            expanded_h = expanded_h.max(h).max(ch);
        }
        let expanded_w = expanded_w.ceil() as i32;
        let expanded_h = expanded_h.ceil() as i32;

        let w = expanded_w + pad * 2;
        let h = expanded_h + pad * 2;

        let anchor_x = match cfg.notch.align {
            NotchAlign::Left => m.left + expanded_w / 2,
            NotchAlign::Center => m.left + mon_w / 2,
            NotchAlign::Right => m.right - expanded_w / 2,
        } + cfg.notch.offset_x;

        let slab_top = m.top + cfg.notch.offset_y.max(0);

        Placement {
            x: anchor_x - w / 2,
            y: slab_top - pad,
            w,
            h,
            center_x: w as f32 / 2.0,
            top: pad as f32,
        }
    }

    /// Footprint for the slide currently under the carousel, blended across the
    /// two it sits between so per-slide sizes animate with the slide rather
    /// than snapping when the spring settles.
    fn blended_size(
        &self,
        cfg: &AppConfig,
        of: fn(&AppConfig, SlideKind) -> (f32, f32),
    ) -> (f32, f32) {
        let slides = cfg.notch.effective_slides();
        let last = slides.len().saturating_sub(1) as f32;
        let pos = self.state.carousel.value.clamp(0.0, last);

        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(slides.len() - 1);
        let f = pos - i0 as f32;

        let (w0, h0) = of(cfg, slides[i0]);
        let (w1, h1) = of(cfg, slides[i1]);

        (lerp(w0, w1, f), lerp(h0, h1, f))
    }

    /// The silhouette for the current expansion, in window-local coordinates.
    fn current_shape(&self, cfg: &AppConfig) -> NotchShape {
        let t = self.state.expand.value.clamp(0.0, 1.0);

        let (open_w, open_h) = self.blended_size(cfg, slide_expanded_size);
        let (shut_w, shut_h) = self.blended_size(cfg, slide_collapsed_size);

        let tp = self.state.toast_progress.value.clamp(0.0, 1.0);
        let toast_w = 380.0;
        let toast_h = 44.0;
        let base_w = lerp(shut_w, toast_w, tp);
        let base_h = lerp(shut_h, toast_h, tp);

        let w = lerp(base_w, open_w, t);
        let h = lerp(base_h, open_h, t);

        let collapsed_radius = (base_h * 0.5).min(20.0);
        let radius_bottom = lerp(collapsed_radius, theme::RADIUS_EXPANDED, t);

        // Fused to the bezel: square top with concave shoulders. Detached or
        // opening: the shoulders retract and the top corners round off.
        let attached = cfg.notch.offset_y <= 0;
        let flare = if attached {
            theme::FLARE * (1.0 - t).powf(1.6)
        } else {
            0.0
        };
        let radius_top = if attached {
            lerp(0.0, theme::RADIUS_EXPANDED, t)
        } else {
            radius_bottom
        };

        NotchShape {
            left: self.placement.center_x - w * 0.5,
            top: self.placement.top,
            right: self.placement.center_x + w * 0.5,
            bottom: self.placement.top + h,
            radius_top,
            radius_bottom,
            flare,
        }
    }

    /// The part of the layout frame the painter actually touches: the
    /// silhouette plus the reach of its shadow, in window-local pixels.
    ///
    /// The window is trimmed to this every frame. A layered window occupies
    /// its whole rectangle as far as the compositor is concerned even where
    /// it is fully transparent, and under `WDA_EXCLUDEFROMCAPTURE` that whole
    /// rectangle is what screen capture blacks out — so a window sized to the
    /// largest panel any slide could open put a black slab the size of the
    /// open panel into every screenshot, even while the notch sat collapsed.
    fn visible_rect(&self, shape: &NotchShape) -> RECT {
        // +2 for the rounding either side of the fractional shape edges, with safety margin for neon bloom.
        let reach = (theme::shadow_spread(self.state.expand.value) * 1.25 + 2.0).max(18.0);

        let left = (shape.left - reach).floor().max(0.0) as i32;
        let top = (shape.top - reach).floor().max(0.0) as i32;
        let right = (shape.right + reach).ceil().min(self.placement.w as f32) as i32;
        let bottom = (shape.bottom + reach).ceil().min(self.placement.h as f32) as i32;

        RECT {
            left,
            top,
            right: right.max(left + 1),
            bottom: bottom.max(top + 1),
        }
    }

    fn cursor_in_window(&self) -> Option<(f32, f32)> {
        let mut pt = POINT::default();
        unsafe {
            if windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt).is_err() {
                return None;
            }
        }
        Some((
            (pt.x - self.placement.x) as f32,
            (pt.y - self.placement.y) as f32,
        ))
    }

    /// One frame: reposition if settings moved, sample input, advance the
    /// springs, and paint — the last of those only if the result would
    /// actually differ from what is already on screen.
    pub fn tick(&mut self, cfg: &mut AppConfig, dt: f32) -> TickOutcome {
        let mut config_dirty = false;

        // -- geometry -------------------------------------------------------
        // No `SetWindowPos` here: `UpdateLayeredWindow` moves and resizes the
        // window on every frame anyway, and doing it twice would flash the
        // untrimmed frame for one frame whenever a setting changed.
        self.refresh_monitors();
        self.placement = Self::compute_placement(cfg, &self.monitors);

        if self.last_wallpaper != cfg.wallpaper.path {
            self.last_wallpaper = cfg.wallpaper.path.clone();
            self.painter.invalidate_image();
        }

        // Themes with live backdrop blur sample the desktop behind the notch. If the notch
        // were part of that sample it would re-blur its own last frame every
        // frame and smear into feedback within a second, so it takes itself
        // out of screen capture for as long as a glass blur theme is selected.
        let wants_exclusion = matches!(
            cfg.notch.theme,
            NotchTheme::Frosted | NotchTheme::Blurred | NotchTheme::Acrylic
        );
        if wants_exclusion != self.capture_excluded {
            let applied = if wants_exclusion {
                backdrop::exclude_from_capture(self.hwnd)
            } else {
                backdrop::include_in_capture(self.hwnd)
            };
            if applied {
                self.capture_excluded = wants_exclusion;
            } else if wants_exclusion {
                eprintln!(
                    "[notch] cannot hide the notch from screen capture; \
                     the glass theme will show its tint without the blur"
                );
                // Leave the flag false: the painter checks it before sampling.
                self.capture_excluded = false;
            }
        }

        self.state.clamp_active(cfg);

        // -- input ----------------------------------------------------------
        let shape = self.current_shape(cfg);
        let hit_shape = shape.inset(-HOVER_SLOP);

        let hovered = self
            .cursor_in_window()
            .map(|(x, y)| hit_shape.contains(x, y))
            .unwrap_or(false);

        let wheel = hook::take_wheel();

        // Left and right together over the notch flips click-through. It is
        // deliberately the same gesture both ways round: once click-through is
        // on, an ordinary click cannot reach the notch to turn it off again.
        if hook::take_chord() {
            cfg.notch.click_through = !cfg.notch.click_through;
            self.state.flash_click_through();
            config_dirty = true;
        }
        self.apply_ex_style(cfg, self.state.editing);

        let drained = with_input(|bus| {
            let click = bus.click.take();
            let chars = std::mem::take(&mut bus.chars);
            let commit = std::mem::replace(&mut bus.commit, false);
            let cancel = std::mem::replace(&mut bus.cancel, false);
            let backspace = std::mem::replace(&mut bus.backspace, false);
            let lost_focus = std::mem::replace(&mut bus.lost_focus, false);
            (click, chars, commit, cancel, backspace, lost_focus)
        })
        .unwrap_or((None, Vec::new(), false, false, false, false));

        let (click, chars, commit, cancel, backspace, lost_focus) = drained;

        if cfg.notch.scroll_to_switch && wheel != 0 && self.state.is_open() {
            // Wheel down walks forward through the deck, matching the way a
            // scroll gesture moves content upward.
            self.state.step_slide(-wheel.signum(), cfg);
        }

        if let Some((cx, cy)) = click {
            self.handle_click(cfg, cx, cy, shape);
        }

        if self.state.editing {
            if cancel || lost_focus {
                self.state.cancel_edit();
                self.set_editing_style(cfg, false);
            } else if commit {
                if self.state.commit_edit(cfg) {
                    config_dirty = true;
                }
                self.set_editing_style(cfg, false);
            } else {
                if backspace {
                    self.state.backspace();
                }
                for ch in chars {
                    self.state.type_char(ch);
                }
            }
        }

        // -- animation ------------------------------------------------------
        let delay = Duration::from_millis(cfg.notch.collapse_delay_ms as u64);
        let target = self.state.set_hovered(hovered, delay);
        self.state.tick(cfg, target, dt);

        // Advance notifications and dynamic toast spring
        let notif_store = crate::notch::notify::global_store();
        notif_store.write().tick(dt);
        let has_toast = notif_store.read().active_toast.is_some();
        let toast_target = if has_toast && !self.state.is_open() {
            1.0
        } else {
            0.0
        };
        self.state.toast_progress.step(toast_target, dt);

        // Reset to default collapsed slide on collapse
        if target == 0.0 && self.state.expand.value <= 0.05 {
            let media_playing = self.painter.media.snapshot().playing;
            let unread = notif_store.read().unread_count() > 0;
            self.state
                .apply_default_collapsed(cfg, media_playing, unread);
        }

        // Persist the resting slide so the notch comes back where it was.
        if self.state.dirty {
            cfg.notch.active_slide = self.state.active;
            self.state.dirty = false;
            config_dirty = true;
        }

        // Publish the freshly-animated shape for hit-testing and the hook.
        let shape = self.current_shape(cfg);
        let open = self.state.is_open();
        let (wx, wy) = (self.placement.x, self.placement.y);
        let visible = self.visible_rect(&shape);
        with_input(|bus| {
            bus.shape = Some(shape.inset(-HOVER_SLOP));
            bus.origin = (wx, wy);
            bus.client_offset = (visible.left as f32, visible.top as f32);
        });
        hook::publish(
            open && cfg.notch.scroll_to_switch,
            // The chord is watched for whether the notch is open or shut, so
            // it can be turned back off from the collapsed pill.
            true,
            wx + shape.left as i32,
            wy + shape.top as i32,
            wx + shape.right as i32,
            wy + shape.bottom as i32,
        );

        // -- paint ----------------------------------------------------------
        //
        // Everything above is cheap: a cursor read, some arithmetic and a few
        // atomics. The paint below is not, so it is only spent when the frame
        // would come out different — or when the last one it drew said it was
        // still moving under its own clock.
        let key = self.frame_key(shape);
        let changed = self.last_key.as_ref() != Some(&key) || self.last_cfg.as_ref() != Some(cfg);

        let since_paint = self.last_paint.elapsed();
        let due = match self.last_motion {
            Motion::Continuous => true,
            Motion::Ambient => since_paint >= AMBIENT_FRAME,
            Motion::Still => since_paint >= KEEPALIVE_FRAME,
        };

        if changed || due {
            if let Err(e) = self.render(cfg, shape) {
                eprintln!("[notch] render failed: {e:?}");
            }
            self.last_motion = self.painter.take_motion();
            self.last_paint = Instant::now();
            self.last_key = Some(key);
            if self.last_cfg.as_ref() != Some(cfg) {
                self.last_cfg = Some(cfg.clone());
            }
        } else if self.z_asserted_at.elapsed() >= Z_ORDER_REFRESH {
            // Painting re-asserts the z-order on its way past; an idle notch
            // still has to, or a topmost window that activated over it would
            // keep it covered indefinitely.
            self.assert_z_order(cfg);
        }

        TickOutcome {
            config_dirty,
            // A frame that differed from the one before it means something is
            // still in flight — a spring, the marquee, a countdown — and the
            // next one should not be kept waiting.
            animating: changed || self.last_motion == Motion::Continuous,
        }
    }

    /// Snapshot of everything that would change what the next frame looks
    /// like. See [`FrameKey`].
    fn frame_key(&self, shape: NotchShape) -> FrameKey {
        FrameKey {
            placement: self.placement,
            shape,
            expand: self.state.expand.value,
            carousel: self.state.carousel.value,
            toast: self.state.toast_progress.value,
            active: self.state.active,
            pinned: self.state.pinned,
            editing: self.state.editing,
            caret_on: self.state.caret_on,
            // Empty while not editing, and an empty `String` does not
            // allocate, so the idle path stays free of heap traffic.
            edit_buffer: self.state.edit_buffer.clone(),
            marquee_offset: self.state.marquee_offset,
            click_through_flash: self.state.click_through_flash,
            selected_notification: self.state.selected_notification_id,
            capture_excluded: self.capture_excluded,
            clock: clock_key(),
            media: self.painter.media.revision(),
            notifications: crate::notch::notify::global_store().read().revision(),
        }
    }

    fn handle_click(&mut self, cfg: &AppConfig, cx: f32, cy: f32, shape: NotchShape) {
        if !self.state.is_open() {
            // Clicked on collapsed alert toast -> expand into notifications slide and show detailed view!
            let notif_store = crate::notch::notify::global_store();
            let toast_opt = notif_store.read().active_toast.clone();
            if let Some(toast) = toast_opt {
                let slides = cfg.notch.effective_slides();
                if let Some(idx) = slides
                    .iter()
                    .position(|s| *s == crate::config::SlideKind::Notifications)
                {
                    self.state.active = idx;
                }
                self.state.selected_notification_id = Some(toast.notification.id);
                self.state.pinned = true;
                notif_store.write().dismiss_toast();
                self.state.expand.set(1.0);
            }
            return;
        }

        // Settings launcher button: opens the Settings panel
        let (sx, sy, sr) = shape.settings_button();
        let hit_s = sr + 5.0;
        if (cx - sx) * (cx - sx) + (cy - sy) * (cy - sy) <= hit_s * hit_s {
            crate::tray::restore_settings_window();
            return;
        }

        // Pin toggle lives above whatever slide is showing, so it is checked
        // before any per-slide hit-testing below.
        let (px, py, pr) = shape.pin_button();
        let hit_p = pr + 5.0; // a little slop past the drawn ring
        if (cx - px) * (cx - px) + (cy - py) * (cy - py) <= hit_p * hit_p {
            self.state.toggle_pinned();
            return;
        }

        let slides = cfg.notch.effective_slides();
        let Some(slide) = slides.get(self.state.active) else {
            return;
        };

        if *slide == crate::config::SlideKind::Notifications {
            let body = crate::notch::geom::slide_body(shape);
            let notif_store = crate::notch::notify::global_store();

            // If a notification is currently expanded into detailed view:
            if let Some(sel_id) = self.state.selected_notification_id {
                let back = crate::notch::geom::notification_back_button(body);
                if cx >= back.left && cx <= back.right && cy >= back.top && cy <= back.bottom {
                    self.state.selected_notification_id = None;
                    return;
                }

                let dismiss = crate::notch::geom::notification_dismiss_button(body);
                if cx >= dismiss.left
                    && cx <= dismiss.right
                    && cy >= dismiss.top
                    && cy <= dismiss.bottom
                {
                    notif_store.write().dismiss_item(sel_id);
                    self.state.selected_notification_id = None;
                    return;
                }
                return;
            }

            // Normal list view:
            let clr = crate::notch::geom::notification_clear_button(body);
            if cx >= clr.left && cx <= clr.right && cy >= clr.top && cy <= clr.bottom {
                notif_store.write().clear_all();
                return;
            }

            // Hit test individual notification card rows to open detailed view:
            let store = notif_store.read();
            let max_items = 3.min(store.items.len());
            for i in 0..max_items {
                let r = crate::notch::geom::notification_item_rect(body, i);
                if cx >= r.left && cx <= r.right && cy >= r.top && cy <= r.bottom {
                    if let Some(item) = store.items.get(i) {
                        self.state.selected_notification_id = Some(item.id);
                        return;
                    }
                }
            }
            return;
        }

        // Transport buttons live along the bottom of the Now Playing slide;
        // same squared-distance-plus-slop hit test as the pin button above.
        if *slide == crate::config::SlideKind::Media {
            let body = crate::notch::geom::slide_body(shape);
            let buttons = crate::notch::geom::media_transport_buttons(body);
            for (i, (bx, by, br)) in buttons.into_iter().enumerate() {
                let hit = br + 5.0;
                if (cx - bx) * (cx - bx) + (cy - by) * (cy - by) <= hit * hit {
                    match i {
                        0 => self.painter.media.previous(),
                        1 => self.painter.media.play_pause(),
                        2 => self.painter.media.next(),
                        _ => {}
                    }
                    break;
                }
            }
            return;
        }

        if *slide != crate::config::SlideKind::Status {
            return;
        }

        // Only the left half of the split view is the editable focus line.
        let split =
            shape.left + theme::GUTTER + (shape.width() - theme::GUTTER * 2.0) * theme::SPLIT_RATIO;
        if cx < split && cy > shape.top {
            self.state.begin_edit(cfg);
            self.set_editing_style(cfg, true);
        }
    }

    /// Push the extended style if it has drifted from what the settings now
    /// say. Called every frame because click-through can be switched from the
    /// settings window, which the notch has no other notification of.
    fn apply_ex_style(&mut self, cfg: &AppConfig, editing: bool) {
        let style = Self::ex_style_for(cfg, editing);
        if style != self.ex_style {
            self.ex_style = style;
            unsafe {
                SetWindowLongW(self.hwnd, GWL_EXSTYLE, style as i32);
            }
        }
    }

    /// Editing needs real keyboard focus, which a `WS_EX_NOACTIVATE` window can
    /// never receive. Drop the flag for the duration and put it straight back.
    fn set_editing_style(&mut self, cfg: &AppConfig, editing: bool) {
        self.apply_ex_style(cfg, editing);
        if editing {
            unsafe {
                let _ = SetForegroundWindow(self.hwnd);
                let _ = SetFocus(self.hwnd);
            }
        }
    }

    /// Put the window back where it belongs in the z-order.
    fn assert_z_order(&mut self, cfg: &AppConfig) {
        let z = if cfg.notch.always_on_top {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                z,
                0,
                0,
                0,
                0,
                SWP_NOSIZE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | SWP_NOACTIVATE
                    | SWP_SHOWWINDOW,
            );
        }
        self.z_asserted_at = Instant::now();
    }

    fn render(&mut self, cfg: &AppConfig, shape: NotchShape) -> windows::core::Result<()> {
        self.assert_z_order(cfg);

        // The surface stays at the full layout size — it is the expensive
        // object, and painting always uses layout coordinates. Only the window
        // shrinks, by blitting the sub-rectangle that has anything in it.
        let w = self.placement.w.max(1) as u32;
        let h = self.placement.h.max(1) as u32;

        self.surface.ensure(w, h)?;
        self.painter.paint(
            &mut self.surface,
            cfg,
            &self.state,
            shape,
            (self.placement.x, self.placement.y),
            self.capture_excluded,
        )?;

        let mem_dc = self.surface.dc();
        if mem_dc.is_invalid() {
            return Ok(());
        }

        unsafe {
            let screen_dc = GetDC(None);
            let visible = self.visible_rect(&shape);
            let dst = POINT {
                x: self.placement.x + visible.left,
                y: self.placement.y + visible.top,
            };
            let size = SIZE {
                cx: visible.right - visible.left,
                cy: visible.bottom - visible.top,
            };
            let src = POINT {
                x: visible.left,
                y: visible.top,
            };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };

            if let Err(e) = UpdateLayeredWindow(
                self.hwnd,
                screen_dc,
                Some(&dst),
                Some(&size),
                mem_dc,
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            ) {
                eprintln!("[notch] UpdateLayeredWindow failed: {e:?}");
            }

            let _ = ReleaseDC(None, screen_dc);
        }

        Ok(())
    }

    // -- Win32 callbacks ----------------------------------------------------

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_NCHITTEST => {
                // Screen coordinates arrive packed in lparam.
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

                let inside = with_input(|bus| match bus.shape {
                    Some(shape) => {
                        let (ox, oy) = bus.origin;
                        shape.contains((x - ox) as f32, (y - oy) as f32)
                    }
                    None => false,
                })
                .unwrap_or(false);

                if inside {
                    HTCLIENT
                } else {
                    HTTRANSPARENT
                }
            }

            WM_LBUTTONDOWN => {
                let x = (lparam.0 & 0xFFFF) as i16 as f32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
                with_input(|bus| {
                    let (ox, oy) = bus.client_offset;
                    bus.click = Some((x + ox, y + oy));
                });
                LRESULT(0)
            }

            WM_CHAR => {
                if let Some(ch) = char::from_u32(wparam.0 as u32) {
                    if !ch.is_control() {
                        with_input(|bus| bus.chars.push(ch));
                    }
                }
                LRESULT(0)
            }

            WM_KEYDOWN => {
                let vk = wparam.0 as u16;
                with_input(|bus| {
                    if vk == VK_RETURN.0 {
                        bus.commit = true;
                    } else if vk == VK_ESCAPE.0 {
                        bus.cancel = true;
                    } else if vk == VK_BACK.0 {
                        bus.backspace = true;
                    }
                });
                LRESULT(0)
            }

            WM_KILLFOCUS => {
                // Clicking away while editing commits nothing and closes the
                // field, rather than leaving a half-typed line hanging.
                with_input(|bus| bus.lost_focus = true);
                LRESULT(0)
            }

            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

impl Drop for NotchWindow {
    fn drop(&mut self) {
        unsafe {
            // The wheel hook outlives individual windows on purpose: it is
            // installed once and parked on its own thread.
            if !self.hwnd.is_invalid() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}
