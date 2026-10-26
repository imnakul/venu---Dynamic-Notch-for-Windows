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
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, SetFocus, UnregisterHotKey, HOT_KEY_MODIFIERS, VK_BACK, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::Shell::{
    DragAcceptFiles, DragFinish, DragQueryFileW, SHQueryUserNotificationState, HDROP, QUNS_BUSY,
    QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, RegisterClassW,
    SetForegroundWindow, SetWindowLongW, SetWindowPos, ShowWindow, UpdateLayeredWindow,
    GWL_EXSTYLE, HMENU, HWND_NOTOPMOST, HWND_TOPMOST, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE,
    SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_CHAR, WM_DROPFILES, WM_HOTKEY,
    WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_NCHITTEST, WM_RBUTTONUP, WNDCLASSW,
    WS_EX_ACCEPTFILES, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::config::{
    AppConfig, BorderEffectConfig, InputConfig, MediaConfig, NotchAlign, NotchTheme, SlideKind,
};
use crate::notch::anim::{lerp, smoothstep};
use crate::notch::audio::{self, AudioMeter};
use crate::notch::backdrop;
use crate::notch::geom::NotchShape;
use crate::notch::hook;
use crate::notch::layout::{self, PillPart, PodCandidates, PodKind};
use crate::notch::menu::{MenuCommand, MenuEntry, MenuRequest};
use crate::notch::paint::{clock_key, Motion, Painter, PodDraw, Scene, SegmentDraw};
use crate::notch::state::{Badge, NotchState};
use crate::notch::surface::D2DSurface;
use crate::notch::theme;

const CLASS_NAME: &str = "VenuNotchClass";

/// A Direct2D rectangle as the layout module's own.
fn layout_rect(r: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F) -> layout::Rect {
    layout::Rect::new(r.left, r.top, r.right, r.bottom)
}
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

/// How often pods re-check what is live (timer, media, calendar, agents).
///
/// Some of those readers allocate (the media title, the calendar's next
/// events), so they are polled twice a second rather than every tick. Half a
/// second late to show a pod is invisible; sixty reads a second is not free.
const POD_POLL: Duration = Duration::from_millis(500);

/// Faster than this, in pixels a second, the cursor is passing over the
/// pill on its way somewhere else, and hover does not start counting.
const FLYBY_SPEED: f32 = 1400.0;

/// "Pause hover" from the shortcut lasts until the shortcut is pressed again.
const PAUSE_UNTIL_RESUMED: Duration = Duration::from_secs(60 * 60 * 24 * 365);

/// "Pause hover" from the menu.
const PAUSE_FOR_AN_HOUR: Duration = Duration::from_secs(60 * 60);

/// How much the closed pill grows under the cursor, in pixels.
const PEEK_GROW_W: f32 = 10.0;
const PEEK_GROW_H: f32 = 2.0;

/// How often full-screen state is re-read for the hover rule.
const FULLSCREEN_REFRESH: Duration = Duration::from_secs(1);

/// Ids the shortcuts are registered under, matching
/// [`InputConfig::hotkeys`] order plus one.
const HOTKEY_TOGGLE: i32 = 1;
const HOTKEY_CLICK_THROUGH: i32 = 2;
const HOTKEY_PAUSE_HOVER: i32 = 3;

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
    /// Satellite pods, published alongside `shape` so clicks and hover land
    /// on them too.
    pods: [Option<NotchShape>; 2],
    /// Layout origin in screen coordinates, so `WM_NCHITTEST` can map the
    /// screen point it is given back into the shape's own space.
    origin: (i32, i32),
    /// Where the window's top-left sits inside the layout frame. The window is
    /// trimmed to what is actually drawn, so client coordinates — which is what
    /// `WM_LBUTTONDOWN` reports — are offset from the shape's own space by
    /// this much.
    client_offset: (f32, f32),
    /// A right-click on the notch, in layout coordinates.
    menu: Option<(f32, f32)>,
    /// Shortcut ids pressed since the last frame.
    hotkeys: Vec<i32>,
    /// Paths dropped onto the notch.
    dropped: Vec<String>,
}

