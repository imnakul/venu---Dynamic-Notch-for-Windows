//! Everything the notch draws.
//!
//! Layout is computed from the *animated* shape rather than the resting one,
//! so content is always composed against the box it will actually occupy this
//! frame. Content and shape are on separate timings: the slab opens first and
//! the contents rise into it a beat later, which is what stops the panel from
//! looking like a texture being stretched.

use std::cell::{Cell, RefCell};

use windows::core::{Interface, PCWSTR};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED,
    D2D1_FIGURE_END_OPEN, D2D1_FILL_MODE_WINDING, D2D_POINT_2F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Bitmap, ID2D1BitmapBrush, ID2D1DCRenderTarget, ID2D1Factory, ID2D1Geometry, ID2D1Layer,
    ID2D1PathGeometry, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_BRUSH_PROPERTIES,
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_COMBINE_MODE_EXCLUDE,
    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP,
    D2D1_LAYER_OPTIONS_NONE, D2D1_LAYER_PARAMETERS, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteTextLayout, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_EXTRA_BOLD,
    DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, IWICStream,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnLoad,
};
use windows::Win32::System::Com::{CoCreateInstance, IStream, CLSCTX_INPROC_SERVER};
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::config::{AppConfig, SlideKind};
use crate::notch::anim::{lerp, smoothstep};
use crate::notch::geom::NotchShape;
use crate::notch::layout::PodKind;
use crate::notch::media::{MediaWatcher, NowPlaying};
use crate::notch::state::{Badge, NotchState};
use crate::notch::surface::D2DSurface;
use crate::notch::text::TextEngine;
use crate::notch::theme::{self, Rgba};

#[path = "paint_panels.rs"]
mod panels;
pub use panels::shelf_entry_at;

const WEEKDAYS: [&str; 7] = [
    "SUNDAY",
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A decoded wallpaper, tied to the render target that created it.
struct CachedImage {
    path: String,
    generation: u64,
    bitmap: ID2D1Bitmap,
}

/// A decoded piece of Now Playing album art, tied to the render target that
/// created it. Keyed by the watcher's `art_generation` counter rather than a
/// path — there is no filename, just whatever bytes the media session handed
/// over this poll.
struct CachedArt {
    key: u64,
    generation: u64,
    bitmap: ID2D1Bitmap,
}

/// How much of the last painted frame was moving on its own.
///
/// The notch is an always-on overlay, so the only frames worth paying for are
/// the ones that differ from the one before. Almost everything that moves (the
/// open spring, the carousel, the marquee, the text) is visible in the notch's
/// own state and can be compared directly. What cannot is the motion
/// the painter drives from the free-running clock: the breathing accent dot,
/// the glow that sweeps a notification's border, and the desktop showing
/// through a glass theme. Those are reported from inside the paint instead,
/// so the frame gate never has to guess at what a slide happens to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Motion {
    /// Nothing is moving by itself. Repaint only when the state changes.
    #[default]
    Still,
    /// Slow, low-amplitude motion — a pulse fading in and out, a blurred
    /// backdrop. A handful of frames a second is indistinguishable from sixty
    /// and costs a fraction as much.
    Ambient,
    /// Motion that has to be smooth: keep the full frame rate.
    Continuous,
}

/// One side segment of the split pill: which face, where, how visible.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentDraw {
    pub kind: SlideKind,
    pub left: f32,
    pub right: f32,
    pub alpha: f32,
}

/// One satellite pod: what it shows, its own silhouette, how visible.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PodDraw {
    pub kind: PodKind,
    pub shape: NotchShape,
    pub alpha: f32,
}

/// Everything drawn besides the main slab and its slide. Built by the window
/// from the notch state each tick, and part of its frame key, so the frame
/// gate covers it like everything else. Empty — and free — unless the split
/// pill or pods are switched on and showing something.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Scene {
    /// `[left, right]` side segments of the split pill.
    pub segments: [Option<SegmentDraw>; 2],
    /// `[right, left]` satellite pods.
    pub pods: [Option<PodDraw>; 2],
}

impl Scene {
    fn has_pods(&self) -> bool {
        self.pods.iter().any(Option::is_some)
    }
}

pub struct Painter {
    pub text: TextEngine,
    /// GDI capture surface for the frosted theme's blur. Allocated lazily and
    /// only ever touched while that theme is active.
    backdrop: crate::notch::backdrop::BackdropCache,
    /// Resolved once per frame from the configured theme, so no drawing code
    /// ever has to branch on which theme is active.
    pal: theme::Palette,
    wic: Option<IWICImagingFactory>,
    image: Option<CachedImage>,
    /// Remembered so a decode failure is not retried sixty times a second.
    failed_path: Option<String>,
    /// Polls the system media session on its own thread; torn down along with
    /// the rest of the painter whenever the notch feature is disabled.
    pub media: MediaWatcher,
    media_art: Option<CachedArt>,
    media_art_failed: Option<u64>,
    /// Width of the moving text in the pill font, keyed on what it was
    /// measured for, so a text layout is only built when the text, font or
    /// size actually changes.
    text_measure: Option<(String, String, u32, f32)>,
    /// Raised by the drawing code while a frame is being painted. A `Cell`
    /// because the slides that report it are split between `&self` and
    /// `&mut self` methods and threading a flag back out of every one of them
    /// would be noise.
    motion: Cell<Motion>,
    /// The one solid-colour brush every fill and stroke goes through,
    /// recoloured per call, rather than a new COM object for each of the few
    /// hundred draw calls in a frame.
    solid: RefCell<Option<ID2D1SolidColorBrush>>,
    /// Surface generation the cached brush belongs to; it dies with its
    /// render target like every other device resource.
    solid_generation: u64,
    /// Gradients for the periodic border effects.
    effects: crate::notch::effects::EffectPainter,
    /// Shelf icons uploaded to the render target, by path.
    icons: RefCell<IconCache>,
    /// Segoe Fluent Icons on Windows 11, Segoe MDL2 Assets before it; the
    /// two share their code points.
    icon_family: &'static str,
}

/// Shelf icons as Direct2D bitmaps. They belong to one render target, so a
/// new surface generation empties the cache.
#[derive(Default)]
struct IconCache {
    generation: u64,
    bitmaps: std::collections::HashMap<
        String,
        (
            std::sync::Arc<crate::notch::shelf::Icon>,
            windows::Win32::Graphics::Direct2D::ID2D1Bitmap,
        ),
    >,
}

/// The pill is wide enough for music controls from here; side segments are
/// narrower and leave them out.
pub const PILL_CONTROLS_MIN_W: f32 = 250.0;

/// Local time, read once per frame.
struct Clock {
    hour24: u32,
    minute: u32,
    second: u32,
    day: u32,
    month: u32,
    weekday: u32,
}

impl Clock {
    fn now() -> Self {
        let st = unsafe { GetLocalTime() };
        Self {
            hour24: st.wHour as u32,
            minute: st.wMinute as u32,
            second: st.wSecond as u32,
            day: st.wDay as u32,
            month: st.wMonth as u32,
            weekday: st.wDayOfWeek as u32,
        }
    }

    fn time_string(&self, twenty_four: bool) -> String {
        if twenty_four {
            format!("{:02}:{:02}", self.hour24, self.minute)
        } else {
            let h = match self.hour24 % 12 {
                0 => 12,
                h => h,
            };
            format!("{}:{:02}", h, self.minute)
        }
    }

    fn meridiem(&self) -> &'static str {
        if self.hour24 < 12 {
            "AM"
        } else {
            "PM"
        }
    }

    /// How far through the day we are, 0..1. Drives the ember progress bar.
    fn day_fraction(&self) -> f32 {
        let secs = self.hour24 * 3600 + self.minute * 60 + self.second;
        secs as f32 / 86_400.0
    }
}

/// Everything the slides read off the clock, packed into one word so a frame
/// can be compared against the one before it.
///
/// A second is the finest granularity anything here uses: the clock face shows
/// minutes, but the day-progress bar beneath it moves with the seconds.
pub fn clock_key() -> u64 {
    let c = Clock::now();
    (c.second as u64)
        | (c.minute as u64) << 8
        | (c.hour24 as u64) << 16
        | (c.day as u64) << 24
        | (c.month as u64) << 32
        | (c.weekday as u64) << 40
}

impl Painter {
    pub fn new() -> windows::core::Result<Self> {
        let text = TextEngine::new()?;
        let icon_family = if text.system_has("Segoe Fluent Icons") {
            "Segoe Fluent Icons"
        } else {
            "Segoe MDL2 Assets"
        };
        let wic: Option<IWICImagingFactory> =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok() };

        if wic.is_none() {
            eprintln!("[notch] WIC unavailable, wallpaper slide will show its placeholder");
        }

        Ok(Self {
            text,
            backdrop: Default::default(),
            pal: theme::Palette::for_theme(crate::config::NotchTheme::Dark),
            wic,
            image: None,
            failed_path: None,
            media: MediaWatcher::spawn(),
            media_art: None,
            media_art_failed: None,
            text_measure: None,
            motion: Cell::new(Motion::Still),
            solid: RefCell::new(None),
            solid_generation: 0,
            effects: Default::default(),
            icons: RefCell::new(IconCache::default()),
            icon_family,
        })
    }

    /// Width of the moving text on one line in the pill's font, for sizing
    /// its split segment. Cached: only a change of text, font or size builds
    /// a new layout.
    pub fn marquee_text_width(&mut self, cfg: &AppConfig) -> f32 {
        let text = cfg.text.trim();
        let family = &cfg.notch.font_family;
        let size = cfg.marquee.pill_font(theme::SIZE_PILL);
        if let Some((t, f, s, w)) = &self.text_measure {
            if t == text && f == family && *s == size.to_bits() {
                return *w;
            }
        }
        let width = self
            .text
            .format(family, size, DWRITE_FONT_WEIGHT_SEMI_BOLD)
            .ok()
            .and_then(|fmt| self.text.layout(text, &fmt, 100_000.0, size * 4.0).ok())
            .map(|layout| TextEngine::measure(&layout).0)
            .unwrap_or(0.0);
        self.text_measure = Some((text.to_string(), family.clone(), size.to_bits(), width));
        width
    }

    /// What the frame just painted needs from the next one. Reading it clears
    /// it, so each report covers exactly one frame.
    pub fn take_motion(&self) -> Motion {
        self.motion.replace(Motion::Still)
    }

    /// Note that this frame contains motion of its own, and how smooth it has
    /// to be. The strongest report in a frame wins.
    fn report(&self, motion: Motion) {
        self.motion.set(self.motion.get().max(motion));
    }

    /// The ambient breathing applied to live indicator dots.
    ///
    /// Goes through the painter rather than straight to the state so that
    /// asking for the pulse is what keeps the frames coming: any slide that
    /// draws one stays animated without having to be listed anywhere else.
    fn pulse(&self, state: &NotchState) -> f32 {
        self.report(Motion::Ambient);
        state.pulse()
    }

    /// Called when the wallpaper path changes so the next frame re-decodes.
    pub fn invalidate_image(&mut self) {
        self.image = None;
        self.failed_path = None;
    }

    // -- brush / shape helpers ---------------------------------------------

    fn brush(
        &self,
        t: &ID2D1DCRenderTarget,
        color: Rgba,
    ) -> windows::core::Result<ID2D1SolidColorBrush> {
        // Direct2D takes the colour at each draw call, so recolouring between
        // calls is safe as long as no caller holds two at once. None does:
        // each draws with its brush before it asks for the next one.
        let color = theme::premul(color);
        if let Some(brush) = self.solid.borrow().as_ref() {
            unsafe { brush.SetColor(&color) };
            return Ok(brush.clone());
        }
        let brush = unsafe { t.CreateSolidColorBrush(&color, None)? };
        *self.solid.borrow_mut() = Some(brush.clone());
        Ok(brush)
    }

    fn fill_rrect(&self, t: &ID2D1DCRenderTarget, rect: D2D_RECT_F, radius: f32, color: Rgba) {
        if let Ok(b) = self.brush(t, color) {
            let rr = D2D1_ROUNDED_RECT {
                rect,
                radiusX: radius,
                radiusY: radius,
            };
            unsafe { t.FillRoundedRectangle(&rr, &b) };
        }
    }

    fn stroke_rrect(
        &self,
        t: &ID2D1DCRenderTarget,
        rect: D2D_RECT_F,
        radius: f32,
        width: f32,
        color: Rgba,
    ) {
        if color[3] <= 0.002 {
            return;
        }
        if let Ok(b) = self.brush(t, color) {
            let rr = D2D1_ROUNDED_RECT {
                rect,
                radiusX: radius,
                radiusY: radius,
            };
            unsafe { t.DrawRoundedRectangle(&rr, &b, width, None) };
        }
    }

    fn line(
        &self,
        t: &ID2D1DCRenderTarget,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        color: Rgba,
    ) {
        if color[3] <= 0.002 {
            return;
        }
        if let Ok(b) = self.brush(t, color) {
            unsafe {
                t.DrawLine(
                    D2D_POINT_2F { x: x1, y: y1 },
                    D2D_POINT_2F { x: x2, y: y2 },
                    &b,
                    width,
                    None,
                );
            }
        }
    }

    /// Vertical gradient approximated with banded strips.
    ///
    /// Direct2D gradient brushes would be one call, but banding at these sizes
    /// (a 40px scrim, a 60px sheen) is invisible at 2px steps and this keeps
    /// the render path to solid brushes only, which is easier to reason about
    /// against a premultiplied layered surface.
    fn vgradient(&self, t: &ID2D1DCRenderTarget, rect: D2D_RECT_F, top: Rgba, bottom: Rgba) {
        let height = rect.bottom - rect.top;
        if height <= 0.0 {
            return;
        }
        let steps = ((height / 2.0).ceil() as u32).clamp(2, 64);
        let step_h = height / steps as f32;

        for i in 0..steps {
            let f = (i as f32 + 0.5) / steps as f32;
            let color = [
                lerp(top[0], bottom[0], f),
                lerp(top[1], bottom[1], f),
                lerp(top[2], bottom[2], f),
                lerp(top[3], bottom[3], f),
            ];
            if color[3] <= 0.002 {
                continue;
            }
            if let Ok(b) = self.brush(t, color) {
                let strip = D2D_RECT_F {
                    left: rect.left,
                    top: rect.top + i as f32 * step_h,
                    right: rect.right,
                    // Overlap by a hair so seams never show as hairlines.
                    bottom: rect.top + (i as f32 + 1.0) * step_h + 0.5,
                };
                unsafe { t.FillRectangle(&strip, &b) };
            }
        }
    }

    /// Horizontal companion to [`Painter::vgradient`], used for marquee edge fades.
    fn hgradient(&self, t: &ID2D1DCRenderTarget, rect: D2D_RECT_F, left: Rgba, right: Rgba) {
        let width = rect.right - rect.left;
        if width <= 0.0 {
            return;
        }
        let steps = ((width / 2.0).ceil() as u32).clamp(2, 64);
        let step_w = width / steps as f32;

        for i in 0..steps {
            let f = (i as f32 + 0.5) / steps as f32;
            let color = [
                lerp(left[0], right[0], f),
                lerp(left[1], right[1], f),
                lerp(left[2], right[2], f),
                lerp(left[3], right[3], f),
            ];
            if color[3] <= 0.002 {
                continue;
            }
            if let Ok(b) = self.brush(t, color) {
                let strip = D2D_RECT_F {
                    left: rect.left + i as f32 * step_w,
                    top: rect.top,
                    right: rect.left + (i as f32 + 1.0) * step_w + 0.5,
                    bottom: rect.bottom,
                };
                unsafe { t.FillRectangle(&strip, &b) };
            }
        }
    }

    fn dot(&self, t: &ID2D1DCRenderTarget, x: f32, y: f32, r: f32, color: Rgba) {
        if let Ok(b) = self.brush(t, color) {
            let e = D2D1_ELLIPSE {
                point: D2D_POINT_2F { x, y },
                radiusX: r,
                radiusY: r,
            };
            unsafe { t.FillEllipse(&e, &b) };
        }
    }

    fn ring(&self, t: &ID2D1DCRenderTarget, x: f32, y: f32, r: f32, width: f32, color: Rgba) {
        if let Ok(b) = self.brush(t, color) {
            let e = D2D1_ELLIPSE {
                point: D2D_POINT_2F { x, y },
                radiusX: r,
                radiusY: r,
            };
            unsafe { t.DrawEllipse(&e, &b, width, None) };
        }
    }

    fn draw_layout(
        &self,
        t: &ID2D1DCRenderTarget,
        layout: &IDWriteTextLayout,
        x: f32,
        y: f32,
        color: Rgba,
    ) {
        if color[3] <= 0.004 {
            return;
        }
        if let Ok(b) = self.brush(t, color) {
            unsafe {
                t.DrawTextLayout(
                    D2D_POINT_2F { x, y },
                    layout,
                    &b,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                )
            };
        }
    }

    /// One-shot "lay this string out and draw it" for the common case.
    #[allow(clippy::too_many_arguments)]
    fn label(
        &self,
        t: &ID2D1DCRenderTarget,
        family: &str,
        text: &str,
        size: f32,
        weight: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT,
        tracking: f32,
        max_w: f32,
        x: f32,
        y: f32,
        color: Rgba,
    ) -> f32 {
        let Ok(format) = self.text.format(family, size, weight) else {
            return 0.0;
        };
        self.text.set_ellipsis(&format);
        let Ok(layout) = self.text.layout(text, &format, max_w, size * 2.4) else {
            return 0.0;
        };
        if tracking > 0.0 {
            TextEngine::set_tracking(&layout, tracking, text.encode_utf16().count() as u32);
        }
        self.draw_layout(t, &layout, x, y, color);
        TextEngine::measure(&layout).1
    }

    fn clip(&self, t: &ID2D1DCRenderTarget, rect: D2D_RECT_F) {
        unsafe { t.PushAxisAlignedClip(&rect, D2D1_ANTIALIAS_MODE_ALIASED) };
    }

    fn unclip(&self, t: &ID2D1DCRenderTarget) {
        unsafe { t.PopAxisAlignedClip() };
    }

    // -- frame --------------------------------------------------------------

    /// Paint one frame into `surface`. `shape` is in window-local coordinates.
    /// `origin` is where the window sits on the desktop; the frosted theme
    /// needs it to know which part of the screen to capture. `may_capture` is
    /// false until the window is confirmed hidden from screen capture, without
    /// which sampling the desktop would sample the notch itself.
    pub fn paint(
        &mut self,
        surface: &mut D2DSurface,
        cfg: &AppConfig,
        state: &NotchState,
        shape: NotchShape,
        scene: &Scene,
        origin: (i32, i32),
        may_capture: bool,
        menu: Option<&crate::notch::menu::MenuView>,
    ) -> windows::core::Result<()> {
        let (win_w, win_h) = surface.size();
        if win_w == 0 || win_h == 0 {
            return Ok(());
        }

        self.pal = theme::Palette::for_theme(cfg.notch.theme);
        // The user's finish strengths scale the theme's own values, so every
        // theme keeps its character and "1.0" is exactly the theme as shipped.
        let finish = &cfg.notch.shape;
        self.pal.edge[3] *= finish.outline_strength();
        self.pal.sheen[3] *= finish.sheen_strength();
        self.pal.shadow_strength *= finish.shadow_strength();
        // Each frame reports its own motion from scratch.
        self.motion.set(Motion::Still);

        let generation = surface.generation();
        let factory = surface.factory.clone();
        let Some(t) = surface.target() else {
            return Ok(());
        };
        let t = t.clone();

        // Device resources die with their target.
        if self.solid_generation != generation {
            self.solid_generation = generation;
            *self.solid.get_mut() = None;
        }
        if let Some(img) = &self.image {
            if img.generation != generation {
                self.image = None;
            }
        }
        if let Some(art) = &self.media_art {
            if art.generation != generation {
                self.media_art = None;
            }
        }

        unsafe { t.BeginDraw() };

        unsafe {
            t.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }))
        };

        let expand = state.expand.value.clamp(0.0, 1.0);

        let geometry = shape.build(&factory).ok();
        // Pods are separate little slabs. Their outlines are built only when
        // there are pods to draw, so a notch without any pays nothing here.
        let pod_geometry: [Option<ID2D1PathGeometry>; 2] = std::array::from_fn(|i| {
            scene.pods[i]
                .as_ref()
                .and_then(|pod| pod.shape.build(&factory).ok())
        });
        self.paint_shadow(
            &t,
            &factory,
            geometry.as_ref(),
            &shape,
            scene,
            &pod_geometry,
            expand,
            (win_w, win_h),
        );

        // Frosted and friends lay a blurred capture of the desktop under a
        // half-transparent tint. One capture covers the notch and its pods.
        if self.pal.blur_backdrop && may_capture {
            let mut slabs: Vec<(&ID2D1PathGeometry, &NotchShape, f32)> = Vec::with_capacity(3);
            for (pod, geo) in scene.pods.iter().zip(&pod_geometry) {
                if let (Some(pod), Some(geo)) = (pod, geo) {
                    slabs.push((geo, &pod.shape, pod.alpha));
                }
            }
            if let Some(geo) = &geometry {
                slabs.push((geo, &shape, 1.0));
            }
            self.paint_backdrop(&t, &slabs, origin);
        }

        // Pods before the slab: while they bud out they are tucked under it.
        for (pod, geo) in scene.pods.iter().zip(&pod_geometry) {
            if let (Some(pod), Some(geo)) = (pod, geo) {
                self.paint_glass(&t, &factory, &pod.shape, geo, cfg.notch.surface, pod.alpha);
            }
        }

        // The slab itself.
        if let Some(geometry) = geometry {
            let outline = self.paint_glass(&t, &factory, &shape, &geometry, cfg.notch.surface, 1.0);

            // The periodic border effect, if one is mid-run. Above the glass,
            // under the content, and never over a notification glow.
            if let (Some(progress), Some(rim)) = (state.effect_progress(cfg), &outline) {
                let fx = &cfg.notch.border_effect;
                let shown = fx.scope.visibility(state.expand.value);
                if state.toast_progress.value <= 0.01 && shown > 0.004 {
                    self.effects
                        .paint(&t, generation, fx, &shape, &geometry, rim, progress, shown);
                }
            }
        }

        // 3-Sided Glowing Border Alert Drain Timer / Glow Animation (painted unclipped for bloom)
        self.paint_notification_glow(&t, &factory, cfg, state, &shape);

        // Content lives inside the slab with a small safety inset so nothing
        // can graze the antialiased edge.
        let content = D2D_RECT_F {
            left: shape.left + 1.0,
            top: shape.top + 1.0,
            right: shape.right - 1.0,
            bottom: shape.bottom - 1.0,
        };
        self.clip(&t, content);

        let slides = cfg.notch.effective_slides();
        let collapsed_alpha = 1.0 - smoothstep(0.02, 0.30, expand);
        let expanded_alpha = smoothstep(0.42, 0.96, expand);

        if collapsed_alpha > 0.004 {
            let idx = state.active.min(slides.len().saturating_sub(1));
            let toast_up = crate::notch::notify::global_store()
                .read()
                .active_toast
                .is_some();
            if toast_up || scene.segments.iter().all(Option::is_none) {
                self.paint_collapsed(
                    &t,
                    cfg,
                    state,
                    slides[idx],
                    shape,
                    collapsed_alpha,
                    generation,
                );
            } else {
                self.paint_split(
                    &t,
                    cfg,
                    state,
                    slides[idx],
                    shape,
                    scene,
                    collapsed_alpha,
                    generation,
                );
            }
        }

        if expanded_alpha > 0.004 {
            self.paint_carousel(
                &t,
                &factory,
                cfg,
                state,
                slides,
                shape,
                expanded_alpha,
                generation,
            );
        }

        // Hover highlight and click ripple over whatever is under them: the
        // open panel's targets, or the music controls on the pill.
        let feedback_alpha = expanded_alpha.max(collapsed_alpha);
        if feedback_alpha > 0.004 {
            self.paint_feedback(&t, &factory, cfg, state, feedback_alpha);
        }

        self.paint_click_through(&t, cfg, state, shape);

        self.unclip(&t);

        // Pod faces, each clipped to its own little slab.
        for pod in scene.pods.iter().flatten() {
            self.paint_pod_face(&t, cfg, state, pod);
        }

        // The right-click menu floats over everything.
        if let Some(menu) = menu {
            self.paint_menu(&t, cfg, menu);
        }

        unsafe { t.EndDraw(None, None)? };
        Ok(())
    }

    /// 3-Sided glowing border around notch with time-based countdown unwrap/drain and glow modes.
    fn paint_notification_glow(
        &self,
        t: &ID2D1DCRenderTarget,
        factory: &ID2D1Factory,
        cfg: &AppConfig,
        state: &NotchState,
        shape: &NotchShape,
    ) {
        let notif_store = crate::notch::notify::global_store();
        let store = notif_store.read();
        let Some(toast) = &store.active_toast else {
            return;
        };

        let glow_alpha = state.toast_progress.value.clamp(0.0, 1.0);
        if glow_alpha <= 0.01 {
            return;
        }

        // The sweep, the hue cycle and the drain all run off `state.elapsed`,
        // which the frame gate cannot see. Hold the full frame rate for as
        // long as the glow is on screen.
        self.report(Motion::Continuous);

        let app_color = cfg
            .notch
            .notifications
            .get_app_color(&toast.notification.app);

        let drain = (toast.remaining / toast.duration).clamp(0.0, 1.0);

        match cfg.notch.notifications.glow_style {
            crate::config::NotificationGlowStyle::CountdownDrain => {
                if drain > 0.005 {
                    if let Ok((geom, (hx, hy))) =
                        build_3sided_border_geometry(factory, shape, drain)
                    {
                        // Multi-pass neon bloom along smooth continuous path
                        if let Ok(b_diffuse) =
                            self.brush(t, theme::fade(app_color, glow_alpha * 0.22))
                        {
                            unsafe { t.DrawGeometry(&geom, &b_diffuse, 8.0, None) };
                        }
                        if let Ok(b_halo) = self.brush(t, theme::fade(app_color, glow_alpha * 0.58))
                        {
                            unsafe { t.DrawGeometry(&geom, &b_halo, 4.0, None) };
                        }
                        if let Ok(b_core) = self.brush(t, theme::fade(app_color, glow_alpha * 0.98))
                        {
                            unsafe { t.DrawGeometry(&geom, &b_core, 2.0, None) };
                        }

                        // Glowing spark point on active draining head
                        self.dot(t, hx, hy, 4.0, theme::fade(app_color, glow_alpha * 0.9));
                        self.dot(
                            t,
                            hx,
                            hy,
                            2.0,
                            theme::fade([1.0, 1.0, 1.0, 1.0], glow_alpha),
                        );
                    }
                }
            }

            crate::config::NotificationGlowStyle::RgbBorderMoving => {
                let path_points = shape.rim_points();
                let len = path_points.len();
                if len >= 2 {
                    for i in 0..len - 1 {
                        let (x1, y1) = path_points[i];
                        let (x2, y2) = path_points[i + 1];
                        let u = i as f32 / len as f32;
                        let hue = (u * 1.6 - state.elapsed * 1.4).rem_euclid(1.0);
                        let seg_color = hsl_to_rgb(hue, 0.95, 0.58);

                        self.line(
                            t,
                            x1,
                            y1,
                            x2,
                            y2,
                            7.0,
                            theme::fade(seg_color, glow_alpha * 0.25),
                        );
                        self.line(
                            t,
                            x1,
                            y1,
                            x2,
                            y2,
                            3.6,
                            theme::fade(seg_color, glow_alpha * 0.6),
                        );
                        self.line(
                            t,
                            x1,
                            y1,
                            x2,
                            y2,
                            1.8,
                            theme::fade(seg_color, glow_alpha * 0.98),
                        );
                    }
                }
            }

            crate::config::NotificationGlowStyle::WavyRgbMoving => {
                let path_points = shape.rim_points();
                let len = path_points.len();
                if len >= 2 {
                    for i in 0..len - 1 {
                        let (x1, y1) = path_points[i];
                        let (x2, y2) = path_points[i + 1];
                        let u = i as f32 / len as f32;
                        let wave = (u * 14.0 - state.elapsed * 5.5).sin() * 0.5 + 0.5;
                        let hue = (u + state.elapsed * 0.75).rem_euclid(1.0);
                        let seg_color = hsl_to_rgb(hue, 0.95, 0.55);
                        let stroke_w = 1.4 + wave * 2.8;

                        self.line(
                            t,
                            x1,
                            y1,
                            x2,
                            y2,
                            stroke_w * 2.2,
                            theme::fade(seg_color, glow_alpha * 0.28),
                        );
                        self.line(
                            t,
                            x1,
                            y1,
                            x2,
                            y2,
                            stroke_w,
                            theme::fade(seg_color, glow_alpha * 0.95),
                        );
                    }
                }
            }

            crate::config::NotificationGlowStyle::NeonGlow => {
                let pulse = 0.82 + 0.18 * (state.elapsed * 3.5).sin();
                if let Ok((geom, _)) = build_3sided_border_geometry(factory, shape, 1.0) {
                    if let Ok(b_diffuse) =
                        self.brush(t, theme::fade(app_color, glow_alpha * 0.20 * pulse))
                    {
                        unsafe { t.DrawGeometry(&geom, &b_diffuse, 10.0, None) };
                    }
                    if let Ok(b_halo) =
                        self.brush(t, theme::fade(app_color, glow_alpha * 0.55 * pulse))
                    {
                        unsafe { t.DrawGeometry(&geom, &b_halo, 5.0, None) };
                    }
                    if let Ok(b_core) = self.brush(t, theme::fade(app_color, glow_alpha * 0.98)) {
                        unsafe { t.DrawGeometry(&geom, &b_core, 2.0, None) };
                    }
                }
            }
        }
    }

    /// A short banner confirming what just changed — click-through switched,
    /// hover paused, something copied — since none of those shows by itself.
    ///
    /// A gesture whose whole effect is that clicks stop landing here has no
    /// natural feedback of its own, so the notch has to say what happened —
    /// otherwise the only symptom is that the notch appears to have died.
    fn paint_click_through(
        &self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        state: &NotchState,
        shape: NotchShape,
    ) {
        let flash = state.badge_time;
        if flash <= 0.0 {
            return;
        }

        // Full opacity for most of the dwell, then a fade out, so it reads as
        // a deliberate confirmation rather than a flicker.
        let alpha = (flash / 0.45).min(1.0);
        let text = match state.badge {
            Badge::ClickThrough if cfg.notch.click_through => "CLICKS PASS THROUGH",
            Badge::ClickThrough => "CLICKS LAND HERE",
            Badge::HoverPaused(true) => "HOVER PAUSED",
            Badge::HoverPaused(false) => "HOVER IS BACK ON",
            Badge::Copied => "COPIED",
            Badge::AddedToShelf => "ADDED TO SHELF",
        };

        let size = theme::SIZE_LABEL;
        let cx = (shape.left + shape.right) * 0.5;
        let cy = (shape.top + shape.bottom) * 0.5;

        let Ok(fmt) = self
            .text
            .format(&cfg.notch.font_family, size, DWRITE_FONT_WEIGHT_SEMI_BOLD)
        else {
            return;
        };
        let Ok(layout) = self.text.layout(text, &fmt, shape.width(), shape.height()) else {
            return;
        };
        TextEngine::set_tracking(&layout, theme::TRACK_LABEL, text.len() as u32);
        let (tw, th) = TextEngine::measure(&layout);

        // A scrim behind it, because the badge lands over whatever slide
        // happens to be showing — including a photo.
        let pad_x = 11.0;
        let pad_y = 5.0;
        let plate = D2D_RECT_F {
            left: cx - tw * 0.5 - pad_x,
            top: cy - th * 0.5 - pad_y,
            right: cx + tw * 0.5 + pad_x,
            bottom: cy + th * 0.5 + pad_y,
        };
        self.fill_rrect(
            t,
            plate,
            (th * 0.5 + pad_y).min(14.0),
            theme::fade(self.pal.scrim, alpha),
        );
        self.draw_layout(
            t,
            &layout,
            cx - tw * 0.5,
            cy - th * 0.5,
            theme::fade(self.pal.text_hi, alpha),
        );
    }

    /// Capture the desktop under the slab, blurred, and fill the silhouette
    /// with it. Silently does nothing if the capture is unavailable.
    /// Fill each slab with a blurred capture of the desktop behind it.
    ///
    /// One capture covers the bounding box of every slab — the notch and any
    /// pods — so pods add no extra screen grab, only a slightly wider one.
    fn paint_backdrop(
        &mut self,
        t: &ID2D1DCRenderTarget,
        slabs: &[(&ID2D1PathGeometry, &NotchShape, f32)],
        origin: (i32, i32),
    ) {
        if slabs.is_empty() {
            return;
        }
        // What is behind the notch is not ours to watch for changes, so a
        // glass theme has to keep resampling. Ambient rather than continuous:
        // a blur this heavy, behind a strip this small, reads the same at a
        // few frames a second as it does at sixty.
        self.report(Motion::Ambient);

        let left = slabs
            .iter()
            .map(|(_, s, _)| s.left)
            .fold(f32::MAX, f32::min);
        let right = slabs
            .iter()
            .map(|(_, s, _)| s.right)
            .fold(f32::MIN, f32::max);
        let top = slabs.iter().map(|(_, s, _)| s.top).fold(f32::MAX, f32::min);
        let bottom = slabs
            .iter()
            .map(|(_, s, _)| s.bottom)
            .fold(f32::MIN, f32::max);

        let x = origin.0 + left.floor() as i32;
        let y = origin.1 + top.floor() as i32;
        let w = (right - left).ceil() as i32;
        let h = (bottom - top).ceil() as i32;

        let Some((bitmap, sx, sy)) = self.backdrop.sample(t, x, y, w, h, self.pal.blur_downscale)
        else {
            return;
        };

        let props = D2D1_BITMAP_BRUSH_PROPERTIES {
            // Clamp, so the bilinear upscale does not wrap the far edge of the
            // capture around to the near one along the shoulders.
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
        };

        let brush: ID2D1BitmapBrush =
            match unsafe { t.CreateBitmapBrush(&bitmap, Some(&props), None) } {
                Ok(b) => b,
                Err(_) => return,
            };

        unsafe {
            let stretch = Matrix3x2 {
                M11: sx,
                M12: 0.0,
                M21: 0.0,
                M22: sy,
                M31: 0.0,
                M32: 0.0,
            };
            brush.SetTransform(&(stretch * Matrix3x2::translation(left, top)));
            for (geometry, _, alpha) in slabs {
                brush.SetOpacity(alpha.clamp(0.0, 1.0));
                t.FillGeometry(*geometry, &brush, None);
            }
        }
    }

    /// The glass of one slab: tint, hairline edge and the specular lip, all
    /// at `alpha`. Shared by the notch and its pods so they are one material.
    /// Returns the edge outline, which the border effects trace.
    fn paint_glass(
        &self,
        t: &ID2D1DCRenderTarget,
        factory: &ID2D1Factory,
        shape: &NotchShape,
        geometry: &ID2D1PathGeometry,
        surface: Rgba,
        alpha: f32,
    ) -> Option<ID2D1PathGeometry> {
        if let Ok(fill) = self.brush(t, theme::fade(surface, alpha)) {
            unsafe { t.FillGeometry(geometry, &fill, None) };
        }
        // Hairline edge, then a brighter specular lip across the top third.
        // Neither runs along a fused top: that edge is the screen's.
        let outline = shape.build_edge(factory).ok();
        if let (Some(outline), Ok(edge)) =
            (&outline, self.brush(t, theme::fade(self.pal.edge, alpha)))
        {
            unsafe { t.DrawGeometry(outline, &edge, 1.0, None) };
        }
        if let Ok(inner) = shape.inset(0.9).build_edge(factory) {
            if let Ok(sheen) = self.brush(t, theme::fade(self.pal.sheen, alpha)) {
                self.clip(
                    t,
                    D2D_RECT_F {
                        left: shape.left - shape.flare,
                        top: shape.top,
                        right: shape.right + shape.flare,
                        bottom: shape.top + shape.height() * 0.42,
                    },
                );
                unsafe { t.DrawGeometry(&inner, &sheen, 1.4, None) };
                self.unclip(t);
            }
        }
        outline
    }

    /// Stacked rings of decreasing alpha standing in for a real blur. Cheap,
    /// and at this radius indistinguishable from a Gaussian shadow.
    ///
    /// The rings are masked to fall outside the slab. Under an opaque surface
    /// that changes nothing, but the glass themes are see-through, and a
    /// shadow showing through them reads as a second, darker panel with its
    /// own rounded top sitting under the shoulders.
    #[allow(clippy::too_many_arguments)]
    fn paint_shadow(
        &self,
        t: &ID2D1DCRenderTarget,
        factory: &ID2D1Factory,
        slab: Option<&ID2D1PathGeometry>,
        shape: &NotchShape,
        scene: &Scene,
        pod_geometry: &[Option<ID2D1PathGeometry>; 2],
        expand: f32,
        (win_w, win_h): (u32, u32),
    ) {
        let bounds = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: win_w as f32,
            bottom: win_h as f32,
        };
        // The mask excludes every slab — notch and pods — so no shadow shows
        // through glass. A group is only built when there are pods.
        let glass: Option<ID2D1Geometry> = if scene.has_pods() {
            let parts: Vec<Option<ID2D1Geometry>> = slab
                .into_iter()
                .chain(pod_geometry.iter().flatten())
                .map(|g| g.cast().ok())
                .collect();
            unsafe { factory.CreateGeometryGroup(D2D1_FILL_MODE_WINDING, &parts) }
                .ok()
                .and_then(|group| group.cast().ok())
        } else {
            slab.and_then(|g| g.cast().ok())
        };
        let mask = glass.and_then(|glass| outside(factory, bounds, &glass).ok());
        if let Some(mask) = &mask {
            let params = D2D1_LAYER_PARAMETERS {
                contentBounds: bounds,
                geometricMask: std::mem::ManuallyDrop::new(Some(mask.cast().unwrap())),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity: 1.0,
                opacityBrush: std::mem::ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS_NONE,
            };
            unsafe { t.PushLayer(&params, None::<&ID2D1Layer>) };
            drop(std::mem::ManuallyDrop::into_inner(params.geometricMask));
        }
        self.paint_shadow_rings(t, shape, expand, 1.0);
        for pod in scene.pods.iter().flatten() {
            self.paint_shadow_rings(t, &pod.shape, 0.0, pod.alpha);
        }
        if mask.is_some() {
            unsafe { t.PopLayer() };
        }
    }

    fn paint_shadow_rings(
        &self,
        t: &ID2D1DCRenderTarget,
        shape: &NotchShape,
        expand: f32,
        opacity: f32,
    ) {
        const RINGS: u32 = 14;
        let spread = theme::shadow_spread(expand);
        // A light panel over a light desktop needs a harder shadow than a
        // black slab does to separate from it at all.
        let strength = lerp(0.16, 0.34, expand) * self.pal.shadow_strength * opacity;

        for i in (0..RINGS).rev() {
            let f = (i as f32 + 1.0) / RINGS as f32;
            let grow = spread * f;
            let alpha = strength * (1.0 - f).powi(2) / RINGS as f32 * 6.0;
            if alpha <= 0.002 {
                continue;
            }
            // A fused slab's shadow starts below its shoulders; otherwise it
            // fills the concave curves and smudges the very line they draw.
            let top = if shape.fused() {
                shape.top + shape.flare
            } else {
                shape.top - grow * 0.35
            };
            let rect = D2D_RECT_F {
                left: shape.left - grow,
                top,
                right: shape.right + grow,
                // Shadow pools below the slab, as if lit from above.
                bottom: shape.bottom + grow * 1.25,
            };
            self.fill_rrect(t, rect, shape.radius_bottom + grow, [0.0, 0.0, 0.0, alpha]);
        }
    }

    // -- collapsed ----------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn paint_collapsed(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        state: &NotchState,
        slide: SlideKind,
        shape: NotchShape,
        alpha: f32,
        generation: u64,
    ) {
        let family = &cfg.notch.font_family;
        let cy = (shape.top + shape.bottom) * 0.5;

        let notif_store = crate::notch::notify::global_store();
        let store_read = notif_store.read();

        if let Some(toast) = &store_read.active_toast {
            let app_name = &toast.notification.app;
            let title = &toast.notification.title;
            let body = &toast.notification.body;

            let app_color = cfg
                .notch
                .notifications
                .get_app_color(&toast.notification.app);

            // App badge pill on left
            let pill_w = 78.0;
            let pill_h = 20.0;
            let pill_rect = D2D_RECT_F {
                left: shape.left + 12.0,
                top: cy - pill_h * 0.5,
                right: shape.left + 12.0 + pill_w,
                bottom: cy + pill_h * 0.5,
            };
            self.fill_rrect(t, pill_rect, 4.0, theme::fade(app_color, alpha * 0.22));
            self.stroke_rrect(t, pill_rect, 4.0, 1.0, theme::fade(app_color, alpha * 0.6));

            self.label(
                t,
                family,
                &app_name.to_uppercase(),
                theme::SIZE_LABEL - 2.0,
                DWRITE_FONT_WEIGHT_BOLD,
                0.5,
                pill_w - 4.0,
                pill_rect.left + 6.0,
                pill_rect.top + 2.0,
                theme::fade(app_color, alpha),
            );

            // Title + Body text preview
            let text_x = pill_rect.right + 10.0;
            let avail = (shape.right - text_x - 14.0).max(20.0);
            let display_text = if body.trim().is_empty() {
                title.clone()
            } else {
                format!("{} \u{2014} {}", title, body)
            };

            let Ok(fmt) = self
                .text
                .format(family, theme::SIZE_PILL, DWRITE_FONT_WEIGHT_SEMI_BOLD)
            else {
                return;
            };
            self.text.set_ellipsis(&fmt);
            let Ok(layout) = self.text.layout(&display_text, &fmt, avail, shape.height()) else {
                return;
            };
            let (_, th) = TextEngine::measure(&layout);
            self.draw_layout(
                t,
                &layout,
                text_x,
                cy - th * 0.5,
                theme::fade(self.pal.text_hi, alpha),
            );
            return;
        }
        // Released before the face is drawn: the Notifications face takes
        // its own read of the same store.
        drop(store_read);

        self.paint_face(t, cfg, state, slide, shape, alpha, generation);
    }

    /// One slide's compact face, laid out inside `shape`: the whole pill, or
    /// one segment of a split pill. Every face sizes itself from the shape it
    /// is given, which is what lets the split pill reuse them unchanged.
    #[allow(clippy::too_many_arguments)]
    fn paint_face(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        state: &NotchState,
        slide: SlideKind,
        shape: NotchShape,
        alpha: f32,
        generation: u64,
    ) {
        let family = &cfg.notch.font_family;
        let pad = theme::PILL_GUTTER;
        let cx = (shape.left + shape.right) * 0.5;
        let cy = (shape.top + shape.bottom) * 0.5;
        let inner_w = (shape.width() - pad * 2.0).max(10.0);

        match slide {
            SlideKind::Clock => {
                let clock = Clock::now();
                let time = clock.time_string(cfg.notch.clock_24h);

                let Ok(fmt) = self.text.format(
                    crate::fonts::GEIST_MONO,
                    theme::SIZE_PILL + 1.0,
                    DWRITE_FONT_WEIGHT_BOLD,
                ) else {
                    return;
                };
                let Ok(layout) = self.text.layout(&time, &fmt, inner_w, shape.height()) else {
                    return;
                };
                let (tw, th) = TextEngine::measure(&layout);

                let suffix = if cfg.notch.clock_24h {
                    String::new()
                } else {
                    clock.meridiem().to_string()
                };
                let mut suffix_w = 0.0;
                let mut suffix_layout = None;
                if !suffix.is_empty() {
                    if let Ok(sfmt) = self.text.format(
                        family,
                        theme::SIZE_LABEL - 0.5,
                        DWRITE_FONT_WEIGHT_EXTRA_BOLD,
                    ) {
                        if let Ok(sl) = self.text.layout(&suffix, &sfmt, 60.0, shape.height()) {
                            TextEngine::set_tracking(&sl, 0.8, suffix.len() as u32);
                            suffix_w = TextEngine::measure(&sl).0 + 5.0;
                            suffix_layout = Some(sl);
                        }
                    }
                }

                let total = tw + suffix_w;
                let x = cx - total * 0.5;
                self.draw_layout(
                    t,
                    &layout,
                    x,
                    cy - th * 0.5,
                    theme::fade(self.pal.text_hi, alpha),
                );
                if let Some(sl) = suffix_layout {
                    let (_, sh) = TextEngine::measure(&sl);
                    self.draw_layout(
                        t,
                        &sl,
                        x + tw + 5.0,
                        // Sits on the baseline of the time, not its centre.
                        cy - th * 0.5 + (th - sh) * 0.72,
                        theme::fade(self.pal.text_lo, alpha),
                    );
                }
            }

            SlideKind::Status => {
                let dot_r = 3.0;
                let dot_x = shape.left + pad + dot_r;
                self.dot(
                    t,
                    dot_x,
                    cy,
                    dot_r,
                    theme::fade(cfg.notch.accent, alpha * self.pulse(state)),
                );

                let text_x = dot_x + dot_r + 9.0;
                let avail = (shape.right - pad - text_x).max(10.0);
                let focus = state.display_focus(cfg);
                let Ok(fmt) =
                    self.text
                        .format(family, theme::SIZE_PILL, DWRITE_FONT_WEIGHT_SEMI_BOLD)
                else {
                    return;
                };
                self.text.set_ellipsis(&fmt);
                let Ok(layout) = self.text.layout(&focus, &fmt, avail, shape.height()) else {
                    return;
                };
                let (_, th) = TextEngine::measure(&layout);
                self.draw_layout(
                    t,
                    &layout,
                    text_x,
                    cy - th * 0.5,
                    theme::fade(self.pal.text_hi, alpha),
                );
            }

            SlideKind::Marquee => {
                self.paint_marquee_strip(
                    t,
                    cfg,
                    state,
                    D2D_RECT_F {
                        left: shape.left + 2.0,
                        top: shape.top,
                        right: shape.right - 2.0,
                        bottom: shape.bottom,
                    },
                    cfg.marquee.pill_font(theme::SIZE_PILL),
                    alpha,
                    cfg.notch.surface,
                );
            }

            SlideKind::Wallpaper => {
                // The pill *is* the picture. A thumbnail beside a caption
                // reads as a file listing; the photo itself reads as a
                // reminder, which is the whole point of the slide.
                let art = D2D_RECT_F {
                    left: shape.left + 3.0,
                    top: shape.top + 2.0,
                    right: shape.right - 3.0,
                    bottom: shape.bottom - 3.0,
                };
                let radius = (shape.radius_bottom - 3.0).max(4.0);
                let drawn = self.fill_with_image(t, cfg, art, radius, alpha, generation);

                let caption = cfg.wallpaper.caption.trim().to_string();

                if drawn {
                    if !caption.is_empty() {
                        // One flat scrim, not a gradient: at 34px tall a
                        // gradient has no room to resolve and just muddies.
                        self.fill_rrect(t, art, radius, theme::fade(self.pal.scrim, alpha * 0.46));
                        let avail = (art.right - art.left - 16.0).max(10.0);
                        if let Ok(fmt) =
                            self.text
                                .format(family, theme::SIZE_PILL, DWRITE_FONT_WEIGHT_SEMI_BOLD)
                        {
                            self.text.set_ellipsis(&fmt);
                            if let Ok(layout) =
                                self.text.layout(&caption, &fmt, avail, shape.height())
                            {
                                let (tw, th) = TextEngine::measure(&layout);
                                self.draw_layout(
                                    t,
                                    &layout,
                                    cx - tw * 0.5,
                                    cy - th * 0.5,
                                    // Always white: it sits on a photo, not on
                                    // the theme's surface.
                                    theme::fade([1.0, 1.0, 1.0, 1.0], alpha),
                                );
                            }
                        }
                    }
                } else {
                    let label = if caption.is_empty() {
                        "Wallpaper".to_string()
                    } else {
                        caption
                    };
                    let thumb = shape.height() - 12.0;
                    let thumb_rect = D2D_RECT_F {
                        left: shape.left + pad,
                        top: cy - thumb * 0.5,
                        right: shape.left + pad + thumb,
                        bottom: cy + thumb * 0.5,
                    };
                    self.fill_rrect(
                        t,
                        thumb_rect,
                        5.0,
                        theme::fade(cfg.notch.accent, alpha * 0.5),
                    );

                    let text_x = thumb_rect.right + 9.0;
                    let avail = (shape.right - pad - text_x).max(10.0);
                    self.label(
                        t,
                        family,
                        &label,
                        theme::SIZE_PILL,
                        DWRITE_FONT_WEIGHT_SEMI_BOLD,
                        0.0,
                        avail,
                        text_x,
                        cy - theme::SIZE_PILL * 0.72,
                        theme::fade(self.pal.text_hi, alpha),
                    );
                }
            }

            SlideKind::Media => {
                let now = self.media.snapshot();
                // A small live wave where the dot used to be.
                let wave_w = 15.0;
                self.wave(
                    t,
                    shape.left + pad + wave_w * 0.5,
                    cy,
                    shape.height() * 0.42,
                    3.0,
                    &self.wave_levels(state),
                    theme::fade(
                        cfg.notch.accent,
                        alpha * if now.playing { 1.0 } else { 0.6 },
                    ),
                );

                let text_x = shape.left + pad + wave_w + 9.0;
                // On the main pill, previous / play-pause / next sit at the
                // right end; a side segment is too narrow for them.
                let controls = now.has_session && shape.width() >= PILL_CONTROLS_MIN_W;
                let right_edge = if controls {
                    let buttons = crate::notch::geom::pill_media_buttons(shape);
                    let glyphs = [
                        "\u{E892}",
                        if now.playing { "\u{E769}" } else { "\u{E768}" },
                        "\u{E893}",
                    ];
                    for ((x, y, _), glyph) in buttons.into_iter().zip(glyphs) {
                        self.glyph(t, glyph, 10.5, x, y, theme::fade(self.pal.text_hi, alpha));
                    }
                    buttons[0].0 - 14.0
                } else {
                    shape.right - pad
                };
                let avail = (right_edge - text_x).max(10.0);
                let text = if !now.has_session {
                    "Nothing playing".to_string()
                } else if now.artist.trim().is_empty() {
                    now.title.trim().to_string()
                } else {
                    format!("{} \u{2014} {}", now.title.trim(), now.artist.trim())
                };

                let Ok(fmt) =
                    self.text
                        .format(family, theme::SIZE_PILL, DWRITE_FONT_WEIGHT_SEMI_BOLD)
                else {
                    return;
                };
                self.text.set_ellipsis(&fmt);
                let Ok(layout) = self.text.layout(&text, &fmt, avail, shape.height()) else {
                    return;
                };
                let (_, th) = TextEngine::measure(&layout);
                self.draw_layout(
                    t,
                    &layout,
                    text_x,
                    cy - th * 0.5,
                    theme::fade(self.pal.text_hi, alpha),
                );
            }

            SlideKind::Notifications => {
                let notif_store = crate::notch::notify::global_store();
                let store = notif_store.read();
                let unread = store.unread_count();
                let dot_r = 3.5;
                let dot_x = shape.left + pad + dot_r;
                let has_unread = unread > 0;
                self.dot(
                    t,
                    dot_x,
                    cy,
                    dot_r,
                    theme::fade(
                        if has_unread {
                            [0.22, 0.74, 0.97, 1.0]
                        } else {
                            self.pal.text_lo
                        },
                        alpha * if has_unread { self.pulse(state) } else { 0.7 },
                    ),
                );

                let text_x = dot_x + dot_r + 9.0;
                let avail = (shape.right - pad - text_x).max(10.0);
                let text = if let Some(latest) = store.items.first() {
                    format!("{}: {}", latest.app, latest.title)
                } else {
                    "No notifications".to_string()
                };

                let Ok(fmt) =
                    self.text
                        .format(family, theme::SIZE_PILL, DWRITE_FONT_WEIGHT_SEMI_BOLD)
                else {
                    return;
                };
                self.text.set_ellipsis(&fmt);
                let Ok(layout) = self.text.layout(&text, &fmt, avail, shape.height()) else {
                    return;
                };
                let (_, th) = TextEngine::measure(&layout);
                self.draw_layout(
                    t,
                    &layout,
                    text_x,
                    cy - th * 0.5,
                    theme::fade(self.pal.text_hi, alpha),
                );
            }

            SlideKind::Usage => {
                let snapshot = crate::notch::notify::usage_store().read().clone();
                let dot_r = 3.0;
                let dot_x = shape.left + pad + dot_r;
                self.dot(
                    t,
                    dot_x,
                    cy,
                    dot_r,
                    theme::fade(cfg.notch.accent, alpha * 0.8),
                );

                let text_x = dot_x + dot_r + 9.0;
                let avail = (shape.right - pad - text_x).max(10.0);
                let text = match snapshot.context_used_pct {
                    Some(pct) => format!("Context {:.0}%", pct),
                    None => "Claude usage".to_string(),
                };

                let Ok(fmt) =
                    self.text
                        .format(family, theme::SIZE_PILL, DWRITE_FONT_WEIGHT_SEMI_BOLD)
                else {
                    return;
                };
                self.text.set_ellipsis(&fmt);
                let Ok(layout) = self.text.layout(&text, &fmt, avail, shape.height()) else {
                    return;
                };
                let (_, th) = TextEngine::measure(&layout);
                self.draw_layout(
                    t,
                    &layout,
                    text_x,
                    cy - th * 0.5,
                    theme::fade(self.pal.text_hi, alpha),
                );
            }
            SlideKind::Stats => {
                let stats = crate::stats::snapshot();
                let metrics = [
                    ("CPU", stats.cpu_pct),
                    ("RAM", stats.ram_pct),
                    ("GPU", stats.gpu_pct),
                ];
                // Laid out edge to edge: the first column starts on the
                // padding, the last ends on it, the middle one sits halfway.
                // Spreading three equal columns instead left the outer ends
                // padded by however much narrower the text was than its
                // column, so a pill with Stats at one end looked lopsided.
                let content_left = shape.left + pad;
                let content_right = shape.right - pad;
                /// One metric, laid out once and placed by its width.
                #[derive(Default)]
                struct Column {
                    label: Option<(IDWriteTextLayout, f32)>,
                    value: Option<(IDWriteTextLayout, f32, f32)>,
                }
                impl Column {
                    fn width(&self) -> f32 {
                        let label = self.label.as_ref().map_or(0.0, |l| l.1);
                        let value = self.value.as_ref().map_or(0.0, |v| v.1);
                        label.max(value)
                    }
                }
                let columns: Vec<Column> = metrics
                    .into_iter()
                    .map(|(name, value)| {
                        let value = value
                            .map(|pct| format!("{pct:.0}%"))
                            .unwrap_or_else(|| "\u{2014}".to_string());
                        Column {
                            label: self
                                .measure_text(family, name, 8.0, DWRITE_FONT_WEIGHT_BOLD)
                                .map(|(l, w, _)| (l, w)),
                            value: self.measure_text(
                                crate::fonts::GEIST_MONO,
                                &value,
                                14.5,
                                DWRITE_FONT_WEIGHT_MEDIUM,
                            ),
                        }
                    })
                    .collect();
                let (w0, w1, w2) = (columns[0].width(), columns[1].width(), columns[2].width());
                let centres = [
                    content_left + w0 * 0.5,
                    (content_left + w0 + content_right - w2) * 0.5,
                    content_right - w2 * 0.5,
                ];
                let edges = [
                    (centres[0] + w0 * 0.5, centres[1] - w1 * 0.5),
                    (centres[1] + w1 * 0.5, centres[2] - w2 * 0.5),
                ];

                for (column, centre) in columns.iter().zip(centres) {
                    if let Some((label, w)) = &column.label {
                        self.draw_layout(
                            t,
                            label,
                            centre - w * 0.5,
                            cy - 14.0,
                            theme::fade(self.pal.text_lo, alpha),
                        );
                    }
                    if let Some((value, w, h)) = &column.value {
                        self.draw_layout(
                            t,
                            value,
                            centre - w * 0.5,
                            cy + 4.5 - h * 0.5,
                            theme::fade(self.pal.text_hi, alpha),
                        );
                    }
                }
                for (left, right) in edges {
                    let x = ((left + right) * 0.5).round();
                    self.fill_rrect(
                        t,
                        D2D_RECT_F {
                            left: x,
                            top: cy - 10.0,
                            right: x + 1.0,
                            bottom: cy + 11.0,
                        },
                        0.5,
                        theme::fade(self.pal.rule, alpha),
                    );
                }
            }
            SlideKind::Timer => {
                use crate::notch::timer::{self, Phase};
                let view = timer::view(&cfg.notch.timer);
                let tone = if view.phase == Phase::Focus {
                    cfg.notch.accent
                } else {
                    BREAK_GREEN
                };
                let left = shape.left + pad;
                self.dot(t, left + 4.0, cy, 3.5, theme::fade(tone, alpha));
                let word = if !view.running && view.progress() > 0.0 {
                    "PAUSED"
                } else if view.phase == Phase::Focus {
                    "FOCUS"
                } else {
                    "BREAK"
                };
                self.label(
                    t,
                    family,
                    word,
                    theme::SIZE_LABEL - 1.0,
                    DWRITE_FONT_WEIGHT_EXTRA_BOLD,
                    theme::TRACK_LABEL,
                    inner_w * 0.5,
                    left + 14.0,
                    cy - 7.0,
                    theme::fade(self.pal.text_lo, alpha),
                );
                if let Some((layout, w, h)) = self.measure_text(
                    crate::fonts::GEIST_MONO,
                    &view.clock(),
                    14.5,
                    DWRITE_FONT_WEIGHT_MEDIUM,
                ) {
                    self.draw_layout(
                        t,
                        &layout,
                        shape.right - pad - w,
                        cy - h * 0.5,
                        theme::fade(self.pal.text_hi, alpha),
                    );
                }
            }
            SlideKind::Calendar => {
                use crate::notch::calendar;
                let now = calendar::now_local();
                let soon = calendar::upcoming(now, 5);
                let next = soon.iter().find(|e| !e.all_day).or(soon.first()).cloned();
                let (lead, title, tone) = match &next {
                    Some(ev) if ev.all_day => {
                        ("TODAY".to_owned(), ev.title.clone(), cfg.notch.accent)
                    }
                    Some(ev) if ev.start <= now => {
                        ("NOW".to_owned(), ev.title.clone(), BREAK_GREEN)
                    }
                    Some(ev) => (
                        span_short(ev.start - now).to_uppercase(),
                        ev.title.clone(),
                        cfg.notch.accent,
                    ),
                    None => ("CAL".to_owned(), "No events".to_owned(), self.pal.text_lo),
                };
                let left = shape.left + pad;
                let lead_w = self
                    .measure_text(
                        family,
                        &lead,
                        theme::SIZE_LABEL,
                        DWRITE_FONT_WEIGHT_EXTRA_BOLD,
                    )
                    .map(|(_, w, _)| w)
                    .unwrap_or(30.0);
                self.label(
                    t,
                    family,
                    &lead,
                    theme::SIZE_LABEL,
                    DWRITE_FONT_WEIGHT_EXTRA_BOLD,
                    0.0,
                    inner_w,
                    left,
                    cy - 7.0,
                    theme::fade(tone, alpha),
                );
                self.label(
                    t,
                    family,
                    &title,
                    theme::SIZE_PILL - 1.0,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    0.0,
                    (inner_w - lead_w - 10.0).max(10.0),
                    left + lead_w + 10.0,
                    cy - 8.5,
                    theme::fade(self.pal.text_hi, alpha),
                );
            }
            SlideKind::Controls => {
                use crate::notch::toggles::{self, Toggle};
                let states = toggles::states();
                let step = inner_w / Toggle::ALL.len() as f32;
                for (i, toggle) in Toggle::ALL.into_iter().enumerate() {
                    let on = states.get(toggle).unwrap_or(false);
                    let x = shape.left + pad + step * (i as f32 + 0.5);
                    let color = if on {
                        theme::fade(cfg.notch.accent, alpha)
                    } else {
                        theme::fade(self.pal.text_lo, alpha)
                    };
                    self.glyph(t, toggle.glyph(on), 13.0, x, cy, color);
                }
            }
            SlideKind::Shelf => {
                let count = crate::notch::shelf::snapshot().pinned.len();
                self.glyph(
                    t,
                    "\u{E718}",
                    11.0,
                    shape.left + pad + 6.0,
                    cy,
                    theme::fade(cfg.notch.accent, alpha),
                );
                let text = match count {
                    0 => "Shelf".to_owned(),
                    1 => "1 pinned".to_owned(),
                    n => format!("{n} pinned"),
                };
                self.label(
                    t,
                    family,
                    &text,
                    theme::SIZE_PILL,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    0.0,
                    (inner_w - 20.0).max(10.0),
                    shape.left + pad + 20.0,
                    cy - theme::SIZE_PILL * 0.72,
                    theme::fade(self.pal.text_hi, alpha),
                );
            }
            SlideKind::Clipboard => {
                self.glyph(
                    t,
                    "\u{E77F}",
                    11.0,
                    shape.left + pad + 6.0,
                    cy,
                    theme::fade(cfg.notch.accent, alpha),
                );
                let text = if !cfg.notch.clipboard.enabled {
                    "Clipboard off".to_owned()
                } else {
                    crate::notch::clipboard::latest()
                        .map(|t| crate::notch::clipboard::one_line(&t))
                        .unwrap_or_else(|| "Nothing copied yet".to_owned())
                };
                self.label(
                    t,
                    family,
                    &text,
                    theme::SIZE_PILL,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    0.0,
                    (inner_w - 20.0).max(10.0),
                    shape.left + pad + 20.0,
                    cy - theme::SIZE_PILL * 0.72,
                    theme::fade(self.pal.text_hi, alpha),
                );
            }
        }
    }

    // -- split pill and pods ------------------------------------------------

    /// The resting pill split into segments: the face the notch is resting
    /// on in the middle, a fixed face either side, a hairline between.
    #[allow(clippy::too_many_arguments)]
    fn paint_split(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        state: &NotchState,
        center: SlideKind,
        shape: NotchShape,
        scene: &Scene,
        alpha: f32,
        generation: u64,
    ) {
        let [left, right] = scene.segments;
        let middle = NotchShape {
            left: left.map(|s| s.right).unwrap_or(shape.left),
            right: right.map(|s| s.left).unwrap_or(shape.right),
            ..shape
        };
        self.paint_clipped_face(t, cfg, state, center, middle, alpha, generation);

        let cy = (shape.top + shape.bottom) * 0.5;
        let rule_half = shape.height() * 0.22;
        for (segment, inner_edge) in [
            (left, left.map(|s| s.right)),
            (right, right.map(|s| s.left)),
        ] {
            let (Some(segment), Some(edge)) = (segment, inner_edge) else {
                continue;
            };
            let part = NotchShape {
                left: segment.left,
                right: segment.right,
                ..shape
            };
            let shown = alpha * segment.alpha;
            self.paint_clipped_face(t, cfg, state, segment.kind, part, shown, generation);
            self.fill_rrect(
                t,
                D2D_RECT_F {
                    left: edge - 0.5,
                    top: cy - rule_half,
                    right: edge + 0.5,
                    bottom: cy + rule_half,
                },
                0.5,
                theme::fade(self.pal.rule, shown),
            );
        }
    }

    /// A face clipped to its own segment, so a wide face never draws into
    /// its neighbour while the segments are still growing in.
    ///
    /// Moving text and the wallpaper normally run edge to edge, while every
    /// other face keeps a gutter of its own. In a split pill that made the
    /// gaps either side of a divider unequal, so those two are inset here to
    /// the same padding as the rest.
    #[allow(clippy::too_many_arguments)]
    fn paint_clipped_face(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        state: &NotchState,
        slide: SlideKind,
        shape: NotchShape,
        alpha: f32,
        generation: u64,
    ) {
        if alpha <= 0.004 || shape.width() < 8.0 {
            return;
        }
        let shape = match slide {
            // Their own edge insets are 2 and 3 px; bring both to the gutter.
            SlideKind::Marquee | SlideKind::Wallpaper => {
                let inset = (crate::notch::layout::SEGMENT_PADDING - 2.5)
                    .min((shape.width() - 8.0).max(0.0) * 0.5);
                NotchShape {
                    left: shape.left + inset,
                    right: shape.right - inset,
                    ..shape
                }
            }
            _ => shape,
        };
        self.clip(
            t,
            D2D_RECT_F {
                left: shape.left,
                top: shape.top,
                right: shape.right,
                bottom: shape.bottom,
            },
        );
        self.paint_face(t, cfg, state, slide, shape, alpha, generation);
        self.unclip(t);
    }

    /// What a pod says. Still apart from the music wave: a pod's text changes
    /// at most once a second (a timer, an elapsed time), and the wave only
    /// moves while music plays and the live wave is on.
    fn paint_pod_face(
        &self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        state: &NotchState,
        pod: &PodDraw,
    ) {
        let alpha = pod.alpha;
        if alpha <= 0.004 {
            return;
        }
        let shape = &pod.shape;
        let cy = (shape.top + shape.bottom) * 0.5;
        let glyph_x = shape.left + 15.0;
        let text_right = shape.right - 11.0;

        self.clip(
            t,
            D2D_RECT_F {
                left: shape.left,
                top: shape.top,
                right: shape.right,
                bottom: shape.bottom,
            },
        );

        let readout = match pod.kind {
            PodKind::Timer => {
                use crate::notch::timer::{self, Phase};
                let view = timer::view(&cfg.notch.timer);
                let tone = if view.phase == Phase::Focus {
                    cfg.notch.accent
                } else {
                    BREAK_GREEN
                };
                self.progress_ring(t, glyph_x, cy, 6.0, view.progress(), tone, alpha);
                Some(view.clock())
            }
            PodKind::Agent => {
                let activity = crate::notch::notify::current_activity();
                let tone = activity
                    .as_ref()
                    .map(|a| cfg.notch.notifications.get_app_color(&a.app))
                    .unwrap_or(cfg.notch.accent);
                self.ring(t, glyph_x, cy, 6.0, 1.6, theme::fade(tone, alpha * 0.45));
                self.dot(t, glyph_x, cy, 3.2, theme::fade(tone, alpha));
                activity.map(|a| elapsed_text(a.elapsed_secs()))
            }
            PodKind::Calendar => {
                use crate::notch::calendar;
                let now = calendar::now_local();
                let next = calendar::upcoming(now, 3).into_iter().find(|e| !e.all_day);
                let (word, tone) = match &next {
                    Some(ev) if ev.start <= now => ("NOW".to_owned(), BREAK_GREEN),
                    Some(ev) => (span_short(ev.start - now), cfg.notch.accent),
                    None => ("--".to_owned(), self.pal.text_lo),
                };
                self.dot(t, glyph_x, cy, 3.5, theme::fade(tone, alpha));
                Some(word)
            }
            PodKind::Media => {
                // The music wave: following the sound while it plays, a
                // flat line when paused.
                let levels = self.wave_levels(state);
                let mid = (shape.left + shape.right) * 0.5;
                self.wave(
                    t,
                    mid,
                    cy,
                    shape.height() * 0.5,
                    3.0,
                    &levels,
                    theme::fade(cfg.notch.accent, alpha),
                );
                None
            }
        };

        if let Some(text) = readout {
            if let Some((layout, w, h)) = self.measure_text(
                crate::fonts::GEIST_MONO,
                &text,
                12.0,
                DWRITE_FONT_WEIGHT_MEDIUM,
            ) {
                self.draw_layout(
                    t,
                    &layout,
                    text_right - w,
                    cy - h * 0.5,
                    theme::fade(self.pal.text_hi, alpha),
                );
            }
        }
        self.unclip(t);
    }

    /// A thin ring with `progress` (0..1) of it drawn in `tone`, clockwise
    /// from twelve. Short line segments: no geometry to build for a 12px dial.
    #[allow(clippy::too_many_arguments)]
    fn progress_ring(
        &self,
        t: &ID2D1DCRenderTarget,
        x: f32,
        y: f32,
        r: f32,
        progress: f32,
        tone: Rgba,
        alpha: f32,
    ) {
        self.ring(t, x, y, r, 1.8, theme::fade(self.pal.rule, alpha));
        let sweep = progress.clamp(0.0, 1.0) * std::f32::consts::TAU;
        if sweep <= 0.01 {
            return;
        }
        let steps = ((sweep / std::f32::consts::TAU) * 32.0).ceil().max(1.0) as usize;
        let at = |a: f32| {
            let a = a - std::f32::consts::FRAC_PI_2;
            (x + r * a.cos(), y + r * a.sin())
        };
        let mut from = at(0.0);
        for i in 1..=steps {
            let to = at(sweep * i as f32 / steps as f32);
            self.line(t, from.0, from.1, to.0, to.1, 1.8, theme::fade(tone, alpha));
            from = to;
        }
    }

    // -- expanded carousel --------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn paint_carousel(
        &mut self,
        t: &ID2D1DCRenderTarget,
        factory: &windows::Win32::Graphics::Direct2D::ID2D1Factory,
        cfg: &AppConfig,
        state: &NotchState,
        slides: &[SlideKind],
        shape: NotchShape,
        alpha: f32,
        generation: u64,
    ) {
        let pos = state.carousel.value;
        let width = shape.width();

        for (i, slide) in slides.iter().enumerate() {
            let delta = i as f32 - pos;
            if delta.abs() > 1.05 {
                continue; // off-stage
            }

            // Neighbours trail behind the active slide and dim out, so the
            // carousel reads as depth rather than a flat filmstrip.
            let travel = delta * width * 0.62;
            let slide_alpha = alpha * (1.0 - delta.abs()).clamp(0.0, 1.0).powf(1.4);
            if slide_alpha <= 0.006 {
                continue;
            }

            let panel = NotchShape {
                left: shape.left + travel,
                top: shape.top,
                right: shape.right + travel,
                bottom: shape.bottom,
                ..shape
            };

            self.paint_slide(
                t,
                factory,
                cfg,
                state,
                *slide,
                panel,
                slide_alpha,
                generation,
            );
        }

        if slides.len() > 1 {
            self.paint_pagination(t, cfg, slides.len(), pos, shape, alpha);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_slide(
        &mut self,
        t: &ID2D1DCRenderTarget,
        factory: &windows::Win32::Graphics::Direct2D::ID2D1Factory,
        cfg: &AppConfig,
        state: &NotchState,
        slide: SlideKind,
        panel: NotchShape,
        alpha: f32,
        generation: u64,
    ) {
        // Content rises the last few pixels as it fades in.
        let rise = (1.0 - alpha) * 12.0;
        let raw = crate::notch::geom::slide_body(panel);
        let body = D2D_RECT_F {
            top: raw.top + rise,
            bottom: raw.bottom + rise,
            ..raw
        };

        match slide {
            SlideKind::Status => self.paint_status(t, cfg, state, body, alpha),
            SlideKind::Clock => self.paint_clock(t, cfg, body, alpha),
            SlideKind::Marquee => self.paint_marquee(t, cfg, state, body, alpha),
            SlideKind::Wallpaper => {
                self.paint_wallpaper(t, factory, cfg, panel, body, alpha, generation)
            }
            SlideKind::Media => {
                self.paint_now_playing(t, factory, cfg, state, body, alpha, generation)
            }
            SlideKind::Shelf => self.paint_shelf(t, cfg, body, alpha, generation),
            SlideKind::Clipboard => self.paint_clipboard(t, cfg, body, alpha),
            SlideKind::Notifications => self.paint_notifications(t, cfg, state, body, alpha),
            SlideKind::Usage => self.paint_usage(t, cfg, body, alpha),
            SlideKind::Stats => self.paint_stats(t, cfg, body, alpha),
            SlideKind::Timer => self.paint_timer(t, cfg, body, alpha),
            SlideKind::Calendar => self.paint_calendar(t, cfg, body, alpha),
            SlideKind::Controls => self.paint_controls(t, cfg, body, alpha),
        }
    }

    /// Draw one Segoe MDL2 Assets glyph centred on `(cx, cy)`.
    fn glyph(
        &self,
        t: &ID2D1DCRenderTarget,
        glyph: &str,
        size: f32,
        cx: f32,
        cy: f32,
        color: Rgba,
    ) {
        let Ok(fmt) = self
            .text
            .format(self.icon_family, size, DWRITE_FONT_WEIGHT_MEDIUM)
        else {
            return;
        };
        let Ok(layout) = self.text.layout(glyph, &fmt, size * 2.0, size * 2.0) else {
            return;
        };
        let (w, h) = TextEngine::measure(&layout);
        self.draw_layout(t, &layout, cx - w * 0.5, cy - h * 0.5, color);
    }

    /// Lay `text` out and report its size, for callers that position it by
    /// its measured width.
    fn measure_text(
        &self,
        family: &str,
        text: &str,
        size: f32,
        weight: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT,
    ) -> Option<(IDWriteTextLayout, f32, f32)> {
        let fmt = self.text.format(family, size, weight).ok()?;
        let layout = self.text.layout(text, &fmt, 2000.0, size * 2.4).ok()?;
        let (w, h) = TextEngine::measure(&layout);
        Some((layout, w, h))
    }

    fn paint_timer(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        body: D2D_RECT_F,
        alpha: f32,
    ) {
        use crate::notch::timer::{self, Phase};
        let family = &cfg.notch.font_family;
        let view = timer::view(&cfg.notch.timer);
        let tone = if view.phase == Phase::Focus {
            cfg.notch.accent
        } else {
            BREAK_GREEN
        };

        let heading = if view.phase == Phase::Focus {
            format!("FOCUS  \u{00B7}  ROUND {} OF {}", view.round, view.rounds)
        } else {
            view.phase.label().to_owned()
        };
        self.label(
            t,
            family,
            &heading,
            theme::SIZE_LABEL,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
            theme::TRACK_LABEL,
            body.right - body.left,
            body.left,
            body.top + 2.0,
            theme::fade(tone, alpha),
        );

        // The countdown, centred and big, between the heading and buttons.
        let cx = (body.left + body.right) * 0.5;
        let buttons = crate::notch::geom::timer_buttons(body);
        let (_, button_y, button_r) = buttons[1];
        let clock_mid = (body.top + 18.0 + button_y - button_r - 22.0) * 0.5;
        if let Some((layout, w, h)) = self.measure_text(
            crate::fonts::GEIST_MONO,
            &view.clock(),
            theme::SIZE_CLOCK,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
        ) {
            let dim = if view.running { 1.0 } else { 0.62 };
            self.draw_layout(
                t,
                &layout,
                cx - w * 0.5,
                clock_mid - h * 0.5,
                theme::fade(self.pal.text_hi, alpha * dim),
            );
        }

        // Progress through the phase.
        let bar_w = ((body.right - body.left) * 0.62).min(320.0);
        let bar_y = clock_mid + theme::SIZE_CLOCK * 0.5 + 6.0;
        let bar = D2D_RECT_F {
            left: cx - bar_w * 0.5,
            top: bar_y,
            right: cx + bar_w * 0.5,
            bottom: bar_y + 4.0,
        };
        self.fill_rrect(t, bar, 2.0, theme::fade(self.pal.text_hi, alpha * 0.32));
        let done = D2D_RECT_F {
            right: bar.left + bar_w * view.progress().clamp(0.0, 1.0),
            ..bar
        };
        self.fill_rrect(t, done, 2.0, theme::fade(tone, alpha));

        // Reset / start-pause / skip.
        let glyphs = [
            "\u{E72C}",
            if view.running { "\u{E769}" } else { "\u{E768}" },
            "\u{E893}",
        ];
        for (i, ((bx, by, r), g)) in buttons.into_iter().zip(glyphs).enumerate() {
            let main = i == 1;
            let fill = if main {
                theme::fade(tone, alpha)
            } else {
                theme::fade(self.pal.well, alpha * 1.6)
            };
            self.dot(t, bx, by, r, fill);
            let ink = if main {
                [0.04, 0.04, 0.06, alpha]
            } else {
                theme::fade(self.pal.text_hi, alpha * 0.82)
            };
            self.glyph(t, g, if main { 13.0 } else { 11.0 }, bx, by, ink);
        }
    }

    fn paint_calendar(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        body: D2D_RECT_F,
        alpha: f32,
    ) {
        use crate::notch::calendar::{self, Status};
        let family = &cfg.notch.font_family;
        let now = calendar::now_local();
        let mut events = calendar::upcoming(now, 5);
        let width = body.right - body.left;
        // A meeting is what "next" means; an all-day event is context, so
        // it goes in the list below rather than taking the headline.
        if let Some(i) = events.iter().position(|e| !e.all_day) {
            let timed = events.remove(i);
            events.insert(0, timed);
        }

        let Some(next) = events.first() else {
            let (head, line) = match calendar::status() {
                Status::Unset => (
                    "CALENDAR",
                    "Add your calendar\u{2019}s iCal link in Settings \u{2192} Slides \u{2192} Calendar.",
                ),
                Status::Loading => ("CALENDAR", "Loading your calendar\u{2026}"),
                Status::Failed(_) => (
                    "CALENDAR",
                    "Couldn\u{2019}t reach your calendar. Trying again soon.",
                ),
                Status::Ready => ("ALL CLEAR", "Nothing on your calendar this week."),
            };
            self.label(
                t,
                family,
                head,
                theme::SIZE_LABEL,
                DWRITE_FONT_WEIGHT_EXTRA_BOLD,
                theme::TRACK_LABEL,
                width,
                body.left,
                body.top + 2.0,
                theme::fade(self.pal.text_lo, alpha),
            );
            self.paragraph(
                t,
                family,
                line,
                theme::SIZE_BODY + 1.0,
                width,
                body.left,
                body.top + 22.0,
                theme::fade(self.pal.text_hi, alpha),
            );
            return;
        };

        let live = next.start <= now;
        let when = if next.all_day {
            "ALL DAY".to_owned()
        } else if live {
            format!(
                "NOW  \u{00B7}  {} LEFT",
                span_text(next.end - now).to_uppercase()
            )
        } else {
            format!("IN {}", span_text(next.start - now).to_uppercase())
        };
        let tone = if live { BREAK_GREEN } else { cfg.notch.accent };
        self.label(
            t,
            family,
            &when,
            theme::SIZE_LABEL,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
            theme::TRACK_LABEL,
            width,
            body.left,
            body.top + 2.0,
            theme::fade(tone, alpha),
        );

        let title_h = self.label(
            t,
            family,
            &next.title,
            theme::SIZE_LEAD + 2.0,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            0.0,
            width,
            body.left,
            body.top + 20.0,
            theme::fade(self.pal.text_hi, alpha),
        );
        let mut detail = event_time_range(next, cfg.notch.clock_24h, now);
        if !next.location.is_empty() {
            detail = format!("{detail}  \u{00B7}  {}", next.location);
        }
        let detail_y = body.top + 24.0 + title_h;
        self.label(
            t,
            family,
            &detail,
            theme::SIZE_BODY - 1.0,
            DWRITE_FONT_WEIGHT_MEDIUM,
            0.0,
            width,
            body.left,
            detail_y,
            theme::fade(self.pal.text_mid, alpha),
        );

        // What comes after it.
        let mut y = detail_y + 28.0;
        if events.len() > 1 && y + 20.0 < body.bottom {
            self.fill_rrect(
                t,
                D2D_RECT_F {
                    left: body.left,
                    top: y,
                    right: body.right,
                    bottom: y + 1.0,
                },
                0.5,
                theme::fade(self.pal.rule, alpha),
            );
            y += 10.0;
        }
        for ev in events.iter().skip(1) {
            if y + 18.0 > body.bottom {
                break;
            }
            let time = if ev.all_day {
                format!("{} all day", day_word(ev.start, now))
            } else {
                format!(
                    "{} {}",
                    day_word(ev.start, now),
                    clock_text(ev.start, cfg.notch.clock_24h)
                )
            };
            let time_w = 118.0;
            self.label(
                t,
                crate::fonts::GEIST_MONO,
                &time,
                theme::SIZE_BODY - 2.0,
                DWRITE_FONT_WEIGHT_MEDIUM,
                0.0,
                time_w,
                body.left,
                y + 1.0,
                theme::fade(self.pal.text_lo, alpha),
            );
            self.label(
                t,
                family,
                &ev.title,
                theme::SIZE_BODY - 1.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                0.0,
                width - time_w,
                body.left + time_w,
                y,
                theme::fade(self.pal.text_hi, alpha * 0.9),
            );
            y += 22.0;
        }
    }

    fn paint_controls(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        body: D2D_RECT_F,
        alpha: f32,
    ) {
        use crate::notch::toggles::{self, Toggle};
        let family = &cfg.notch.font_family;
        let states = toggles::states();
        self.label(
            t,
            family,
            "QUICK TOGGLES",
            theme::SIZE_LABEL,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
            theme::TRACK_LABEL,
            body.right - body.left,
            body.left,
            body.top + 2.0,
            theme::fade(self.pal.text_lo, alpha),
        );

        let tiles = crate::notch::geom::toggle_tiles(body);
        for (toggle, tile) in Toggle::ALL.into_iter().zip(tiles) {
            let state = states.get(toggle);
            let on = state.unwrap_or(false);
            if on {
                self.fill_rrect(t, tile, 14.0, theme::fade(cfg.notch.accent, alpha * 0.92));
            } else {
                self.fill_rrect(t, tile, 14.0, theme::fade(self.pal.text_hi, alpha * 0.26));
                self.stroke_rrect(
                    t,
                    tile,
                    14.0,
                    1.0,
                    theme::fade(self.pal.text_hi, alpha * 0.34),
                );
            }
            let ink = if on {
                [0.04, 0.04, 0.06, alpha]
            } else {
                theme::fade(self.pal.text_hi, alpha)
            };
            let sub = if on {
                [0.04, 0.04, 0.06, alpha * 0.68]
            } else {
                theme::fade(self.pal.text_lo, alpha)
            };
            self.glyph(
                t,
                toggle.glyph(on),
                17.0,
                tile.left + 23.0,
                tile.top + 23.0,
                ink,
            );
            let text_y = tile.bottom - 38.0;
            let text_w = tile.right - tile.left - 20.0;
            self.label(
                t,
                family,
                toggle.label(),
                theme::SIZE_BODY - 1.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                0.0,
                text_w,
                tile.left + 12.0,
                text_y,
                ink,
            );
            let word = match (toggle, state) {
                (_, None) => "Unavailable",
                (Toggle::Mic | Toggle::Sound, Some(false)) => "Muted",
                (_, Some(true)) => "On",
                (_, Some(false)) => "Off",
            };
            self.label(
                t,
                family,
                word,
                theme::SIZE_LABEL,
                DWRITE_FONT_WEIGHT_MEDIUM,
                0.0,
                text_w,
                tile.left + 12.0,
                text_y + 17.0,
                sub,
            );
        }
    }

    /// Wrapped body text, for messages that may run past one line.
    #[allow(clippy::too_many_arguments)]
    fn paragraph(
        &self,
        t: &ID2D1DCRenderTarget,
        family: &str,
        text: &str,
        size: f32,
        max_w: f32,
        x: f32,
        y: f32,
        color: Rgba,
    ) {
        let Ok(fmt) = self.text.format(family, size, DWRITE_FONT_WEIGHT_SEMI_BOLD) else {
            return;
        };
        TextEngine::set_wrapping(&fmt, DWRITE_WORD_WRAPPING_WRAP);
        if let Ok(layout) = self.text.layout(text, &fmt, max_w, size * 6.0) {
            self.draw_layout(t, &layout, x, y, color);
        }
    }

    fn paint_usage(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        body: D2D_RECT_F,
        alpha: f32,
    ) {
        let family = &cfg.notch.font_family;
        let snapshot = crate::notch::notify::usage_store().read().clone();

        self.label(
            t,
            family,
            "CLAUDE CODE USAGE",
            theme::SIZE_LABEL,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
            theme::TRACK_LABEL,
            body.right - body.left,
            body.left,
            body.top,
            theme::fade(self.pal.text_lo, alpha * 0.9),
        );

        if snapshot.updated_at_secs == 0 {
            self.label(
                t,
                family,
                "Waiting for the Claude Code status line…",
                theme::SIZE_BODY,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                0.0,
                body.right - body.left,
                body.left,
                body.top + 28.0,
                theme::fade(self.pal.text_hi, alpha * 0.85),
            );
            return;
        }

        // Cost, top right
        if let Some(cost) = snapshot.cost_usd {
            self.label(
                t,
                crate::fonts::GEIST_MONO,
                &format!("${:.2} session", cost),
                theme::SIZE_LEAD - 2.0,
                DWRITE_FONT_WEIGHT_BOLD,
                0.0,
                body.right - body.left,
                body.left,
                body.top + 2.0,
                theme::fade(cfg.notch.accent, alpha),
            );
        }

        let rows: [(&str, Option<f32>, Option<&str>); 3] = [
            ("CONTEXT WINDOW", snapshot.context_used_pct, None),
            (
                "5-HOUR LIMIT",
                snapshot.rate_5h_pct,
                snapshot.rate_5h_resets_at.as_deref(),
            ),
            (
                "7-DAY LIMIT",
                snapshot.rate_7d_pct,
                snapshot.rate_7d_resets_at.as_deref(),
            ),
        ];

        let row_h = 26.0;
        let mut y = body.top + 34.0;
        let bar_w = body.right - body.left;

        for (label_text, pct, resets_at) in rows {
            let Some(pct) = pct else {
                y += row_h;
                continue;
            };

            let mut caption = format!("{}  {:.0}%", label_text, pct);
            if let Some(resets) = resets_at {
                caption.push_str("  ·  resets ");
                caption.push_str(resets);
            }

            self.label(
                t,
                crate::fonts::GEIST_MONO,
                &caption,
                theme::SIZE_LABEL - 1.0,
                DWRITE_FONT_WEIGHT_BOLD,
                theme::TRACK_LABEL * 0.5,
                bar_w,
                body.left,
                y,
                theme::fade(self.pal.text_lo, alpha * 0.9),
            );

            let bar_y = y + 15.0;
            let track = D2D_RECT_F {
                left: body.left,
                top: bar_y,
                right: body.left + bar_w,
                bottom: bar_y + 4.0,
            };
            self.fill_rrect(t, track, 2.0, theme::fade(self.pal.well, alpha * 2.4));

            let fill_color = if pct >= 90.0 {
                [0.94, 0.27, 0.27, 1.0]
            } else if pct >= 70.0 {
                [1.00, 0.65, 0.00, 1.0]
            } else {
                cfg.notch.accent
            };
            let filled = D2D_RECT_F {
                left: body.left,
                top: bar_y,
                right: body.left + bar_w * (pct / 100.0).clamp(0.0, 1.0),
                bottom: bar_y + 4.0,
            };
            self.fill_rrect(t, filled, 2.0, theme::fade(fill_color, alpha));

            y += row_h;
        }
    }

    fn paint_stats(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        body: D2D_RECT_F,
        alpha: f32,
    ) {
        let family = &cfg.notch.font_family;
        let stats = crate::stats::snapshot();
        let body_width = (body.right - body.left).max(1.0);
        let body_height = (body.bottom - body.top).max(1.0);
        let compact = body_width < 430.0 || body_height < 138.0;

        self.label(
            t,
            family,
            "YOUR PC",
            theme::SIZE_LABEL - 0.5,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
            theme::TRACK_LABEL,
            body_width * 0.55,
            body.left,
            body.top,
            theme::fade(self.pal.text_lo, alpha),
        );
        self.label(
            t,
            family,
            "Whole-system overview",
            if compact { 11.0 } else { 13.0 },
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            0.0,
            body_width * 0.62,
            body.left,
            body.top + 14.0,
            theme::fade(self.pal.text_hi, alpha),
        );

        let live_x = (body.right - 62.0).max(body.left + body_width * 0.72);
        self.dot(
            t,
            live_x + 2.0,
            body.top + 7.0,
            2.0,
            theme::fade(cfg.notch.accent, alpha * 0.82),
        );
        self.label(
            t,
            family,
            "LIVE · 1 SEC",
            8.5,
            DWRITE_FONT_WEIGHT_BOLD,
            0.55,
            52.0,
            live_x + 8.0,
            body.top + 2.0,
            theme::fade(self.pal.text_lo, alpha),
        );

        let metrics = [
            ("CPU", stats.cpu_pct, "All processors"),
            ("RAM", stats.ram_pct, "Physical memory"),
            ("GPU", stats.gpu_pct, "Busiest physical engine"),
        ];
        let column_width = body_width / 3.0;
        let metrics_top = body.top + if compact { 39.0 } else { 42.0 };
        let footer_height = 23.0;
        let metrics_bottom = (body.bottom - footer_height).max(metrics_top + 34.0);
        let number_size = if compact { 23.0 } else { 31.0 };

        for (index, (name, value, description)) in metrics.into_iter().enumerate() {
            let left = body.left + index as f32 * column_width;
            let inset = if compact { 5.0 } else { 11.0 };
            let content_width = (column_width - inset * 2.0).max(1.0);
            self.label(
                t,
                family,
                name,
                theme::SIZE_LABEL - 1.0,
                DWRITE_FONT_WEIGHT_EXTRA_BOLD,
                theme::TRACK_LABEL * 0.75,
                content_width,
                left + inset,
                metrics_top,
                theme::fade(self.pal.text_lo, alpha),
            );

            let value_text = value
                .map(|pct| format!("{:.0}%", pct.clamp(0.0, 100.0)))
                .unwrap_or_else(|| "—".to_string());
            if let Ok(format) = self.text.format(
                crate::fonts::GEIST_MONO,
                number_size,
                DWRITE_FONT_WEIGHT_MEDIUM,
            ) {
                self.text.set_ellipsis(&format);
                if let Ok(layout) =
                    self.text
                        .layout(&value_text, &format, content_width, number_size * 1.5)
                {
                    let (_, height) = TextEngine::measure(&layout);
                    self.draw_layout(
                        t,
                        &layout,
                        left + inset,
                        metrics_top + 13.0 + (number_size * 1.1 - height).max(0.0) * 0.5,
                        theme::fade(self.pal.text_hi, alpha),
                    );
                }
            }

            self.label(
                t,
                family,
                description,
                if compact { 8.5 } else { 10.0 },
                DWRITE_FONT_WEIGHT_MEDIUM,
                0.0,
                content_width,
                left + inset,
                metrics_top + if compact { 43.0 } else { 51.0 },
                theme::fade(self.pal.text_mid, alpha),
            );

            if let Some(pct) = value {
                let bar_y = metrics_bottom - 7.0;
                let track = D2D_RECT_F {
                    left: left + inset,
                    top: bar_y,
                    right: left + column_width - inset,
                    bottom: bar_y + 2.0,
                };
                self.fill_rrect(t, track, 1.0, theme::fade(self.pal.rule, alpha));
                self.fill_rrect(
                    t,
                    D2D_RECT_F {
                        right: track.left
                            + (track.right - track.left) * (pct / 100.0).clamp(0.0, 1.0),
                        ..track
                    },
                    1.0,
                    theme::fade(cfg.notch.accent, alpha),
                );
            }

            if index < 2 {
                let x = body.left + (index + 1) as f32 * column_width;
                self.fill_rrect(
                    t,
                    D2D_RECT_F {
                        left: x,
                        top: metrics_top + 2.0,
                        right: x + 1.0,
                        bottom: metrics_bottom - 3.0,
                    },
                    0.5,
                    theme::fade(self.pal.rule, alpha * 0.95),
                );
            }
        }

        let footer_y = body.bottom - 21.0;
        self.fill_rrect(
            t,
            D2D_RECT_F {
                left: body.left,
                top: footer_y - 3.0,
                right: body.right,
                bottom: footer_y - 2.0,
            },
            0.5,
            theme::fade(self.pal.rule, alpha),
        );
        let memory = if stats.ram_total > 0 {
            format!(
                "RAM {} / {}",
                crate::stats::format_bytes(stats.ram_used),
                crate::stats::format_bytes(stats.ram_total)
            )
        } else {
            "RAM —".to_string()
        };
        let footer = format!("{memory}   ·   {}", stats.power_value());
        self.label(
            t,
            crate::fonts::GEIST_MONO,
            &footer,
            if body_width < 390.0 { 8.2 } else { 9.2 },
            DWRITE_FONT_WEIGHT_MEDIUM,
            0.0,
            body_width,
            body.left,
            footer_y,
            theme::fade(self.pal.text_mid, alpha),
        );
    }

    fn paint_notifications(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        state: &NotchState,
        body: D2D_RECT_F,
        alpha: f32,
    ) {
        let family = &cfg.notch.font_family;
        let notif_store = crate::notch::notify::global_store();
        let store = notif_store.read();

        // If an item is selected for detailed inspection, render its expanded card in place:
        if let Some(sel_id) = state.selected_notification_id {
            if let Some(item) = store.items.iter().find(|i| i.id == sel_id) {
                // Header: Back Button + App Badge
                let back_rect = crate::notch::geom::notification_back_button(body);
                self.fill_rrect(t, back_rect, 4.0, theme::fade(self.pal.well, alpha * 0.9));
                self.stroke_rrect(
                    t,
                    back_rect,
                    4.0,
                    1.0,
                    theme::fade(self.pal.rule, alpha * 0.8),
                );
                self.label(
                    t,
                    family,
                    "← Back",
                    theme::SIZE_LABEL - 0.5,
                    DWRITE_FONT_WEIGHT_BOLD,
                    0.0,
                    50.0,
                    back_rect.left + 8.0,
                    back_rect.top + 2.0,
                    theme::fade(self.pal.text_hi, alpha),
                );

                let app_color = cfg.notch.notifications.get_app_color(&item.app);
                let app_pill = D2D_RECT_F {
                    left: back_rect.right + 10.0,
                    top: body.top - 2.0,
                    right: back_rect.right + 94.0,
                    bottom: body.top + 18.0,
                };
                self.fill_rrect(t, app_pill, 4.0, theme::fade(app_color, alpha * 0.22));
                self.stroke_rrect(t, app_pill, 4.0, 1.0, theme::fade(app_color, alpha * 0.6));
                self.label(
                    t,
                    family,
                    &item.app.to_uppercase(),
                    theme::SIZE_LABEL - 2.0,
                    DWRITE_FONT_WEIGHT_BOLD,
                    0.5,
                    78.0,
                    app_pill.left + 6.0,
                    app_pill.top + 1.0,
                    theme::fade(app_color, alpha),
                );

                self.label(
                    t,
                    family,
                    &item.time_str,
                    theme::SIZE_LABEL - 1.0,
                    DWRITE_FONT_WEIGHT_MEDIUM,
                    0.0,
                    80.0,
                    app_pill.right + 12.0,
                    body.top + 1.0,
                    theme::fade(self.pal.text_lo, alpha * 0.8),
                );

                // Notification Title
                self.label(
                    t,
                    family,
                    &item.title,
                    theme::SIZE_LEAD - 1.0,
                    DWRITE_FONT_WEIGHT_BOLD,
                    0.0,
                    body.right - body.left,
                    body.left,
                    body.top + 28.0,
                    theme::fade(self.pal.text_hi, alpha),
                );

                // Detailed Body Card inside rounded glass well
                let content_rect = D2D_RECT_F {
                    left: body.left,
                    top: body.top + 54.0,
                    right: body.right,
                    bottom: body.bottom - 26.0,
                };
                self.fill_rrect(
                    t,
                    content_rect,
                    8.0,
                    theme::fade(self.pal.well, alpha * 0.75),
                );
                self.stroke_rrect(
                    t,
                    content_rect,
                    8.0,
                    1.0,
                    theme::fade(self.pal.rule, alpha * 0.5),
                );

                let Ok(fmt) = self
                    .text
                    .format(family, theme::SIZE_BODY, DWRITE_FONT_WEIGHT_MEDIUM)
                else {
                    return;
                };
                let avail_w = (content_rect.right - content_rect.left - 24.0).max(20.0);
                let avail_h = (content_rect.bottom - content_rect.top - 16.0).max(20.0);
                if let Ok(layout) = self.text.layout(&item.body, &fmt, avail_w, avail_h) {
                    self.draw_layout(
                        t,
                        &layout,
                        content_rect.left + 12.0,
                        content_rect.top + 10.0,
                        theme::fade(self.pal.text_mid, alpha),
                    );
                }

                // Dismiss Button at bottom
                let dismiss_rect = crate::notch::geom::notification_dismiss_button(body);
                self.fill_rrect(t, dismiss_rect, 5.0, theme::fade(app_color, alpha * 0.18));
                self.stroke_rrect(
                    t,
                    dismiss_rect,
                    5.0,
                    1.0,
                    theme::fade(app_color, alpha * 0.5),
                );
                self.label(
                    t,
                    family,
                    "Dismiss",
                    theme::SIZE_LABEL,
                    DWRITE_FONT_WEIGHT_BOLD,
                    0.0,
                    60.0,
                    dismiss_rect.left + 12.0,
                    dismiss_rect.top + 3.0,
                    theme::fade(app_color, alpha),
                );
                return;
            }
        }

        // Header: "NOTIFICATIONS" and "Clear All"
        self.dot(
            t,
            body.left + 3.0,
            body.top + 6.0,
            3.0,
            theme::fade([0.22, 0.74, 0.97, 1.0], alpha * self.pulse(state)),
        );

        let unread = store.unread_count();
        let heading = if unread > 0 {
            format!("NOTIFICATIONS  ·  {} NEW", unread)
        } else {
            "NOTIFICATIONS".to_string()
        };

        self.label(
            t,
            family,
            &heading,
            theme::SIZE_LABEL,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
            theme::TRACK_LABEL,
            body.right - body.left - 100.0,
            body.left + 14.0,
            body.top + 1.0,
            theme::fade([0.22, 0.74, 0.97, 1.0], alpha),
        );

        // "Clear all" button in header
        if !store.items.is_empty() {
            let clr_rect = crate::notch::geom::notification_clear_button(body);
            self.label(
                t,
                family,
                "Clear all",
                theme::SIZE_LABEL - 0.5,
                DWRITE_FONT_WEIGHT_BOLD,
                0.2,
                64.0,
                clr_rect.left,
                clr_rect.top + 2.0,
                theme::fade(self.pal.text_lo, alpha * 0.9),
            );
        }

        // Body area
        let list_top = body.top + 26.0;
        if store.items.is_empty() {
            // Empty state
            let center_y = (list_top + body.bottom) * 0.5;
            let cx = (body.left + body.right) * 0.5;

            // Soft empty badge
            self.dot(
                t,
                cx,
                center_y - 14.0,
                14.0,
                theme::fade(self.pal.well, alpha),
            );
            self.ring(
                t,
                cx,
                center_y - 14.0,
                14.0,
                1.2,
                theme::fade(self.pal.rule, alpha),
            );
            self.dot(
                t,
                cx,
                center_y - 14.0,
                3.5,
                theme::fade([0.22, 0.74, 0.97, 1.0], alpha),
            );

            self.label(
                t,
                family,
                "All caught up",
                theme::SIZE_LEAD - 2.0,
                DWRITE_FONT_WEIGHT_BOLD,
                0.0,
                body.right - body.left,
                cx - 45.0,
                center_y + 4.0,
                theme::fade(self.pal.text_hi, alpha),
            );
            self.label(
                t,
                family,
                "Notifications from Antigravity, Codex, Claude & allowed apps will appear here",
                theme::SIZE_LABEL,
                DWRITE_FONT_WEIGHT_MEDIUM,
                0.0,
                body.right - body.left - 40.0,
                body.left + 20.0,
                center_y + 24.0,
                theme::fade(self.pal.text_lo, alpha * 0.8),
            );
        } else {
            // Shared responsive row geometry keeps all four recent entries
            // inside the expanded panel and aligned with click hit-testing.
            let max_items = crate::notch::geom::notification_row_count(body).min(store.items.len());

            for (i, item) in store.items.iter().take(max_items).enumerate() {
                let item_rect = crate::notch::geom::notification_item_rect(body, i);
                let item_h = item_rect.bottom - item_rect.top;

                // Card background well
                self.fill_rrect(t, item_rect, 8.0, theme::fade(self.pal.well, alpha * 0.75));
                self.stroke_rrect(
                    t,
                    item_rect,
                    8.0,
                    1.0,
                    theme::fade(self.pal.rule, alpha * 0.6),
                );

                // App pill badge
                let app_color = cfg.notch.notifications.get_app_color(&item.app);

                let pill_rect = D2D_RECT_F {
                    left: item_rect.left + 8.0,
                    top: item_rect.top + ((item_h - 14.0) * 0.5).max(1.0),
                    right: item_rect.left + 82.0,
                    bottom: item_rect.top + ((item_h - 14.0) * 0.5).max(1.0) + 14.0,
                };
                self.fill_rrect(t, pill_rect, 4.0, theme::fade(app_color, alpha * 0.18));
                self.stroke_rrect(t, pill_rect, 4.0, 1.0, theme::fade(app_color, alpha * 0.5));

                self.label(
                    t,
                    family,
                    &item.app.to_uppercase(),
                    theme::SIZE_LABEL - 2.0,
                    DWRITE_FONT_WEIGHT_BOLD,
                    0.5,
                    70.0,
                    pill_rect.left + 6.0,
                    pill_rect.top + 1.0,
                    theme::fade(app_color, alpha),
                );

                // Time string
                self.label(
                    t,
                    crate::fonts::GEIST_MONO,
                    &item.time_str,
                    9.0,
                    DWRITE_FONT_WEIGHT_MEDIUM,
                    0.0,
                    64.0,
                    item_rect.right - 64.0,
                    item_rect.top + (item_h - 11.0) * 0.5,
                    theme::fade(self.pal.text_lo, alpha * 0.7),
                );

                // Notification Title
                let text_left = item_rect.left + 90.0;
                let text_width = (item_rect.right - text_left - 70.0).max(20.0);
                self.label(
                    t,
                    family,
                    &item.title,
                    theme::SIZE_LABEL,
                    DWRITE_FONT_WEIGHT_BOLD,
                    0.0,
                    text_width,
                    text_left,
                    item_rect.top + 3.5,
                    theme::fade(self.pal.text_hi, alpha),
                );

                // Notification Body text
                self.label(
                    t,
                    family,
                    &item.body,
                    9.5,
                    DWRITE_FONT_WEIGHT_MEDIUM,
                    0.0,
                    text_width,
                    text_left,
                    item_rect.top + item_h - 12.0,
                    theme::fade(self.pal.text_mid, alpha * 0.9),
                );

                if !item.read {
                    // Glowing unread cyan dot
                    self.dot(
                        t,
                        item_rect.left + 4.0,
                        item_rect.top + 4.0,
                        2.5,
                        theme::fade([0.22, 0.74, 0.97, 1.0], alpha),
                    );
                }
            }
        }
    }

    fn paint_status(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        state: &NotchState,
        body: D2D_RECT_F,
        alpha: f32,
    ) {
        let family = &cfg.notch.font_family;
        let full_w = body.right - body.left;
        let split = body.left + full_w * theme::SPLIT_RATIO;
        let left_w = (split - body.left - 18.0).max(40.0);

        // --- left: what right now is about ---------------------------------
        let heading = if cfg.status.heading.trim().is_empty() {
            "TODAY"
        } else {
            cfg.status.heading.trim()
        };

        self.dot(
            t,
            body.left + 3.0,
            body.top + 6.0,
            3.0,
            theme::fade(cfg.notch.accent, alpha * self.pulse(state)),
        );
        self.label(
            t,
            family,
            heading,
            theme::SIZE_LABEL,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
            theme::TRACK_LABEL,
            left_w - 16.0,
            body.left + 14.0,
            body.top,
            theme::fade(self.pal.text_lo, alpha),
        );

        let focus_y = body.top + 26.0;
        let focus_h = (body.bottom - focus_y - 20.0).max(20.0);
        let focus = state.display_focus(cfg);

        if let Ok(fmt) = self
            .text
            .format(family, theme::SIZE_LEAD, DWRITE_FONT_WEIGHT_SEMI_BOLD)
        {
            TextEngine::set_wrapping(&fmt, DWRITE_WORD_WRAPPING_WRAP);
            self.text.set_ellipsis(&fmt);
            if let Ok(layout) = self.text.layout(&focus, &fmt, left_w, focus_h) {
                // The "what are you working on?" prompt is a placeholder, not
                // content, so it sits back from a real focus line.
                let placeholder = !state.editing && cfg.status.focus.trim().is_empty();
                let color = if placeholder {
                    self.pal.text_lo
                } else {
                    self.pal.text_hi
                };
                self.draw_layout(t, &layout, body.left, focus_y, theme::fade(color, alpha));

                // Caret + underline while editing, so the notch clearly has
                // keyboard focus without borrowing a system text field.
                if state.editing && !state.adding_item() {
                    let (tw, th) = TextEngine::measure(&layout);
                    let caret_x = (body.left + tw.min(left_w) + 2.0).min(body.left + left_w);
                    let caret_top = focus_y + (th - theme::SIZE_LEAD * 1.15).max(0.0);
                    if state.caret_on {
                        let caret = D2D_RECT_F {
                            left: caret_x,
                            top: caret_top,
                            right: caret_x + 2.0,
                            bottom: caret_top + theme::SIZE_LEAD * 1.15,
                        };
                        self.fill_rrect(t, caret, 1.0, theme::fade(cfg.notch.accent, alpha));
                    }
                    let rule = D2D_RECT_F {
                        left: body.left,
                        top: focus_y + th + 6.0,
                        right: body.left + left_w,
                        bottom: focus_y + th + 7.2,
                    };
                    self.fill_rrect(t, rule, 0.6, theme::fade(cfg.notch.accent, alpha * 0.55));
                }
            }
        }

        // Affordance, only once the panel is fully open.
        let hint_alpha = alpha * smoothstep(0.85, 1.0, alpha);
        if hint_alpha > 0.01 {
            let hint = if state.editing {
                "ENTER TO SAVE   ESC TO CANCEL"
            } else {
                "CLICK TO EDIT"
            };
            self.label(
                t,
                family,
                hint,
                theme::SIZE_LABEL - 1.0,
                DWRITE_FONT_WEIGHT_BOLD,
                theme::TRACK_LABEL,
                left_w,
                body.left,
                body.bottom - 11.0,
                theme::fade(self.pal.text_lo, hint_alpha * 0.85),
            );
        }

        // --- divider -------------------------------------------------------
        let rule = D2D_RECT_F {
            left: split - 0.6,
            top: body.top - 2.0,
            right: split + 0.6,
            bottom: body.bottom + 2.0,
        };
        self.fill_rrect(t, rule, 0.6, theme::fade(self.pal.rule, alpha));

        // --- right: the short list -----------------------------------------
        let rx = split + 20.0;
        let rw = (body.right - rx).max(40.0);

        self.label(
            t,
            family,
            "ON DECK",
            theme::SIZE_LABEL,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
            theme::TRACK_LABEL,
            rw - 30.0,
            rx,
            body.top,
            theme::fade(self.pal.text_lo, alpha),
        );
        // Add a task: a round + in the accent, at the end of the heading.
        if cfg.status.items.len() < crate::notch::layout::STATUS_MAX_ITEMS {
            self.chip_button(
                t,
                cfg,
                crate::notch::geom::status_plus_button(body),
                crate::notch::menu::glyph::ADD,
                true,
                alpha,
            );
        }

        let column = crate::notch::geom::status_list_column(body);
        let list = crate::notch::layout::status_list(column, cfg.status.items.len());

        for (row, item) in list.items.iter().zip(cfg.status.items.iter()) {
            let y = row.top;
            let cy = y + crate::notch::layout::STATUS_ROW_H * 0.5 - 3.0;

            // Marker: filled ember pip when done, hollow ring when not.
            if item.done {
                self.dot(
                    t,
                    rx + 4.0,
                    cy,
                    4.0,
                    theme::fade(cfg.notch.accent, alpha * 0.9),
                );
            } else {
                self.dot(
                    t,
                    rx + 4.0,
                    cy,
                    4.0,
                    theme::fade(self.pal.well, alpha * 3.0),
                );
                self.dot(t, rx + 4.0, cy, 2.4, theme::fade(cfg.notch.surface, alpha));
            }

            let color = if item.done {
                theme::fade(self.pal.text_lo, alpha)
            } else {
                theme::fade(self.pal.text_mid, alpha)
            };

            self.label(
                t,
                family,
                &item.text,
                theme::SIZE_BODY,
                DWRITE_FONT_WEIGHT_MEDIUM,
                0.0,
                rw - 18.0,
                rx + 16.0,
                y + 1.0,
                color,
            );
        }

        // The add row: a quiet "+ Add" until clicked, then a text field.
        if let Some(row) = list.add {
            let cy = row.top + crate::notch::layout::STATUS_ROW_H * 0.5 - 3.0;
            let adding = state.adding_item();
            let tone = if adding {
                cfg.notch.accent
            } else {
                self.pal.text_lo
            };
            self.label(
                t,
                family,
                "+",
                theme::SIZE_BODY + 2.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                0.0,
                12.0,
                rx,
                cy - 9.0,
                theme::fade(tone, alpha),
            );
            let text = if adding {
                state.edit_buffer.as_str()
            } else if list.overflow > 0 {
                ""
            } else {
                "Add to the list"
            };
            let text_color = if adding {
                self.pal.text_hi
            } else {
                self.pal.text_lo
            };
            let mut caret_x = rx + 16.0;
            if !text.is_empty() {
                if let Some((layout, w, _)) =
                    self.measure_text(family, text, theme::SIZE_BODY, DWRITE_FONT_WEIGHT_MEDIUM)
                {
                    self.draw_layout(
                        t,
                        &layout,
                        rx + 16.0,
                        row.top + 1.0,
                        theme::fade(text_color, alpha),
                    );
                    caret_x += w.min(rw - 24.0) + 2.0;
                }
            }
            if adding && state.caret_on {
                self.fill_rrect(
                    t,
                    D2D_RECT_F {
                        left: caret_x,
                        top: row.top + 2.0,
                        right: caret_x + 2.0,
                        bottom: row.top + 2.0 + theme::SIZE_BODY * 1.2,
                    },
                    1.0,
                    theme::fade(cfg.notch.accent, alpha),
                );
            }
        }

        if list.overflow > 0 && !state.adding_item() {
            let y = list.add.map(|r| r.top + 3.0).unwrap_or(body.bottom - 11.0);
            self.label(
                t,
                family,
                &format!("+{} MORE", list.overflow),
                theme::SIZE_LABEL - 1.0,
                DWRITE_FONT_WEIGHT_BOLD,
                theme::TRACK_LABEL,
                rw,
                if list.add.is_some() {
                    rx + 34.0
                } else {
                    rx + 16.0
                },
                y,
                theme::fade(self.pal.text_lo, alpha * 0.85),
            );
        }
    }

    fn paint_clock(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        body: D2D_RECT_F,
        alpha: f32,
    ) {
        let family = &cfg.notch.font_family;
        let clock = Clock::now();
        let time = clock.time_string(cfg.notch.clock_24h);

        // --- oversized time -------------------------------------------------
        let mut time_w = 0.0;
        let mut time_h = theme::SIZE_CLOCK;
        if let Ok(fmt) = self.text.format(
            crate::fonts::GEIST_MONO,
            theme::SIZE_CLOCK,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
        ) {
            if let Ok(layout) =
                self.text
                    .layout(&time, &fmt, body.right - body.left, body.bottom - body.top)
            {
                let (w, h) = TextEngine::measure(&layout);
                time_w = w;
                time_h = h;
                // Optically centred: big numerals carry more weight below the
                // x-height, so nudge up rather than centring the box.
                self.draw_layout(
                    t,
                    &layout,
                    body.left,
                    body.top + (body.bottom - body.top - h) * 0.5 - 8.0,
                    theme::fade(self.pal.text_hi, alpha),
                );
            }
        }

        let time_top = body.top + (body.bottom - body.top - time_h) * 0.5 - 8.0;

        if !cfg.notch.clock_24h {
            self.label(
                t,
                family,
                clock.meridiem(),
                theme::SIZE_LABEL + 1.5,
                DWRITE_FONT_WEIGHT_EXTRA_BOLD,
                theme::TRACK_LABEL,
                60.0,
                body.left + time_w + 10.0,
                time_top + time_h * 0.30,
                theme::fade(cfg.notch.accent, alpha),
            );
        }

        // --- right rail: date + how much of the day is gone -----------------
        let rail_x = body.left + (body.right - body.left) * 0.56;
        let rail_w = (body.right - rail_x).max(60.0);

        let weekday = WEEKDAYS[(clock.weekday as usize).min(6)];
        let month = MONTHS[((clock.month as usize).max(1) - 1).min(11)];

        self.label(
            t,
            family,
            weekday,
            theme::SIZE_LABEL,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
            theme::TRACK_LABEL,
            rail_w,
            rail_x,
            body.top + 2.0,
            theme::fade(self.pal.text_lo, alpha),
        );

        self.label(
            t,
            family,
            &format!("{} {}", month, clock.day),
            theme::SIZE_LEAD + 4.0,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            0.0,
            rail_w,
            rail_x,
            body.top + 20.0,
            theme::fade(self.pal.text_hi, alpha),
        );

        let frac = clock.day_fraction();
        let bar_y = body.bottom - 26.0;
        let bar = D2D_RECT_F {
            left: rail_x,
            top: bar_y,
            right: rail_x + rail_w,
            bottom: bar_y + 4.0,
        };
        self.fill_rrect(t, bar, 2.0, theme::fade(self.pal.well, alpha * 2.4));

        let filled = D2D_RECT_F {
            left: rail_x,
            top: bar_y,
            right: rail_x + rail_w * frac.clamp(0.0, 1.0),
            bottom: bar_y + 4.0,
        };
        self.fill_rrect(t, filled, 2.0, theme::fade(cfg.notch.accent, alpha));

        self.label(
            t,
            family,
            &format!("{}% OF TODAY GONE", (frac * 100.0).round() as i32),
            theme::SIZE_LABEL - 1.0,
            DWRITE_FONT_WEIGHT_BOLD,
            theme::TRACK_LABEL,
            rail_w,
            rail_x,
            bar_y + 10.0,
            theme::fade(self.pal.text_lo, alpha * 0.9),
        );
    }

    fn paint_marquee(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        state: &NotchState,
        body: D2D_RECT_F,
        alpha: f32,
    ) {
        let family = &cfg.notch.font_family;

        self.label(
            t,
            family,
            "MOVING TEXT",
            theme::SIZE_LABEL,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
            theme::TRACK_LABEL,
            body.right - body.left,
            body.left,
            body.top,
            theme::fade(self.pal.text_lo, alpha),
        );

        let strip = D2D_RECT_F {
            left: body.left - theme::GUTTER * 0.5,
            top: body.top + 30.0,
            right: body.right + theme::GUTTER * 0.5,
            bottom: body.bottom - 6.0,
        };
        self.paint_marquee_strip(
            t,
            cfg,
            state,
            strip,
            cfg.marquee.panel_font(theme::SIZE_LEAD + 4.0),
            alpha,
            cfg.notch.surface,
        );
    }

    /// The text band, shared by the collapsed pill and the expanded slide.
    ///
    /// Scrolling, it repeats the phrase enough times to cover the strip and
    /// fades out into the panel colour at both ends, so text never appears to
    /// be sliced off by the clip rectangle. Held still, it is one centred copy
    /// with an ellipsis if it overruns — and no edge fades, because nothing is
    /// sliding past the edge for them to explain.
    #[allow(clippy::too_many_arguments)]
    fn paint_marquee_strip(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        state: &NotchState,
        strip: D2D_RECT_F,
        size: f32,
        alpha: f32,
        fade_to: Rgba,
    ) {
        let family = &cfg.notch.font_family;
        let width = strip.right - strip.left;
        let height = strip.bottom - strip.top;
        if width <= 4.0 || height <= 2.0 {
            return;
        }

        let spacing = " ".repeat(cfg.phrase_spacing.max(1) as usize);
        let phrase = format!("{}{}", cfg.text, spacing);

        let Ok(fmt) = self.text.format(family, size, DWRITE_FONT_WEIGHT_SEMI_BOLD) else {
            return;
        };

        if !cfg.marquee.scroll {
            self.text.set_ellipsis(&fmt);
            // The repetition spacing is padding *between* copies, so a static
            // line neither needs it nor should be centred as though it had it.
            let Ok(layout) = self.text.layout(cfg.text.trim(), &fmt, width, height * 4.0) else {
                return;
            };
            let (tw, th) = TextEngine::measure(&layout);

            self.clip(t, strip);
            self.draw_layout(
                t,
                &layout,
                strip.left + (width - tw) * 0.5,
                strip.top + (height - th) * 0.5,
                theme::fade(cfg.colors.text_color, alpha),
            );
            self.unclip(t);
            return;
        }

        // Measure one phrase so the loop is seamless regardless of content.
        let Ok(probe) = self.text.layout(&phrase, &fmt, 100_000.0, height * 4.0) else {
            return;
        };
        let phrase_w = TextEngine::measure(&probe).0.max(1.0);

        let repeats = ((width / phrase_w).ceil() as usize + 2).clamp(2, 400);
        let full: String = phrase.repeat(repeats);

        let Ok(layout) = self.text.layout(&full, &fmt, 100_000.0, height * 4.0) else {
            return;
        };
        let (_, th) = TextEngine::measure(&layout);

        let scroll = state.marquee_offset.rem_euclid(phrase_w);
        let y = strip.top + (height - th) * 0.5;

        self.clip(t, strip);
        self.draw_layout(
            t,
            &layout,
            strip.left - scroll,
            y,
            theme::fade(cfg.colors.text_color, alpha),
        );

        // Edge fades, painted in the panel's own colour.
        let fade_w = (width * 0.10).clamp(10.0, 48.0);
        let opaque = [fade_to[0], fade_to[1], fade_to[2], fade_to[3] * alpha];
        let clear = [fade_to[0], fade_to[1], fade_to[2], 0.0];
        self.hgradient(
            t,
            D2D_RECT_F {
                left: strip.left,
                top: strip.top,
                right: strip.left + fade_w,
                bottom: strip.bottom,
            },
            opaque,
            clear,
        );
        self.hgradient(
            t,
            D2D_RECT_F {
                left: strip.right - fade_w,
                top: strip.top,
                right: strip.right,
                bottom: strip.bottom,
            },
            clear,
            opaque,
        );
        self.unclip(t);
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_wallpaper(
        &mut self,
        t: &ID2D1DCRenderTarget,
        _factory: &windows::Win32::Graphics::Direct2D::ID2D1Factory,
        cfg: &AppConfig,
        panel: NotchShape,
        body: D2D_RECT_F,
        alpha: f32,
        generation: u64,
    ) {
        let family = &cfg.notch.font_family;
        let inset = 10.0;
        let art = D2D_RECT_F {
            left: panel.left + inset,
            top: panel.top + inset,
            right: panel.right - inset,
            bottom: panel.bottom - inset,
        };
        let radius = (panel.radius_bottom - inset * 0.5).max(6.0);

        let drawn = self.fill_with_image(t, cfg, art, radius, alpha, generation);

        if !drawn {
            // Placeholder: an ember wash so the slide still reads as a picture
            // frame rather than an error state.
            self.fill_rrect(t, art, radius, theme::fade(self.pal.well, alpha * 2.0));
            self.label(
                t,
                family,
                "NO IMAGE SET",
                theme::SIZE_LABEL,
                DWRITE_FONT_WEIGHT_EXTRA_BOLD,
                theme::TRACK_LABEL,
                art.right - art.left - 32.0,
                art.left + 18.0,
                art.top + 16.0,
                theme::fade(cfg.notch.accent, alpha),
            );
            self.label(
                t,
                family,
                "Choose one under Settings \u{203A} Notch",
                theme::SIZE_BODY,
                DWRITE_FONT_WEIGHT_MEDIUM,
                0.0,
                art.right - art.left - 32.0,
                art.left + 18.0,
                art.top + 34.0,
                theme::fade(self.pal.text_mid, alpha),
            );
        }

        let caption = cfg.wallpaper.caption.trim();
        if !caption.is_empty() {
            // Scrim so the caption survives a bright photo.
            let scrim_h = 62.0_f32.min((art.bottom - art.top) * 0.6);
            self.clip(t, art);
            self.vgradient(
                t,
                D2D_RECT_F {
                    left: art.left,
                    top: art.bottom - scrim_h,
                    right: art.right,
                    bottom: art.bottom,
                },
                theme::fade(self.pal.scrim, 0.0),
                theme::fade(self.pal.scrim, 0.80 * alpha),
            );
            self.unclip(t);

            self.label(
                t,
                family,
                caption,
                theme::SIZE_LEAD,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                0.0,
                art.right - art.left - 36.0,
                art.left + 18.0,
                art.bottom - 34.0,
                theme::fade(self.pal.text_hi, alpha),
            );
        }

        let _ = body;
    }

    /// Decode-on-demand, cache, and paint the wallpaper as a rounded fill.
    /// Returns `false` when there is nothing to draw.
    fn fill_with_image(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        art: D2D_RECT_F,
        radius: f32,
        alpha: f32,
        generation: u64,
    ) -> bool {
        let path = cfg.wallpaper.path.trim().to_string();
        if path.is_empty() {
            return false;
        }
        if self.failed_path.as_deref() == Some(path.as_str()) {
            return false;
        }

        let stale = match &self.image {
            Some(img) => img.path != path || img.generation != generation,
            None => true,
        };

        if stale {
            match self.decode(t, &path, generation) {
                Some(img) => {
                    self.image = Some(img);
                    self.failed_path = None;
                }
                None => {
                    eprintln!("[notch] could not decode wallpaper: {path}");
                    self.failed_path = Some(path);
                    self.image = None;
                    return false;
                }
            }
        }

        let Some(img) = &self.image else {
            return false;
        };

        let size = unsafe { img.bitmap.GetSize() };
        if size.width <= 0.0 || size.height <= 0.0 {
            return false;
        }

        // Cover-fit, then push the overflow around by the focal point: at 0.5
        // the crop is centred, at 0 the left/top edge is flush, at 1 the
        // right/bottom edge is. Zoom scales past the cover fit for a tighter
        // crop on the same point.
        let (focus_x, focus_y, zoom) = cfg.wallpaper.sanitised();
        let dst_w = art.right - art.left;
        let dst_h = art.bottom - art.top;
        let scale = (dst_w / size.width).max(dst_h / size.height) * zoom;
        let draw_w = size.width * scale;
        let draw_h = size.height * scale;
        let ox = art.left - (draw_w - dst_w) * focus_x;
        let oy = art.top - (draw_h - dst_h) * focus_y;

        let props = D2D1_BITMAP_BRUSH_PROPERTIES {
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
        };

        let brush: ID2D1BitmapBrush =
            match unsafe { t.CreateBitmapBrush(&img.bitmap, Some(&props), None) } {
                Ok(b) => b,
                Err(_) => return false,
            };

        unsafe {
            // Scale *then* translate. `A * B` in this crate means "apply A,
            // then B", so the other order would scale the offset too and slide
            // the crop away from the focal point the user chose.
            brush.SetTransform(&(scale_matrix(scale) * Matrix3x2::translation(ox, oy)));
            brush.SetOpacity(alpha);
        }

        let rr = D2D1_ROUNDED_RECT {
            rect: art,
            radiusX: radius,
            radiusY: radius,
        };
        unsafe { t.FillRoundedRectangle(&rr, &brush) };
        true
    }

    fn decode(&self, t: &ID2D1DCRenderTarget, path: &str, generation: u64) -> Option<CachedImage> {
        let wic = self.wic.as_ref()?;
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

        unsafe {
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
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeMedianCut,
                )
                .ok()?;

            let bitmap = t.CreateBitmapFromWicBitmap(&converter, None).ok()?;

            Some(CachedImage {
                path: path.to_string(),
                generation,
                bitmap: bitmap.cast().ok()?,
            })
        }
    }

    /// Three transport buttons — previous, play/pause, next — drawn as
    /// hand-built vector glyphs rather than font symbols, so their shape never
    /// depends on which glyphs happen to be in the notch's font stack.
    fn paint_media_controls(
        &self,
        t: &ID2D1DCRenderTarget,
        factory: &windows::Win32::Graphics::Direct2D::ID2D1Factory,
        cfg: &AppConfig,
        body: D2D_RECT_F,
        playing: bool,
        alpha: f32,
    ) {
        let [prev, toggle, next] = crate::notch::geom::media_transport_buttons(body);

        for (cx, cy, r) in [prev, toggle, next] {
            self.dot(t, cx, cy, r, theme::fade(self.pal.well, alpha * 1.6));
        }

        let dim = theme::fade(self.pal.text_hi, alpha * 0.82);
        self.skip_icon(t, factory, prev, dim, true);
        self.skip_icon(t, factory, next, dim, false);

        let accent = theme::fade(cfg.notch.accent, alpha);
        if playing {
            self.pause_icon(t, toggle, accent);
        } else {
            self.play_icon(t, factory, toggle, accent);
        }
    }

    fn play_icon(
        &self,
        t: &ID2D1DCRenderTarget,
        factory: &windows::Win32::Graphics::Direct2D::ID2D1Factory,
        (cx, cy, r): (f32, f32, f32),
        color: Rgba,
    ) {
        let s = r * 0.55;
        let pts = [
            D2D_POINT_2F {
                x: cx - s * 0.7,
                y: cy - s,
            },
            D2D_POINT_2F {
                x: cx - s * 0.7,
                y: cy + s,
            },
            D2D_POINT_2F {
                x: cx + s * 0.9,
                y: cy,
            },
        ];
        self.fill_triangle(t, factory, pts, color);
    }

    fn pause_icon(&self, t: &ID2D1DCRenderTarget, (cx, cy, r): (f32, f32, f32), color: Rgba) {
        let s = r * 0.55;
        let bar_w = s * 0.5;
        let offset = s * 0.55;
        for cx_bar in [cx - offset, cx + offset] {
            let bar = D2D_RECT_F {
                left: cx_bar - bar_w * 0.5,
                top: cy - s,
                right: cx_bar + bar_w * 0.5,
                bottom: cy + s,
            };
            self.fill_rrect(t, bar, bar_w * 0.25, color);
        }
    }

    /// A bar plus a triangle pointing at it — the ⏮ / ⏭ glyph shape, built
    /// from primitives already used elsewhere (`fill_rrect`, `fill_triangle`).
    fn skip_icon(
        &self,
        t: &ID2D1DCRenderTarget,
        factory: &windows::Win32::Graphics::Direct2D::ID2D1Factory,
        (cx, cy, r): (f32, f32, f32),
        color: Rgba,
        to_left: bool,
    ) {
        let s = r * 0.62;
        let sign: f32 = if to_left { -1.0 } else { 1.0 };

        let bar_x = cx + sign * s * 0.95;
        let bar = D2D_RECT_F {
            left: bar_x - s * 0.16,
            top: cy - s,
            right: bar_x + s * 0.16,
            bottom: cy + s,
        };
        self.fill_rrect(t, bar, s * 0.1, color);

        let tip_x = cx + sign * s * 0.1;
        let base_x = cx - sign * s * 0.75;
        let pts = [
            D2D_POINT_2F {
                x: base_x,
                y: cy - s,
            },
            D2D_POINT_2F {
                x: base_x,
                y: cy + s,
            },
            D2D_POINT_2F { x: tip_x, y: cy },
        ];
        self.fill_triangle(t, factory, pts, color);
    }

    /// Fills a closed triangle via a one-off path geometry. Used only by the
    /// transport-button icons — everything else on the notch is text, dots,
    /// rings, or rounded rects, none of which can produce a pointed shape.
    fn fill_triangle(
        &self,
        t: &ID2D1DCRenderTarget,
        factory: &windows::Win32::Graphics::Direct2D::ID2D1Factory,
        pts: [D2D_POINT_2F; 3],
        color: Rgba,
    ) {
        let Ok(geometry) = (unsafe { factory.CreatePathGeometry() }) else {
            return;
        };
        unsafe {
            let Ok(sink) = geometry.Open() else {
                return;
            };
            sink.BeginFigure(pts[0], D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(pts[1]);
            sink.AddLine(pts[2]);
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            if sink.Close().is_err() {
                return;
            }
        }
        if let Ok(b) = self.brush(t, color) {
            unsafe { t.FillGeometry(&geometry, &b, None) };
        }
    }

    /// Decode-on-demand, cache, and paint the current track's album art as a
    /// centred cover-fit crop. Mirrors [`Painter::fill_with_image`], but keyed
    /// on the watcher's art generation instead of a file path, and with no
    /// focal-point control — a thumbnail has no meaningful focus to tune.
    fn fill_media_art(
        &mut self,
        t: &ID2D1DCRenderTarget,
        now: &NowPlaying,
        art: D2D_RECT_F,
        radius: f32,
        alpha: f32,
        generation: u64,
    ) -> bool {
        let Some(bytes) = &now.art else {
            return false;
        };
        let key = now.art_generation;
        if self.media_art_failed == Some(key) {
            return false;
        }

        let stale = match &self.media_art {
            Some(cached) => cached.key != key || cached.generation != generation,
            None => true,
        };

        if stale {
            match self.decode_media_art(t, bytes) {
                Some(bitmap) => {
                    self.media_art = Some(CachedArt {
                        key,
                        generation,
                        bitmap,
                    });
                    self.media_art_failed = None;
                }
                None => {
                    self.media_art_failed = Some(key);
                    self.media_art = None;
                    return false;
                }
            }
        }

        let Some(cached) = &self.media_art else {
            return false;
        };

        let size = unsafe { cached.bitmap.GetSize() };
        if size.width <= 0.0 || size.height <= 0.0 {
            return false;
        }

        let dst_w = art.right - art.left;
        let dst_h = art.bottom - art.top;
        let scale = (dst_w / size.width).max(dst_h / size.height);
        let draw_w = size.width * scale;
        let draw_h = size.height * scale;
        let ox = art.left - (draw_w - dst_w) * 0.5;
        let oy = art.top - (draw_h - dst_h) * 0.5;

        let props = D2D1_BITMAP_BRUSH_PROPERTIES {
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
        };

        let brush: ID2D1BitmapBrush =
            match unsafe { t.CreateBitmapBrush(&cached.bitmap, Some(&props), None) } {
                Ok(b) => b,
                Err(_) => return false,
            };

        unsafe {
            brush.SetTransform(&(scale_matrix(scale) * Matrix3x2::translation(ox, oy)));
            brush.SetOpacity(alpha);
        }

        let rr = D2D1_ROUNDED_RECT {
            rect: art,
            radiusX: radius,
            radiusY: radius,
        };
        unsafe { t.FillRoundedRectangle(&rr, &brush) };
        true
    }

    /// Decode album-art bytes straight from memory through WIC — the same
    /// pipeline as [`Painter::decode`], except the source is a byte slice
    /// instead of a file, so it goes through `IWICStream::InitializeFromMemory`
    /// rather than `CreateDecoderFromFilename`.
    fn decode_media_art(&self, t: &ID2D1DCRenderTarget, bytes: &[u8]) -> Option<ID2D1Bitmap> {
        let wic = self.wic.as_ref()?;

        unsafe {
            let stream: IWICStream = wic.CreateStream().ok()?;
            stream.InitializeFromMemory(bytes).ok()?;
            let istream: IStream = stream.cast().ok()?;

            let decoder = wic
                .CreateDecoderFromStream(&istream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)
                .ok()?;
            let frame = decoder.GetFrame(0).ok()?;
            let converter = wic.CreateFormatConverter().ok()?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeMedianCut,
                )
                .ok()?;

            t.CreateBitmapFromWicBitmap(&converter, None).ok()
        }
    }

    /// Slide indicator. The active dot stretches into a short ember capsule and
    /// slides continuously, so it tracks the carousel spring rather than
    /// snapping between positions.
    fn paint_pagination(
        &self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        count: usize,
        pos: f32,
        shape: NotchShape,
        alpha: f32,
    ) {
        let dot_r = 2.4;
        let gap = 12.0;
        let total = (count - 1) as f32 * gap;
        let cx = (shape.left + shape.right) * 0.5;
        let y = shape.bottom - 16.0;
        let start = cx - total * 0.5;

        for i in 0..count {
            let x = start + i as f32 * gap;
            self.dot(t, x, y, dot_r, theme::fade(self.pal.text_lo, alpha * 0.8));
        }

        let px = start + pos.clamp(0.0, (count - 1) as f32) * gap;
        let capsule = D2D_RECT_F {
            left: px - 7.0,
            top: y - dot_r - 0.6,
            right: px + 7.0,
            bottom: y + dot_r + 0.6,
        };
        self.fill_rrect(
            t,
            capsule,
            dot_r + 0.6,
            theme::fade(cfg.notch.accent, alpha),
        );
    }
}

fn scale_matrix(s: f32) -> Matrix3x2 {
    Matrix3x2 {
        M11: s,
        M12: 0.0,
        M21: 0.0,
        M22: s,
        M31: 0.0,
        M32: 0.0,
    }
}

/// Convert HSL color model to RGBA float array.
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Rgba {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (((h * 6.0) % 2.0) - 1.0).abs());
    let m = l - c * 0.5;
    let (r1, g1, b1) = match ((h * 6.0) as u32) % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r1 + m, g1 + m, b1 + m, 1.0]
}