/// Everything the input bus collected for one frame.
#[derive(Default)]
struct InputDrain {
    click: Option<(f32, f32)>,
    chars: Vec<char>,
    commit: bool,
    cancel: bool,
    backspace: bool,
    lost_focus: bool,
    menu: Option<(f32, f32)>,
    hotkeys: Vec<i32>,
    dropped: Vec<String>,
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
        // Three metrics need enough room to breathe even when the ordinary
        // collapsed pill is configured for a clock or a short status line.
        SlideKind::Stats => (base_w.max(260.0), base_h.max(30.0)),
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
        // Leave enough height for the overview heading, three values and the
        // power/memory footer, while still allowing the width to follow the
        // user's chosen notch size.
        SlideKind::Stats => (base_w.max(min_w), base_h.max(164.0)),
        // The player is centred and stacked: narrower and taller than the
        // other pages unless the user sized it themselves.
        SlideKind::Media => (
            override_or(
                cfg.notch.media.panel_width,
                MediaConfig::AUTO_WIDTH.max(min_w),
                min_w,
            ),
            override_or(
                cfg.notch.media.panel_height,
                MediaConfig::AUTO_HEIGHT.max(min_h),
                min_h,
            ),
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
    badge_time: f32,
    selected_notification: Option<u64>,
    capture_excluded: bool,
    /// Local time, to the second, while a Clock slide is on screen; zero
    /// otherwise.
    clock: u64,
    /// Bumped by the media poller when what is playing actually changes.
    media: u64,
    /// Bumped by the notification centre on every change to it.
    notifications: u64,
    /// Bumped by the `/usage` webhook.
    usage: u64,
    /// Seconds into the running border effect. `None` between runs, so an
    /// effect costs nothing until it plays.
    effect: Option<u32>,
    /// Sample revision only while the Stats slide is active, so system
    /// telemetry does not create background work for unrelated slides.
    stats: u64,
    /// The focus timer, to the second, while a Timer slide is on screen.
    timer: u64,
    /// Split segments and pods: what they show, where, how visible.
    scene: Scene,
    /// What each pod reads, at the granularity it draws: the timer and an
    /// agent's elapsed time to the second, the calendar to the minute.
    pod_content: [u64; 2],
    /// Feed revision and the minute, while a Calendar slide is on screen:
    /// "in 12 min" has to count down.
    calendar: (u64, i64),
    /// The switches' states, while the Quick Toggles slide is on screen.
    controls: crate::notch::toggles::States,
    /// Music wave bars, quantised; at rest unless music is playing.
    wave: [u8; layout::WAVE_BARS],
    /// Shelf and clipboard contents, while their slides are on screen.
    shelf: u64,
    clipboard: u64,
    /// The player's elapsed time to the second, while it is open.
    media_clock: u64,
    /// The short list's add row is being typed into.
    adding_item: bool,
}

/// What one frame of the notch asked for.
pub struct TickOutcome {
    /// Something the user changed in the notch itself needs writing to disk.
    pub config_dirty: bool,
    /// The notch is mid-motion and wants the next frame at full rate. When
    /// this is false the overlay thread drops to a slow poll that watches for
    /// hover and input without drawing anything.
    pub animating: bool,
    /// The Settings button was clicked. The overlay thread has to raise the
    /// Settings window itself once it has dropped the config write lock -- see
    /// `NotchManager::tick`.
    pub open_settings: bool,
    /// A right-click menu to show, once the config lock is released.
    pub menu: Option<MenuRequest>,
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
    /// Set by the Settings button, consumed at the end of `tick`. See
    /// `TickOutcome::open_settings`.
    open_settings: bool,
    /// What pods could show, re-read every [`POD_POLL`] while pods are on.
    pod_live: PodCandidates,
    pod_polled_at: Instant,
    /// Where the cursor came in when the panel opened from a pod. The pod
    /// merges into the notch as it opens, so without this a cursor resting
    /// where the pod was could fall outside the panel and close it again.
    hold_zone: Option<NotchShape>,
    /// Last frame's cursor, for its speed.
    last_cursor: Option<(f32, f32)>,
    /// A full-screen app, game or presentation is in front.
    fullscreen: bool,
    fullscreen_read_at: Instant,
    /// A right-click menu waiting to be shown.
    menu: Option<MenuRequest>,
    /// The shortcuts as currently registered with Windows.
    hotkeys: [Option<(u32, u32)>; 3],
    /// The live music wave.
    audio: AudioMeter,
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
            // Files dropped on the notch are pinned to the Shelf.
            DragAcceptFiles(hwnd, true);
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
            open_settings: false,
            pod_live: PodCandidates::default(),
            pod_polled_at: Instant::now() - POD_POLL,
            hold_zone: None,
            last_cursor: None,
            fullscreen: false,
            fullscreen_read_at: Instant::now() - FULLSCREEN_REFRESH,
            menu: None,
            hotkeys: [None; 3],
            audio: AudioMeter::default(),
        })
    }

    fn ex_style_for(cfg: &AppConfig, editing: bool) -> u32 {
        // Accept-files is part of the style, not just a call at creation:
        // every later style change rewrites the whole set, and leaving it
        // out would quietly switch dropping files on the notch off.
        let mut style = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_ACCEPTFILES;
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
        // The resting pill can be wider than any one slide asks for: split
        // segments add to it, and pods sit out beside it. Reserve that once,
        // so the frame never has to grow while they animate.
        let widest_pill = cfg
            .notch
            .effective_slides()
            .iter()
            .map(|&slide| slide_collapsed_size(cfg, slide).0)
            .fold(0.0f32, f32::max);
        let shoulder = cfg.notch.shape.shoulder_at(0.0);
        let pill_footprint = widest_pill
            + layout::max_split_extra(&cfg.notch.split)
            + 2.0 * layout::max_pod_reach(&cfg.notch.pods, shoulder);
        expanded_w = expanded_w.max(pill_footprint);

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

        // Split segments widen the resting pill either side of its face, and
        // the panel never opens narrower than the pill it grows out of.
        let (split_l, split_r) = self.state.split_widths();
        let shut_w = shut_w + split_l + split_r;
        let open_w = open_w.max(shut_w);

        let tp = self.state.toast_progress.value.clamp(0.0, 1.0);
        let toast_w = 380.0;
        let toast_h = 44.0;
        let base_w = lerp(shut_w, toast_w, tp);
        let base_h = lerp(shut_h, toast_h, tp);

        // The closed pill grows a touch under the cursor (see `peek`).
        let peek = self.state.peek.value.clamp(0.0, 1.2) * (1.0 - t);
        let base_w = base_w + PEEK_GROW_W * peek;
        let base_h = base_h + PEEK_GROW_H * peek;

        let w = lerp(base_w, open_w, t);
        let h = lerp(base_h, open_h, t);

        // Roundness and shoulders come from Appearance > Shape; the defaults
        // are the original constants.
        let look = &cfg.notch.shape;
        let radius_bottom = look.radius_at(base_h, t);

        // Fused to the bezel: square top with concave shoulders, open or shut.
        // The shoulders grow with the panel so they stay in proportion;
        // rounding the top corners off instead would leave two slivers of
        // desktop between the panel and the screen edge. Detached: a plain
        // floating panel rounded on all four corners.
        let attached = cfg.notch.offset_y <= 0;
        let (flare, radius_top) = if attached {
            (look.shoulder_at(t), 0.0)
        } else {
            (0.0, radius_bottom)
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
    fn visible_rect(&self, shape: &NotchShape, scene: &Scene) -> RECT {
        // +2 for the rounding either side of the fractional shape edges, with safety margin for neon bloom.
        // The shoulders stick out past the sides by their own reach.
        let reach = (theme::shadow_spread(self.state.expand.value) * 1.25 + 2.0)
            .max(shape.flare + 2.0)
            .max(18.0);

        // Pods sit beside the slab, so the window takes them in too.
        let (mut l, mut r) = (shape.left, shape.right);
        for pod in scene.pods.iter().flatten() {
            l = l.min(pod.shape.left - pod.shape.flare);
            r = r.max(pod.shape.right + pod.shape.flare);
        }

        let left = (l - reach).floor().max(0.0) as i32;
        let top = (shape.top - reach).floor().max(0.0) as i32;
        let right = (r + reach).ceil().min(self.placement.w as f32) as i32;
        let bottom = (shape.bottom + reach).ceil().min(self.placement.h as f32) as i32;

        RECT {
            left,
            top,
            right: right.max(left + 1),
            bottom: bottom.max(top + 1),
        }
    }

    /// Split segments and pods for this frame, laid out against `shape`.
    fn build_scene(&self, cfg: &AppConfig, shape: &NotchShape) -> Scene {
        let mut scene = Scene::default();

        let (split_l, split_r) = self.state.split_widths();
        if let Some(kind) = self.state.split[0].visible() {
            scene.segments[0] = Some(SegmentDraw {
                kind,
                left: shape.left,
                right: shape.left + split_l,
                alpha: self.state.split[0].amount(),
            });
        }
        if let Some(kind) = self.state.split[1].visible() {
            scene.segments[1] = Some(SegmentDraw {
                kind,
                left: shape.right - split_r,
                right: shape.right,
                alpha: self.state.split[1].amount(),
            });
        }

        let pod_shoulder = layout::pod_shoulder(shape.flare);
        for (i, slot) in self.state.pods.iter().enumerate() {
            let Some(kind) = slot.visible() else {
                continue;
            };
            let amount = slot.amount();
            let (left, right) = layout::pod_span(
                i,
                amount,
                shape.left,
                shape.right,
                shape.flare,
                kind.width(),
                pod_shoulder,
            );
            // A pod is the pill's satellite: same height and corners as the
            // resting pill, its own smaller shoulders.
            let pill_h = cfg.notch.collapsed_height as f32;
            let radius = cfg.notch.shape.radius_at(pill_h, 0.0);
            let floating = shape.flare <= 0.5;
            scene.pods[i] = Some(PodDraw {
                kind,
                shape: NotchShape {
                    left,
                    top: shape.top,
                    right,
                    bottom: shape.top + pill_h,
                    radius_top: if floating { radius } else { 0.0 },
                    radius_bottom: radius,
                    flare: if floating { 0.0 } else { pod_shoulder },
                },
                alpha: amount,
            });
        }
        scene
    }

    /// Re-read what pods could show, at most every [`POD_POLL`].
    fn poll_pods(&mut self, cfg: &AppConfig) {
        let pods = &cfg.notch.pods;
        if !pods.enabled {
            self.pod_live = PodCandidates::default();
            return;
        }
        if self.pod_polled_at.elapsed() < POD_POLL {
            return;
        }
        self.pod_polled_at = Instant::now();
        self.pod_live = PodCandidates {
            agent: pods.agents && crate::notch::notify::has_activity(),
            timer: pods.timer && crate::notch::timer::view(&cfg.notch.timer).running,
            calendar: pods.calendar && {
                use crate::notch::calendar;
                let now = calendar::now_local();
                let lead = pods.calendar_lead_mins as i64;
                calendar::upcoming(now, 3)
                    .iter()
                    .any(|e| !e.all_day && e.start <= now + lead && e.end > now)
            },
            media: pods.media && self.painter.media.snapshot().playing,
        };
    }

    /// The window, for things that need an owner (the right-click menu).
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// The pod under a point, if any.
    fn pod_at(scene: &Scene, x: f32, y: f32) -> Option<PodKind> {
        scene
            .pods
            .iter()
            .flatten()
            .find(|pod| pod.alpha > 0.5 && pod.shape.inset(-HOVER_SLOP).contains(x, y))
            .map(|pod| pod.kind)
    }

    /// Point a panel that is starting to open at what the cursor came in
    /// on: a pod opens onto its own slide, a split segment onto its face.
    fn aim_opening(
        &mut self,
        cfg: &AppConfig,
        on_pod: Option<PodKind>,
        cursor_x: Option<f32>,
        shape: &NotchShape,
        scene: &Scene,
    ) {
        if let Some(kind) = on_pod {
            self.state.open_onto(kind.opens(), cfg);
            self.hold_zone = scene
                .pods
                .iter()
                .flatten()
                .find(|pod| pod.kind == kind)
                .map(|pod| pod.shape.inset(-HOVER_SLOP));
        } else if let (Some(x), true) = (cursor_x, cfg.notch.split.open_hovered) {
            let (split_l, split_r) = self.state.split_widths();
            let part = layout::pill_part(x, shape.left, shape.right, split_l, split_r);
            let segment = match part {
                PillPart::Left => self.state.split[0].visible(),
                PillPart::Right => self.state.split[1].visible(),
                PillPart::Center => None,
            };
            if let Some(kind) = segment {
                self.state.open_onto(&[kind], cfg);
            }
        }
    }

    /// Re-read whether a full-screen app is in front, at most once a second.
    fn refresh_fullscreen(&mut self) {
        if self.fullscreen_read_at.elapsed() < FULLSCREEN_REFRESH {
            return;
        }
        self.fullscreen_read_at = Instant::now();
        self.fullscreen = unsafe { SHQueryUserNotificationState() }
            .map(|state| {
                state == QUNS_BUSY
                    || state == QUNS_RUNNING_D3D_FULL_SCREEN
                    || state == QUNS_PRESENTATION_MODE
            })
            .unwrap_or(false);
    }

    /// Whether a slide of `kind` is on screen as the resting face (or a split
    /// segment) and as part of the open panel, matching the painter's fades.
    fn slide_shows(&self, cfg: &AppConfig, kind: SlideKind) -> (bool, bool) {
        let slides = cfg.notch.effective_slides();
        let position = self.state.carousel.value;
        let expand = self.state.expand.value;
        let collapsed_visible = 1.0 - smoothstep(0.02, 0.30, expand) > 0.004;
        let expanded_alpha = smoothstep(0.42, 0.96, expand);
        let collapsed = collapsed_visible
            && (slides.get(self.state.active) == Some(&kind) || self.state.split_shows(kind));
        let expanded = expanded_alpha > 0.004
            && slides.iter().enumerate().any(|(index, slide)| {
                *slide == kind
                    && (index as f32 - position).abs() <= 1.05
                    && expanded_alpha
                        * (1.0 - (index as f32 - position).abs())
                            .clamp(0.0, 1.0)
                            .powf(1.4)
                        > 0.006
            });
        (collapsed, expanded)
    }

    /// Whether any music wave is on screen: the media pod, the Now Playing
    /// face on the pill, or the open player.
    fn wave_on_screen(&self, cfg: &AppConfig, scene: &Scene) -> bool {
        let pod = scene
            .pods
            .iter()
            .flatten()
            .any(|pod| pod.kind == PodKind::Media && pod.alpha > 0.01);
        let (collapsed, expanded) = self.slide_shows(cfg, SlideKind::Media);
        pod || collapsed || expanded
    }

    /// Keep the shortcuts registered with Windows in step with the settings.
    /// A shortcut another app already owns is reported to Settings rather
    /// than retried every frame.
    fn sync_hotkeys(&mut self, cfg: &AppConfig) {
        use crate::notch::keys::{self, HotkeyStatus};
        let wanted = cfg
            .notch
            .input
            .hotkeys()
            .map(|hotkey| hotkey.and_then(keys::registration));
        for (index, want) in wanted.into_iter().enumerate() {
            if want == self.hotkeys[index] {
                continue;
            }
            let id = index as i32 + 1;
            unsafe {
                if self.hotkeys[index].is_some() {
                    let _ = UnregisterHotKey(self.hwnd, id);
                }
                let status = match want {
                    Some((mods, vk)) => {
                        if RegisterHotKey(self.hwnd, id, HOT_KEY_MODIFIERS(mods), vk).is_ok() {
                            HotkeyStatus::Active
                        } else {
                            HotkeyStatus::InUse
                        }
                    }
                    None => HotkeyStatus::Off,
                };
                keys::set_status(index, status);
            }
            self.hotkeys[index] = want;
        }
    }

    /// Pin files dropped on the notch, and open onto the Shelf to show them.
    fn pin_dropped(&mut self, cfg: &mut AppConfig, paths: &[String]) -> bool {
        let mut changed = false;
        for path in paths {
            changed |= cfg.notch.shelf.pin(path);
        }
        if !changed {
            return false;
        }
        if !cfg.notch.slides.contains(&SlideKind::Shelf) {
            cfg.notch.slides.push(SlideKind::Shelf);
        }
        self.state.flash(Badge::AddedToShelf);
        self.state.open_sticky();
        self.state.open_onto(&[SlideKind::Shelf], cfg);
        true
    }

    /// The right-click menu for a point on the notch.
    fn menu_for(&self, cfg: &AppConfig, cx: f32, cy: f32, shape: NotchShape) -> MenuRequest {
        let mut at = POINT::default();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut at);
        }
        let mut entries = Vec::new();

        let open = self.state.is_open();
        let slide = cfg.notch.effective_slides().get(self.state.active).copied();
        if open && slide == Some(SlideKind::Shelf) {
            let body = crate::notch::geom::slide_body(shape);
            if let Some(entry) = crate::notch::paint::shelf_entry_at(body, cx, cy) {
                let path = entry.path.clone();
                entries.push(MenuEntry::item(MenuCommand::Open(path.clone()), "Open"));
                if entry.exists {
                    entries.push(MenuEntry::item(MenuCommand::CopyFile(path.clone()), "Copy"));
                }
                entries.push(MenuEntry::item(
                    MenuCommand::CopyPath(path.clone()),
                    "Copy path",
                ));
                entries.push(MenuEntry::item(
                    MenuCommand::Reveal(path.clone()),
                    "Show in folder",
                ));
                entries.push(if entry.pinned {
                    MenuEntry::item(MenuCommand::Unpin(path), "Remove from Shelf")
                } else {
                    MenuEntry::item(MenuCommand::Pin(path), "Pin to Shelf")
                });
                entries.push(MenuEntry::Separator);
            }
        }
        if open && slide == Some(SlideKind::Clipboard) && crate::notch::clipboard::len() > 0 {
            entries.push(MenuEntry::item(
                MenuCommand::ClearClipboard,
                "Clear clipboard history",
            ));
            entries.push(MenuEntry::Separator);
        }

        entries.push(MenuEntry::item(MenuCommand::Settings, "Open Settings"));
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::check(
            MenuCommand::TogglePin,
            "Keep open",
            self.state.pinned,
        ));
        if cfg.notch.input.open_with.hovers() {
            entries.push(if self.state.hover_paused() {
                MenuEntry::item(MenuCommand::ResumeHover, "Resume hover")
            } else {
                MenuEntry::item(MenuCommand::PauseHover, "Pause hover for 1 hour")
            });
        }
        entries.push(MenuEntry::check(
            MenuCommand::ToggleClickThrough,
            "Let clicks pass through",
            cfg.notch.click_through,
        ));
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::item(MenuCommand::NextPage, "Next page"));
        entries.push(MenuEntry::item(MenuCommand::PreviousPage, "Previous page"));
        if open {
            entries.push(MenuEntry::item(MenuCommand::Close, "Close"));
        }
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::item(MenuCommand::Quit, "Quit Venu"));

        MenuRequest {
            at: (at.x, at.y),
            entries,
        }
    }

    /// Carry out a menu choice. Returns whether the config needs saving.
    /// Settings and Quit are the manager's: they need the lock released.
    pub fn apply_menu(&mut self, command: MenuCommand, cfg: &mut AppConfig) -> bool {
        use crate::notch::{clipboard, shelf};
        match command {
            MenuCommand::TogglePin => {
                self.state.toggle_pinned();
                false
            }
            MenuCommand::PauseHover => {
                self.state.pause_hover(Some(PAUSE_FOR_AN_HOUR));
                self.state.flash(Badge::HoverPaused(true));
                false
            }
            MenuCommand::ResumeHover => {
                self.state.pause_hover(None);
                self.state.flash(Badge::HoverPaused(false));
                false
            }
            MenuCommand::NextPage | MenuCommand::PreviousPage => {
                let step = if command == MenuCommand::NextPage {
                    1
                } else {
                    -1
                };
                self.state.step_slide(step, cfg);
                if !self.state.is_open() {
                    self.state.face_override = true;
                }
                false
            }
            MenuCommand::ToggleClickThrough => {
                cfg.notch.click_through = !cfg.notch.click_through;
                self.state.flash_click_through();
                true
            }
            MenuCommand::Close => {
                self.state.close_now();
                false
            }
            MenuCommand::Open(path) => {
                shelf::open(&path);
                false
            }
            MenuCommand::CopyFile(path) => {
                clipboard::copy_file(&path);
                self.state.flash(Badge::Copied);
                false
            }
            MenuCommand::CopyPath(path) => {
                if clipboard::copy_text(&path) {
                    self.state.flash(Badge::Copied);
                }
                false
            }
            MenuCommand::Reveal(path) => {
                shelf::reveal(&path);
                false
            }
            MenuCommand::Pin(path) => {
                let pinned = cfg.notch.shelf.pin(&path);
                if pinned {
                    self.state.flash(Badge::AddedToShelf);
                }
                pinned
            }
            MenuCommand::Unpin(path) => {
                let before = cfg.notch.shelf.items.len();
                cfg.notch.shelf.items.retain(|p| p != &path);
                before != cfg.notch.shelf.items.len()
            }
            MenuCommand::ClearClipboard => {
                clipboard::clear();
                false
            }
            MenuCommand::Settings | MenuCommand::Quit => false,
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

        // Things that keep time whatever the notch is showing: the focus
        // timer rolling over, the battery being watched, the calendar feed.
        // Each is a lock and a comparison unless something is due.
        let toast_secs = cfg.notch.notifications.toast_duration_secs;
        crate::notch::timer::tick(&cfg.notch.timer, toast_secs);
        crate::notch::battery::tick(&cfg.notch.battery, toast_secs);
        crate::notch::calendar::configure(
            &cfg.notch.calendar.source,
            cfg.notch.calendar.refresh_mins,
        );

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

        // Moving text in a split segment is sized to the text it shows. The
        // measurement is cached by the painter, so this is a comparison a
        // tick unless the text, font or size changed.
        let split = &cfg.notch.split;
        self.state.text_segment_width = if split.enabled
            && (split.left == Some(SlideKind::Marquee) || split.right == Some(SlideKind::Marquee))
        {
            layout::text_segment_width(self.painter.marquee_text_width(cfg))
        } else {
            0.0
        };

        if let Some(selected_id) = self.state.selected_notification_id {
            let selected_still_exists = crate::notch::notify::global_store()
                .read()
                .items
                .iter()
                .any(|item| item.id == selected_id);
            if !selected_still_exists {
                self.state.selected_notification_id = None;
            }
        }

        // -- input ----------------------------------------------------------
        let shape = self.current_shape(cfg);
        let hit_shape = shape.inset(-HOVER_SLOP);
        let scene = self.build_scene(cfg, &shape);
        let open_with = cfg.notch.input.open_with;

        let cursor = self.cursor_in_window();
        let on_pill = cursor
            .map(|(x, y)| hit_shape.contains(x, y))
            .unwrap_or(false);
        let on_pod = cursor.and_then(|(x, y)| Self::pod_at(&scene, x, y));
        let held = self.state.expand.value > 0.01
            && cursor
                .zip(self.hold_zone)
                .is_some_and(|((x, y), zone)| zone.contains(x, y));
        let over = on_pill || on_pod.is_some() || held;
        let on_target = on_pill || on_pod.is_some();

        // How fast the cursor is moving, so sweeping across the pill on the
        // way to a browser tab never counts as resting on it.
        let speed = match (cursor, self.last_cursor) {
            (Some((x, y)), Some((px, py))) if dt > 0.0 => {
                ((x - px).powi(2) + (y - py).powi(2)).sqrt() / dt
            }
            _ => 0.0,
        };
        self.last_cursor = cursor;
        if !over {
            self.state.hover_blocked = false;
        }
        self.refresh_fullscreen();

        let toast_up = crate::notch::notify::global_store()
            .read()
            .active_toast
            .is_some();
        let hover_ok = open_with.hovers()
            && !self.state.hover_paused()
            && !self.state.hover_blocked
            && !hook::button_held()
            && (cfg.notch.input.hover_in_fullscreen || !self.fullscreen);

        let delay = cfg
            .notch
            .input
            .hover_delay_ms
            .min(InputConfig::MAX_HOVER_DELAY_MS) as f32
            / 1000.0;
        if on_target && hover_ok && !self.state.open_target && speed < FLYBY_SPEED {
            self.state.dwell += dt;
        } else {
            self.state.dwell = 0.0;
        }
        let hover_opens = hover_ok && on_target && self.state.dwell >= delay;
        let hover_hold =
            over && !self.state.hover_blocked && (self.state.open_target || hover_opens);

        // The closed pill grows a touch under the cursor: it says "this
        // opens", and while hover is counting down it says "about to".
        self.state.peeking = on_target
            && !self.state.open_target
            && !toast_up
            && !self.state.hover_blocked
            && (open_with.clicks() || hover_ok);

        // Arriving on a split segment or a pod opens the panel straight onto
        // that slide. Decided once, as the panel starts to open.
        if hover_opens && !self.state.open_target && !toast_up {
            self.aim_opening(cfg, on_pod, cursor.map(|(x, _)| x), &shape, &scene);
        }

        let wheel = hook::take_wheel();

        // Left and right together over the notch flips click-through. It is
        // deliberately the same gesture both ways round: once click-through is
        // on, an ordinary click cannot reach the notch to turn it off again.
        let mut flip_click_through = hook::take_chord();

        // The mouse trigger and the shortcuts open or close the notch.
        let mut toggle = hook::take_trigger();
        if hook::take_outside() && self.state.sticky && !self.state.pinned {
            self.state.release_sticky();
        }

        let drained = with_input(|bus| {
            let click = bus.click.take();
            let chars = std::mem::take(&mut bus.chars);
            let commit = std::mem::replace(&mut bus.commit, false);
            let cancel = std::mem::replace(&mut bus.cancel, false);
            let backspace = std::mem::replace(&mut bus.backspace, false);
            let lost_focus = std::mem::replace(&mut bus.lost_focus, false);
            let menu = bus.menu.take();
            let hotkeys = std::mem::take(&mut bus.hotkeys);
            let dropped = std::mem::take(&mut bus.dropped);
            InputDrain {
                click,
                chars,
                commit,
                cancel,
                backspace,
                lost_focus,
                menu,
                hotkeys,
                dropped,
            }
        })
        .unwrap_or_default();

        for id in &drained.hotkeys {
            match *id {
                HOTKEY_TOGGLE => toggle = true,
                HOTKEY_CLICK_THROUGH => flip_click_through = true,
                HOTKEY_PAUSE_HOVER => {
                    if self.state.hover_paused() {
                        self.state.pause_hover(None);
                    } else {
                        self.state.pause_hover(Some(PAUSE_UNTIL_RESUMED));
                    }
                    let paused = self.state.hover_paused();
                    self.state.flash(Badge::HoverPaused(paused));
                }
                _ => {}
            }
        }

        if flip_click_through {
            cfg.notch.click_through = !cfg.notch.click_through;
            self.state.flash_click_through();
            config_dirty = true;
        }
        self.apply_ex_style(cfg, self.state.editing);

        if toggle {
            let opening = !self.state.open_target;
            self.state.toggle_open();
            if opening && !toast_up && on_target {
                self.aim_opening(cfg, on_pod, cursor.map(|(x, _)| x), &shape, &scene);
            }
        }

        if !drained.dropped.is_empty() {
            config_dirty |= self.pin_dropped(cfg, &drained.dropped);
        }

        let scroll = cfg.notch.scroll_pages();
        if wheel != 0 && scroll.applies(self.state.is_open()) {
            // Wheel down walks forward through the deck, matching the way a
            // scroll gesture moves content upward.
            self.state.step_slide(-wheel.signum(), cfg);
            if !self.state.is_open() {
                // The closed pill keeps the face it was scrolled to.
                self.state.face_override = true;
            }
        }

        if let Some((cx, cy)) = drained.click {
            config_dirty |= self.handle_click(cfg, cx, cy, shape, &scene);
        }

        if let Some((cx, cy)) = drained.menu {
            self.menu = Some(self.menu_for(cfg, cx, cy, shape));
        }

        let InputDrain {
            chars,
            commit,
            cancel,
            backspace,
            lost_focus,
            ..
        } = drained;

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
        let target = self.state.set_hovered(hover_hold, delay);
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

        // Pods merge back into the notch while it is open or showing an
        // alert, and bud out again once it is resting.
        self.poll_pods(cfg);
        let resting = target == 0.0
            && self.state.expand.value <= 0.02
            && self.state.toast_progress.value <= 0.01;
        let wanted = if resting {
            let slides = cfg.notch.effective_slides();
            let on_pill = [
                slides.get(self.state.active).copied(),
                self.state.split[0].shown,
                self.state.split[1].shown,
            ];
            layout::wanted_pods(&cfg.notch.pods, self.pod_live, &on_pill)
        } else {
            [None, None]
        };
        self.state.step_pods(wanted, dt);

        // Reset to default collapsed slide on collapse
        if target == 0.0 && self.state.expand.value <= 0.05 {
            // A peek opened onto a segment or pod goes back to where it was.
            self.state.restore_home();
            self.hold_zone = None;
            if !self.state.face_override {
                let media_playing = self.painter.media.is_playing();
                let unread = notif_store.read().unread_count() > 0;
                self.state
                    .apply_default_collapsed(cfg, media_playing, unread);
            }
            self.state.sticky = false;
        }

        // Persist the resting slide so the notch comes back where it was.
        if self.state.dirty {
            cfg.notch.active_slide = self.state.active;
            self.state.dirty = false;
            config_dirty = true;
        }

        // Publish the freshly-animated shape for hit-testing and the hook.
        let shape = self.current_shape(cfg);
        let scene = self.build_scene(cfg, &shape);
        let open = self.state.is_open();
        let (wx, wy) = (self.placement.x, self.placement.y);
        let visible = self.visible_rect(&shape, &scene);
        let pod_hits = scene
            .pods
            .map(|pod| pod.map(|p| p.shape.inset(-HOVER_SLOP)));
        with_input(|bus| {
            bus.shape = Some(shape.inset(-HOVER_SLOP));
            bus.pods = pod_hits;
            bus.origin = (wx, wy);
            bus.client_offset = (visible.left as f32, visible.top as f32);
        });
        hook::publish(hook::Watch {
            wheel: cfg.notch.scroll_pages().applies(open),
            // The chord is watched for whether the notch is open or shut, so
            // it can be turned back off from the collapsed pill.
            chord: true,
            trigger: cfg.notch.input.mouse_trigger,
            outside: self.state.sticky && !self.state.pinned,
            bounds: (
                wx + shape.left as i32,
                wy + shape.top as i32,
                wx + shape.right as i32,
                wy + shape.bottom as i32,
            ),
        });

        // Background helpers follow the settings: the shelf's folder watcher
        // and the clipboard listener only run while they are wanted.
        let shelf_in_deck = cfg.notch.effective_slides().contains(&SlideKind::Shelf);
        let shelf_shown = self.slide_shows(cfg, SlideKind::Shelf).1;
        crate::notch::shelf::sync(&cfg.notch.shelf, shelf_in_deck, shelf_shown);
        crate::notch::clipboard::sync(cfg.notch.clipboard.enabled, cfg.notch.clipboard.max_items);
        self.sync_hotkeys(cfg);

        // The music wave moves only while it is on screen and music plays.
        let wave_live = cfg.notch.media.live_wave
            && self.painter.media.is_playing()
            && self.wave_on_screen(cfg, &scene);
        self.state.wave = self.audio.tick(wave_live);

        // -- paint ----------------------------------------------------------
        //
        // Everything above is cheap: a cursor read, some arithmetic and a few
        // atomics. The paint below is not, so it is only spent when the frame
        // would come out different — or when the last one it drew said it was
        // still moving under its own clock.
        let key = self.frame_key(shape, scene, cfg);
        let changed = self.last_key.as_ref() != Some(&key) || self.last_cfg.as_ref() != Some(cfg);

        let since_paint = self.last_paint.elapsed();
        let due = match self.last_motion {
            Motion::Continuous => true,
            Motion::Ambient => since_paint >= AMBIENT_FRAME,
            Motion::Still => since_paint >= KEEPALIVE_FRAME,
        };

        if changed || due {
            if let Err(e) = self.render(cfg, shape, &scene) {
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
            open_settings: std::mem::take(&mut self.open_settings),
            menu: self.menu.take(),
        }
    }

    /// Snapshot of everything that would change what the next frame looks
    /// like. See [`FrameKey`].
    fn frame_key(&self, shape: NotchShape, scene: Scene, cfg: &AppConfig) -> FrameKey {
        let slides = cfg.notch.effective_slides();
        let position = self.state.carousel.value;
        let expand = self.state.expand.value;
        let collapsed_visible = 1.0 - smoothstep(0.02, 0.30, expand) > 0.004;
        let expanded_alpha = smoothstep(0.42, 0.96, expand);
        // Whether a slide of this kind contributes pixels to the open
        // carousel, matching the painter's own fade so neither side guesses.
        let expanded_shows = |kind: SlideKind| {
            expanded_alpha > 0.004
                && slides.iter().enumerate().any(|(index, slide)| {
                    *slide == kind
                        && (index as f32 - position).abs() <= 1.05
                        && expanded_alpha
                            * (1.0 - (index as f32 - position).abs())
                                .clamp(0.0, 1.0)
                                .powf(1.4)
                            > 0.006
                })
        };
        // A face can be on the resting pill as its main face or in a split
        // segment; either way its live data has to keep the frame current.
        let collapsed_shows = |kind: SlideKind| {
            collapsed_visible
                && (slides.get(self.state.active) == Some(&kind) || self.state.split_shows(kind))
        };

        let collapsed_stats_visible = collapsed_shows(SlideKind::Stats)
            && crate::notch::notify::global_store()
                .read()
                .active_toast
                .is_none();
        let stats_visible = collapsed_stats_visible || expanded_shows(SlideKind::Stats);
        // Only the Clock slide reads the time. Keying every frame on it
        // repainted a collapsed notch once a second whatever it was showing.
        let clock_visible = collapsed_shows(SlideKind::Clock) || expanded_shows(SlideKind::Clock);
        if stats_visible {
            crate::stats::request_sampling();
        }
        let shows = |kind: SlideKind| collapsed_shows(kind) || expanded_shows(kind);
        let timer = if shows(SlideKind::Timer) {
            crate::notch::timer::view(&cfg.notch.timer).key()
        } else {
            0
        };
        let calendar = if shows(SlideKind::Calendar) {
            (
                crate::notch::calendar::revision(),
                crate::notch::calendar::now_local(),
            )
        } else {
            (0, 0)
        };
        let controls = if shows(SlideKind::Controls) {
            crate::notch::toggles::states()
        } else {
            Default::default()
        };
        let shelf = if shows(SlideKind::Shelf) {
            crate::notch::shelf::revision()
        } else {
            0
        };
        let clipboard = if shows(SlideKind::Clipboard) {
            crate::notch::clipboard::revision()
        } else {
            0
        };
        let media_clock = if expanded_shows(SlideKind::Media) {
            self.painter.media.position_key()
        } else {
            0
        };
        let stats_revision = if stats_visible {
            crate::stats::snapshot().revision
        } else {
            0
        };
        let notification_expanded_visible = expanded_alpha > 0.004
            && slides.iter().enumerate().any(|(index, slide)| {
                *slide == SlideKind::Notifications
                    && (index as f32 - position).abs() <= 1.05
                    && expanded_alpha
                        * (1.0 - (index as f32 - position).abs())
                            .clamp(0.0, 1.0)
                            .powf(1.4)
                        > 0.006
            });
        let notifications = crate::notch::notify::global_store();
        let notifications = notifications.read();
        let notification_pixels_visible = notifications.active_toast.is_some()
            || (collapsed_visible
                && slides.get(self.state.active) == Some(&SlideKind::Notifications))
            || notification_expanded_visible;
        let notification_revision = if notification_pixels_visible {
            notifications.revision()
        } else {
            0
        };

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
            badge_time: self.state.badge_time,
            selected_notification: self.state.selected_notification_id,
            capture_excluded: self.capture_excluded,
            clock: if clock_visible { clock_key() } else { 0 },
            media: self.painter.media.revision(),
            notifications: notification_revision,
            usage: crate::notch::notify::usage_revision(),
            // An effect scoped away from the notch's current state draws nothing,
            // so it must not keep the frames coming either.
            effect: self
                .state
                .effect_time
                .filter(|_| cfg.notch.border_effect.scope.visibility(expand) > 0.004)
                .map(|time| {
                    // A looping effect only moves the key at its capped rate, so
                    // frames in between are skipped like any unchanged frame.
                    let fx = &cfg.notch.border_effect;
                    let fps = if fx.continuous {
                        BorderEffectConfig::CONTINUOUS_FPS
                    } else {
                        60.0
                    };
                    (time * fps) as u32
                }),
            stats: stats_revision,
            timer,
            calendar,
            controls,
            wave: audio::key(&self.state.wave),
            shelf,
            clipboard,
            media_clock,
            adding_item: self.state.adding_item(),
            pod_content: scene
                .pods
                .map(|pod| pod.map(|p| self.pod_content_key(p.kind, cfg)).unwrap_or(0)),
            scene,
        }
    }

    /// What a pod's face reads, at the granularity it draws it.
    fn pod_content_key(&self, kind: PodKind, cfg: &AppConfig) -> u64 {
        match kind {
            PodKind::Timer => crate::notch::timer::view(&cfg.notch.timer).key(),
            PodKind::Agent => {
                let elapsed = crate::notch::notify::current_elapsed_secs().unwrap_or(0);
                elapsed ^ (crate::notch::notify::activity_revision() << 40)
            }
            PodKind::Calendar => {
                crate::notch::calendar::revision()
                    ^ (crate::notch::calendar::now_local() as u64) << 20
            }
            PodKind::Media => self.painter.media.revision(),
        }
    }

    fn handle_click(
        &mut self,
        cfg: &mut AppConfig,
        cx: f32,
        cy: f32,
        shape: NotchShape,
        scene: &Scene,
    ) -> bool {
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
                return false;
            }

            if !cfg.notch.input.open_with.clicks() {
                return false;
            }
            // A pod's quick action, where it has one; otherwise the click
            // opens onto its slide like a hover would.
            let pod = Self::pod_at(scene, cx, cy);
            match pod {
                Some(PodKind::Media) => {
                    self.painter.media.play_pause();
                    return false;
                }
                Some(PodKind::Timer) => {
                    crate::notch::timer::toggle(&cfg.notch.timer);
                    return false;
                }
                _ => {}
            }
            self.state.open_sticky();
            self.aim_opening(cfg, pod, Some(cx), &shape, scene);
            return false;
        }

        // Settings launcher button: opens the Settings panel.
        // Raising the window is left to the overlay thread once it has dropped
        // the config write lock -- see `NotchManager::tick`. Doing the Win32
        // calls here would mean holding that lock across a synchronous
        // cross-thread message to a window whose thread wants the same lock.
        let (sx, sy, sr) = shape.settings_button();
        let hit_s = sr + 5.0;
        if (cx - sx) * (cx - sx) + (cy - sy) * (cy - sy) <= hit_s * hit_s {
            self.open_settings = true;
            return false;
        }

        // Pin toggle lives above whatever slide is showing, so it is checked
        // before any per-slide hit-testing below.
        let (px, py, pr) = shape.pin_button();
        let hit_p = pr + 5.0; // a little slop past the drawn ring
        if (cx - px) * (cx - px) + (cy - py) * (cy - py) <= hit_p * hit_p {
            self.state.toggle_pinned();
            return false;
        }

        let slides = cfg.notch.effective_slides();
        let Some(slide) = slides.get(self.state.active) else {
            return false;
        };

        if *slide == crate::config::SlideKind::Notifications {
            let body = crate::notch::geom::slide_body(shape);
            let notif_store = crate::notch::notify::global_store();

            // If a notification is currently expanded into detailed view:
            if let Some(sel_id) = self.state.selected_notification_id {
                let back = crate::notch::geom::notification_back_button(body);
                if cx >= back.left && cx <= back.right && cy >= back.top && cy <= back.bottom {
                    self.state.selected_notification_id = None;
                    return false;
                }

                let dismiss = crate::notch::geom::notification_dismiss_button(body);
                if cx >= dismiss.left
                    && cx <= dismiss.right
                    && cy >= dismiss.top
                    && cy <= dismiss.bottom
                {
                    notif_store.write().dismiss_item(sel_id);
                    self.state.selected_notification_id = None;
                    return false;
                }
                return false;
            }

            // Normal list view:
            let clr = crate::notch::geom::notification_clear_button(body);
            if cx >= clr.left && cx <= clr.right && cy >= clr.top && cy <= clr.bottom {
                notif_store.write().clear_all();
                return false;
            }

            // Hit test individual notification card rows to open detailed view:
            let store = notif_store.read();
            let max_items = crate::notch::geom::notification_row_count(body).min(store.items.len());
            let mut selected_id = None;
            for i in 0..max_items {
                let r = crate::notch::geom::notification_item_rect(body, i);
                if cx >= r.left && cx <= r.right && cy >= r.top && cy <= r.bottom {
                    selected_id = store.items.get(i).map(|item| item.id);
                    break;
                }
            }
            drop(store);
            if let Some(id) = selected_id {
                self.state.selected_notification_id = Some(id);
                notif_store.write().mark_read(id);
            }
            return false;
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
            return false;
        }

        if *slide == crate::config::SlideKind::Timer {
            let body = crate::notch::geom::slide_body(shape);
            let buttons = crate::notch::geom::timer_buttons(body);
            for (i, (bx, by, br)) in buttons.into_iter().enumerate() {
                let hit = br + 5.0;
                if (cx - bx) * (cx - bx) + (cy - by) * (cy - by) <= hit * hit {
                    match i {
                        0 => crate::notch::timer::reset(),
                        1 => crate::notch::timer::toggle(&cfg.notch.timer),
                        _ => crate::notch::timer::skip(&cfg.notch.timer),
                    }
                    break;
                }
            }
            return false;
        }

        if *slide == crate::config::SlideKind::Controls {
            let body = crate::notch::geom::slide_body(shape);
            let tiles = crate::notch::geom::toggle_tiles(body);
            for (toggle, r) in crate::notch::toggles::Toggle::ALL.into_iter().zip(tiles) {
                if cx >= r.left && cx <= r.right && cy >= r.top && cy <= r.bottom {
                    crate::notch::toggles::flip(toggle);
                    break;
                }
            }
            return false;
        }

        if *slide == SlideKind::Shelf {
            let body = crate::notch::geom::slide_body(shape);
            if let Some(entry) = crate::notch::paint::shelf_entry_at(body, cx, cy) {
                crate::notch::shelf::open(&entry.path);
            }
            return false;
        }

        if *slide == SlideKind::Clipboard {
            let body = layout_rect(crate::notch::geom::slide_body(shape));
            if layout::clipboard_clear_button(body).contains(cx, cy) {
                crate::notch::clipboard::clear();
                return false;
            }
            let items = crate::notch::clipboard::items();
            let rows = layout::clipboard_rows(body, items.len());
            if let Some(index) = rows.iter().position(|r| r.contains(cx, cy)) {
                if let Some(text) = items.get(index) {
                    if crate::notch::clipboard::copy_text(text) {
                        self.state.flash(Badge::Copied);
                    }
                }
            }
            return false;
        }

        if *slide != SlideKind::Status {
            return false;
        }

        // Left: the editable focus line. Right: the short list, where a
        // click ticks an item off (or back on) and "+ Add" types a new one.
        let body = crate::notch::geom::slide_body(shape);
        let split = body.left + (body.right - body.left) * theme::SPLIT_RATIO;
        if cx < split && cy > shape.top {
            self.state.begin_edit(cfg);
            self.set_editing_style(cfg, true);
            return false;
        }
        let list = layout::status_list(
            crate::notch::geom::status_list_column(body),
            cfg.status.items.len(),
        );
        if let Some(index) = list.items.iter().position(|r| r.contains(cx, cy)) {
            if let Some(item) = cfg.status.items.get_mut(index) {
                item.done = !item.done;
                return true;
            }
        }
        if list.add.is_some_and(|r| r.contains(cx, cy)) {
            self.state.begin_add();
            self.set_editing_style(cfg, true);
        }
        false
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

    fn render(
        &mut self,
        cfg: &AppConfig,
        shape: NotchShape,
        scene: &Scene,
    ) -> windows::core::Result<()> {
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
            scene,
            (self.placement.x, self.placement.y),
            self.capture_excluded,
        )?;

        let mem_dc = self.surface.dc();
        if mem_dc.is_invalid() {
            return Ok(());
        }

        unsafe {
            let screen_dc = GetDC(None);
            let visible = self.visible_rect(&shape, scene);
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

                let inside = with_input(|bus| {
                    let (ox, oy) = bus.origin;
                    let (lx, ly) = ((x - ox) as f32, (y - oy) as f32);
                    bus.shape.is_some_and(|shape| shape.contains(lx, ly))
                        || bus.pods.iter().flatten().any(|pod| pod.contains(lx, ly))
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

            WM_RBUTTONUP => {
                let x = (lparam.0 & 0xFFFF) as i16 as f32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
                with_input(|bus| {
                    let (ox, oy) = bus.client_offset;
                    bus.menu = Some((x + ox, y + oy));
                });
                LRESULT(0)
            }

            WM_HOTKEY => {
                with_input(|bus| bus.hotkeys.push(wparam.0 as i32));
                LRESULT(0)
            }

            WM_DROPFILES => {
                let drop = HDROP(wparam.0 as *mut _);
                let count = DragQueryFileW(drop, u32::MAX, None).min(32);
                let mut paths = Vec::with_capacity(count as usize);
                for index in 0..count {
                    let len = DragQueryFileW(drop, index, None);
                    if len == 0 {
                        continue;
                    }
                    let mut buf = vec![0u16; len as usize + 1];
                    let got = DragQueryFileW(drop, index, Some(&mut buf)) as usize;
                    paths.push(String::from_utf16_lossy(&buf[..got.min(buf.len())]));
                }
                DragFinish(drop);
                with_input(|bus| bus.dropped.extend(paths));
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
                for (index, registered) in self.hotkeys.iter().enumerate() {
                    if registered.is_some() {
                        let _ = UnregisterHotKey(self.hwnd, index as i32 + 1);
                    }
                }
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}