/// Builds a Direct2D path tracing the 3-sided outer border (left, bottom,
/// right — the top is fused to the bezel or is the panel's own top edge, so
/// it never gets a glow) for any drain progress in `[0.0, 1.0]`, returning
/// the geometry and the leading tip coordinate. Walks the same perimeter
/// points `NotchShape::rim_points` produces as a polyline, so the two stay
/// in sync and neither needs to reason about Direct2D arc sweep directions.
/// Everything in `bounds` except `slab`: the mask that keeps the shadow off
/// the glass.
fn outside(
    factory: &ID2D1Factory,
    bounds: D2D_RECT_F,
    slab: &ID2D1Geometry,
) -> windows::core::Result<ID2D1PathGeometry> {
    let rect = unsafe { factory.CreateRectangleGeometry(&bounds)? };
    let out = unsafe { factory.CreatePathGeometry()? };
    let sink = unsafe { out.Open()? };
    unsafe {
        rect.CombineWithGeometry(slab, D2D1_COMBINE_MODE_EXCLUDE, None, 0.25, &sink)?;
        sink.Close()?;
    }
    Ok(out)
}

fn build_3sided_border_geometry(
    factory: &ID2D1Factory,
    shape: &NotchShape,
    drain_fraction: f32,
) -> windows::core::Result<(ID2D1PathGeometry, (f32, f32))> {
    let points = shape.rim_points();

    let geometry = unsafe { factory.CreatePathGeometry()? };
    let sink = unsafe { geometry.Open()? };

    let first = *points.first().unwrap_or(&(shape.left, shape.top));
    unsafe {
        sink.BeginFigure(
            D2D_POINT_2F {
                x: first.0,
                y: first.1,
            },
            D2D1_FIGURE_BEGIN_HOLLOW,
        )
    };

    let seg_lens: Vec<f32> = points
        .windows(2)
        .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
        .collect();
    let total_len: f32 = seg_lens.iter().sum();
    let target_len = (total_len * drain_fraction.clamp(0.0, 1.0)).max(0.1);

    let mut consumed = 0.0f32;
    let mut current_pt = first;

    for (i, &seg_len) in seg_lens.iter().enumerate() {
        if seg_len <= 0.0001 {
            continue;
        }
        let remaining = target_len - consumed;
        if remaining <= 0.0 {
            break;
        }

        let (p0, p1) = (points[i], points[i + 1]);
        if remaining >= seg_len {
            unsafe { sink.AddLine(D2D_POINT_2F { x: p1.0, y: p1.1 }) };
            current_pt = p1;
            consumed += seg_len;
        } else {
            let f = remaining / seg_len;
            let pt = (p0.0 + (p1.0 - p0.0) * f, p0.1 + (p1.1 - p0.1) * f);
            unsafe { sink.AddLine(D2D_POINT_2F { x: pt.0, y: pt.1 }) };
            current_pt = pt;
            break;
        }
    }

    unsafe {
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
    }

    Ok((geometry, current_pt))
}

/// `m:ss`, or `h:mm:ss` past the hour, for how long something has run.
fn elapsed_text(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The colour of a break, and of a meeting that is happening now.
const BREAK_GREEN: Rgba = [0.30, 0.86, 0.55, 1.0];

/// "45 min", "1 h 20 min", "3 d".
fn span_text(mins: i64) -> String {
    let m = mins.max(0);
    if m < 60 {
        format!("{} min", m.max(1))
    } else if m < 1440 {
        let (h, r) = (m / 60, m % 60);
        if r == 0 {
            format!("{h} h")
        } else {
            format!("{h} h {r} min")
        }
    } else {
        format!("{} d", m / 1440)
    }
}

/// The pill's short form of [`span_text`]: "45m", "1h20", "3d".
fn span_short(mins: i64) -> String {
    let m = mins.max(0);
    if m < 60 {
        format!("{}m", m.max(1))
    } else if m < 1440 {
        let (h, r) = (m / 60, m % 60);
        if r == 0 {
            format!("{h}h")
        } else {
            format!("{h}h{r:02}")
        }
    } else {
        format!("{}d", m / 1440)
    }
}

/// Clock time of a local-minutes timestamp.
fn clock_text(at: i64, twenty_four: bool) -> String {
    let m = at.rem_euclid(1440);
    let (h, mm) = (m / 60, m % 60);
    if twenty_four {
        format!("{h:02}:{mm:02}")
    } else {
        let h12 = if h % 12 == 0 { 12 } else { h % 12 };
        format!("{h12}:{mm:02} {}", if h < 12 { "AM" } else { "PM" })
    }
}

/// "Today", "Tmrw" or a weekday, for the event list.
fn day_word(at: i64, now: i64) -> &'static str {
    let (d, today) = (at.div_euclid(1440), now.div_euclid(1440));
    match d - today {
        0 => "Today",
        1 => "Tmrw",
        _ => ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
            [crate::notch::calendar::weekday(d) as usize],
    }
}

fn event_time_range(ev: &crate::notch::calendar::Event, twenty_four: bool, now: i64) -> String {
    if ev.all_day {
        return format!("{}, all day", day_word(ev.start, now));
    }
    format!(
        "{}, {} \u{2013} {}",
        day_word(ev.start, now),
        clock_text(ev.start, twenty_four),
        clock_text(ev.end, twenty_four)
    )
}
