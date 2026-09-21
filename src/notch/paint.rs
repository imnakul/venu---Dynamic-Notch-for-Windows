//! Everything the notch draws.
//!
//! Layout is computed from the *animated* shape rather than the resting one,
//! so content is always composed against the box it will actually occupy this
//! frame. Content and shape are on separate timings: the slab opens first and
//! the contents rise into it a beat later, which is what stops the panel from
//! looking like a texture being stretched.

use std::cell::Cell;

use windows::core::{Interface, PCWSTR};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED,
    D2D1_FIGURE_END_OPEN, D2D_POINT_2F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Bitmap, ID2D1BitmapBrush, ID2D1DCRenderTarget, ID2D1Factory, ID2D1PathGeometry,
    ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_BRUSH_PROPERTIES,
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_ELLIPSE,
    D2D1_EXTEND_MODE_CLAMP, D2D1_ROUNDED_RECT,
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
use crate::notch::media::{MediaWatcher, NowPlaying};
use crate::notch::state::NotchState;
use crate::notch::surface::D2DSurface;
use crate::notch::text::TextEngine;
use crate::notch::theme::{self, Rgba};

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
    /// Raised by the drawing code while a frame is being painted. A `Cell`
    /// because the slides that report it are split between `&self` and
    /// `&mut self` methods and threading a flag back out of every one of them
    /// would be noise.
    motion: Cell<Motion>,
}

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
            motion: Cell::new(Motion::Still),
        })
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
        unsafe { t.CreateSolidColorBrush(&theme::premul(color), None) }
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
        origin: (i32, i32),
        may_capture: bool,
    ) -> windows::core::Result<()> {
        let (win_w, win_h) = surface.size();
        if win_w == 0 || win_h == 0 {
            return Ok(());
        }

        self.pal = theme::Palette::for_theme(cfg.notch.theme);
        // Each frame reports its own motion from scratch.
        self.motion.set(Motion::Still);

        let generation = surface.generation();
        let factory = surface.factory.clone();
        let Some(t) = surface.target() else {
            return Ok(());
        };
        let t = t.clone();

        // Device resources die with their target.
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

        self.paint_shadow(&t, &shape, expand);

        // The slab itself.
        if let Ok(geometry) = shape.build(&factory) {
            // Frosted lays a blurred capture of the desktop under a
            // half-transparent tint; the other themes are just the tint. The
            // capture is drawn through a bitmap brush rather than a layer so
            // it takes the silhouette exactly, shoulders included.
            if self.pal.blur_backdrop && may_capture {
                self.paint_backdrop(&t, &geometry, &shape, origin);
            }

            if let Ok(fill) = self.brush(&t, cfg.notch.surface) {
                unsafe { t.FillGeometry(&geometry, &fill, None) };
            }
            // Hairline edge, then a brighter specular lip across the top third.
            if let Ok(edge) = self.brush(&t, self.pal.edge) {
                unsafe { t.DrawGeometry(&geometry, &edge, 1.0, None) };
            }
            if let Ok(inner) = shape.inset(0.9).build(&factory) {
                if let Ok(sheen) = self.brush(&t, self.pal.sheen) {
                    self.clip(
                        &t,
                        D2D_RECT_F {
                            left: shape.left - theme::FLARE,
                            top: shape.top,
                            right: shape.right + theme::FLARE,
                            bottom: shape.top + shape.height() * 0.42,
                        },
                    );
                    unsafe { t.DrawGeometry(&inner, &sheen, 1.4, None) };
                    self.unclip(&t);
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
            self.paint_collapsed(
                &t,
                cfg,
                state,
                slides[idx],
                shape,
                collapsed_alpha,
                generation,
            );
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

        if expanded_alpha > 0.004 {
            self.paint_settings_button(&t, cfg, state, shape, expanded_alpha);
            self.paint_pin_button(&t, cfg, state, shape, expanded_alpha);
        }

        self.paint_click_through(&t, cfg, state, shape);

        self.unclip(&t);

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
                let path_points = sample_notch_perimeter(shape, 88);
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
                let path_points = sample_notch_perimeter(shape, 88);
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

    /// Settings launcher button: opens the Settings panel on click (Hugeicons stroke gear).
    fn paint_settings_button(
        &self,
        t: &ID2D1DCRenderTarget,
        _cfg: &AppConfig,
        _state: &NotchState,
        shape: NotchShape,
        alpha: f32,
    ) {
        let (cx, cy, r) = shape.settings_button();

        // Soft well
        self.dot(t, cx, cy, r + 6.0, theme::fade(self.pal.well, alpha));

        let gear_color = theme::fade(self.pal.text_lo, alpha * 0.95);
        let hub_r = r * 0.36;
        let outer_r = r * 0.74;

        // Gear body rings
        self.ring(t, cx, cy, outer_r, 1.2, gear_color);
        self.ring(t, cx, cy, hub_r, 1.0, gear_color);

        // 6 gear cogs / teeth
        for i in 0..6 {
            let angle = (i as f32) * (std::f32::consts::PI / 3.0);
            let cos_a = angle.cos();
            let sin_a = angle.sin();
            let x1 = cx + cos_a * (outer_r - 0.6);
            let y1 = cy + sin_a * (outer_r - 0.6);
            let x2 = cx + cos_a * (outer_r + 2.4);
            let y2 = cy + sin_a * (outer_r + 2.4);
            self.line(t, x1, y1, x2, y2, 1.6, gear_color);
        }
    }

    /// Pin toggle button: locks the panel open, drawn with authentic Hugeicons pushpin stroke geometry.
    fn paint_pin_button(
        &self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        state: &NotchState,
        shape: NotchShape,
        alpha: f32,
    ) {
        let (cx, cy, r) = shape.pin_button();
        let pinned = state.pinned;

        // Soft well behind it
        self.dot(t, cx, cy, r + 6.0, theme::fade(self.pal.well, alpha));

        let color = if pinned {
            theme::fade(cfg.notch.accent, alpha)
        } else {
            theme::fade(self.pal.text_lo, alpha * 0.9)
        };

        // Hugeicons angled pushpin stroke geometry:
        let tip_x = cx - 4.4;
        let tip_y = cy + 4.4;
        let base_x = cx - 1.0;
        let base_y = cy + 1.0;
        let head_x = cx + 3.8;
        let head_y = cy - 3.8;

        // Needle line
        self.line(t, tip_x, tip_y, base_x, base_y, 1.3, color);

        // Collar crossbar
        self.line(
            t,
            base_x - 2.4,
            base_y - 2.4,
            base_x + 2.4,
            base_y + 2.4,
            1.5,
            color,
        );

        // Barrel
        self.line(
            t,
            base_x,
            base_y,
            head_x,
            head_y,
            if pinned { 2.6 } else { 1.6 },
            color,
        );

        // Pin cap head
        self.line(
            t,
            head_x - 3.2,
            head_y - 3.2,
            head_x + 3.2,
            head_y + 3.2,
            2.0,
            color,
        );

        if pinned {
            self.dot(t, tip_x, tip_y, 1.6, theme::fade(cfg.notch.accent, alpha));
        }
    }

    /// The click-through marker: a permanent pinprick while the mode is on,
    /// and a short banner the moment it is switched either way.
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
        if cfg.notch.click_through {
            // Bottom-right, small and dim: present enough to explain why
            // clicks are going elsewhere, quiet enough to live there.
            self.dot(
                t,
                shape.right - 7.5,
                shape.bottom - 7.5,
                2.0,
                theme::fade(self.pal.text_lo, 0.75 * self.pulse(state)),
            );
        }

        let flash = state.click_through_flash;
        if flash <= 0.0 {
            return;
        }

        // Full opacity for most of the dwell, then a fade out, so it reads as
        // a deliberate confirmation rather than a flicker.
        let alpha = (flash / 0.45).min(1.0);
        let text = if cfg.notch.click_through {
            "CLICKS PASS THROUGH"
        } else {
            "CLICKS LAND HERE"
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
    fn paint_backdrop(
        &mut self,
        t: &ID2D1DCRenderTarget,
        geometry: &windows::Win32::Graphics::Direct2D::ID2D1PathGeometry,
        shape: &NotchShape,
        origin: (i32, i32),
    ) {
        // What is behind the notch is not ours to watch for changes, so a
        // glass theme has to keep resampling. Ambient rather than continuous:
        // a blur this heavy, behind a strip this small, reads the same at a
        // few frames a second as it does at sixty.
        self.report(Motion::Ambient);

        let x = origin.0 + shape.left.floor() as i32;
        let y = origin.1 + shape.top.floor() as i32;
        let w = shape.width().ceil() as i32;
        let h = shape.height().ceil() as i32;

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
            brush.SetTransform(&(stretch * Matrix3x2::translation(shape.left, shape.top)));
            t.FillGeometry(geometry, &brush, None);
        }
    }

    /// Stacked rings of decreasing alpha standing in for a real blur. Cheap,
    /// and at this radius indistinguishable from a Gaussian shadow.
    fn paint_shadow(&self, t: &ID2D1DCRenderTarget, shape: &NotchShape, expand: f32) {
        const RINGS: u32 = 14;
        let spread = theme::shadow_spread(expand);
        // A light panel over a light desktop needs a harder shadow than a
        // black slab does to separate from it at all.
        let strength = lerp(0.16, 0.34, expand) * self.pal.shadow_strength;

        for i in (0..RINGS).rev() {
            let f = (i as f32 + 1.0) / RINGS as f32;
            let grow = spread * f;
            let alpha = strength * (1.0 - f).powi(2) / RINGS as f32 * 6.0;
            if alpha <= 0.002 {
                continue;
            }
            let rect = D2D_RECT_F {
                left: shape.left - grow,
                top: shape.top - grow * 0.35,
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
        let pad = theme::PILL_GUTTER;
        let cx = (shape.left + shape.right) * 0.5;
        let cy = (shape.top + shape.bottom) * 0.5;
        let inner_w = (shape.width() - pad * 2.0).max(10.0);

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

        match slide {
            SlideKind::Clock => {
                let clock = Clock::now();
                let time = clock.time_string(cfg.notch.clock_24h);

                let Ok(fmt) =
                    self.text
                        .format(family, theme::SIZE_PILL + 1.0, DWRITE_FONT_WEIGHT_BOLD)
                else {
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
                let dot_r = 3.0;
                let dot_x = shape.left + pad + dot_r;
                self.dot(
                    t,
                    dot_x,
                    cy,
                    dot_r,
                    theme::fade(
                        cfg.notch.accent,
                        alpha * if now.playing { self.pulse(state) } else { 0.5 },
                    ),
                );

                let text_x = dot_x + dot_r + 9.0;
                let avail = (shape.right - pad - text_x).max(10.0);
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
            SlideKind::Media => self.paint_media(t, factory, cfg, body, alpha, generation),
            SlideKind::Notifications => self.paint_notifications(t, cfg, state, body, alpha),
            SlideKind::Usage => self.paint_usage(t, cfg, body, alpha),
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
                family,
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
                family,
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
            // Draw list of recent notifications (up to 3 items)
            let item_h = 38.0;
            let max_items = 3.min(store.items.len());

            for (i, item) in store.items.iter().take(max_items).enumerate() {
                let iy = list_top + (i as f32) * (item_h + 6.0);
                let item_rect = D2D_RECT_F {
                    left: body.left,
                    top: iy,
                    right: body.right,
                    bottom: iy + item_h,
                };

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
                    top: item_rect.top + 7.0,
                    right: item_rect.left + 82.0,
                    bottom: item_rect.top + 21.0,
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
                    family,
                    &item.time_str,
                    theme::SIZE_LABEL - 2.0,
                    DWRITE_FONT_WEIGHT_MEDIUM,
                    0.0,
                    70.0,
                    item_rect.right - 64.0,
                    item_rect.top + 8.0,
                    theme::fade(self.pal.text_lo, alpha * 0.7),
                );

                // Notification Title
                self.label(
                    t,
                    family,
                    &item.title,
                    theme::SIZE_PILL - 1.0,
                    DWRITE_FONT_WEIGHT_BOLD,
                    0.0,
                    item_rect.right - item_rect.left - 165.0,
                    item_rect.left + 90.0,
                    item_rect.top + 6.0,
                    theme::fade(self.pal.text_hi, alpha),
                );

                // Notification Body text
                self.label(
                    t,
                    family,
                    &item.body,
                    theme::SIZE_LABEL - 0.5,
                    DWRITE_FONT_WEIGHT_MEDIUM,
                    0.0,
                    item_rect.right - item_rect.left - 100.0,
                    item_rect.left + 90.0,
                    item_rect.top + 20.0,
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
                if state.editing {
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
            rw,
            rx,
            body.top,
            theme::fade(self.pal.text_lo, alpha),
        );

        let row_h = 25.0;
        let list_top = body.top + 24.0;
        let capacity = (((body.bottom - list_top) / row_h).floor() as usize).max(1);

        if cfg.status.items.is_empty() {
            self.label(
                t,
                family,
                "Nothing else queued",
                theme::SIZE_BODY,
                DWRITE_FONT_WEIGHT_MEDIUM,
                0.0,
                rw,
                rx,
                list_top + 2.0,
                theme::fade(self.pal.text_lo, alpha),
            );
            return;
        }

        for (i, item) in cfg.status.items.iter().take(capacity).enumerate() {
            let y = list_top + i as f32 * row_h;
            let cy = y + row_h * 0.5 - 3.0;

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

        let overflow = cfg.status.items.len().saturating_sub(capacity);
        if overflow > 0 {
            self.label(
                t,
                family,
                &format!("+{overflow} MORE"),
                theme::SIZE_LABEL - 1.0,
                DWRITE_FONT_WEIGHT_BOLD,
                theme::TRACK_LABEL,
                rw,
                rx + 16.0,
                body.bottom - 11.0,
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
        if let Ok(fmt) = self
            .text
            .format(family, theme::SIZE_CLOCK, DWRITE_FONT_WEIGHT_EXTRA_BOLD)
        {
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

    /// Now Playing: album art on the left, title/artist stacked to its right,
    /// transport buttons along the bottom. Falls back to a centred label when
    /// no app currently owns the system media session.
    fn paint_media(
        &mut self,
        t: &ID2D1DCRenderTarget,
        factory: &windows::Win32::Graphics::Direct2D::ID2D1Factory,
        cfg: &AppConfig,
        body: D2D_RECT_F,
        alpha: f32,
        generation: u64,
    ) {
        let family = &cfg.notch.font_family;
        let now = self.media.snapshot();

        if !now.has_session {
            self.label(
                t,
                family,
                "NOTHING PLAYING",
                theme::SIZE_LABEL,
                DWRITE_FONT_WEIGHT_EXTRA_BOLD,
                theme::TRACK_LABEL,
                body.right - body.left,
                body.left,
                body.top + (body.bottom - body.top) * 0.5 - 6.0,
                theme::fade(self.pal.text_lo, alpha),
            );
            return;
        }

        let controls_h = 52.0_f32.min((body.bottom - body.top) * 0.55);
        let top_h = (body.bottom - body.top - controls_h).max(24.0);
        let art_size = top_h.min((body.right - body.left) * 0.32).clamp(32.0, 72.0);

        let art = D2D_RECT_F {
            left: body.left,
            top: body.top,
            right: body.left + art_size,
            bottom: body.top + art_size,
        };
        let radius = 8.0;

        let drawn = self.fill_media_art(t, &now, art, radius, alpha, generation);
        if !drawn {
            self.fill_rrect(t, art, radius, theme::fade(self.pal.well, alpha * 2.0));
            self.dot(
                t,
                (art.left + art.right) * 0.5,
                (art.top + art.bottom) * 0.5,
                art_size * 0.18,
                theme::fade(self.pal.text_lo, alpha),
            );
        }

        let text_x = art.right + 14.0;
        let text_w = (body.right - text_x).max(20.0);
        let title = if now.title.trim().is_empty() {
            "Unknown title"
        } else {
            now.title.trim()
        };
        let title_h = self.label(
            t,
            family,
            title,
            theme::SIZE_BODY + 1.0,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            0.0,
            text_w,
            text_x,
            art.top + 2.0,
            theme::fade(self.pal.text_hi, alpha),
        );

        if !now.artist.trim().is_empty() {
            self.label(
                t,
                family,
                now.artist.trim(),
                theme::SIZE_LABEL,
                DWRITE_FONT_WEIGHT_MEDIUM,
                0.0,
                text_w,
                text_x,
                art.top + 2.0 + title_h + 3.0,
                theme::fade(self.pal.text_mid, alpha),
            );
        }

        self.paint_media_controls(t, factory, cfg, body, now.playing, alpha);
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

/// Builds a smooth continuous Direct2D PathGeometry tracing the 3-sided outer border
/// (left down, bottom-left curve, bottom across, bottom-right curve, right up)
/// for any drain progress in [0.0, 1.0], returning the geometry and the leading tip coordinate.
/// A quarter-circle corner arc, oriented so `point_at(0.0)` sits nearer the
/// top of the shape and `point_at(1.0)` sits where the straight edge begins
/// — the direction the 3-sided border is traced in, whether the corner is a
/// concave bezel shoulder or a convex rounded corner.
#[derive(Clone, Copy)]
struct ArcCorner {
    center: (f32, f32),
    radius: f32,
    theta_start: f32,
    theta_end: f32,
}

impl ArcCorner {
    fn point_at(&self, frac: f32) -> (f32, f32) {
        let theta = self.theta_start - frac.clamp(0.0, 1.0) * (self.theta_start - self.theta_end);
        (
            self.center.0 + self.radius * theta.cos(),
            self.center.1 + self.radius * theta.sin(),
        )
    }

    fn start(&self) -> (f32, f32) {
        self.point_at(0.0)
    }

    fn end(&self) -> (f32, f32) {
        self.point_at(1.0)
    }
}

/// The four corner arcs of the 3-sided border (top-left, bottom-left,
/// bottom-right, top-right), matching `NotchShape::build`'s own corner
/// geometry exactly — concave shoulder while fused to the bezel, rounded
/// corner while detached, or `None` for a hard corner — so the glow border
/// always hugs the real silhouette instead of cutting across the shoulder.
fn notch_corners(
    shape: &NotchShape,
) -> (
    Option<ArcCorner>,
    Option<ArcCorner>,
    Option<ArcCorner>,
    Option<ArcCorner>,
) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let (l, t, r, b) = (shape.left, shape.top, shape.right, shape.bottom);
    let limit = (shape.width().min(shape.height()) * 0.5).max(0.0);
    let rt = shape.radius_top.clamp(0.0, limit);
    let rb = shape.radius_bottom.clamp(0.0, limit);
    let flare = shape.flare.clamp(0.0, (shape.height() * 0.5).max(0.0));

    let top_left = if flare > 0.5 {
        Some(ArcCorner {
            center: (l, t),
            radius: flare,
            theta_start: PI,
            theta_end: FRAC_PI_2,
        })
    } else if rt > 0.5 {
        Some(ArcCorner {
            center: (l + rt, t + rt),
            radius: rt,
            theta_start: PI + FRAC_PI_2,
            theta_end: PI,
        })
    } else {
        None
    };

    let top_right = if flare > 0.5 {
        Some(ArcCorner {
            center: (r, t),
            radius: flare,
            theta_start: FRAC_PI_2,
            theta_end: 0.0,
        })
    } else if rt > 0.5 {
        Some(ArcCorner {
            center: (r - rt, t + rt),
            radius: rt,
            theta_start: 2.0 * PI,
            theta_end: PI + FRAC_PI_2,
        })
    } else {
        None
    };

    let bottom_left = if rb > 0.5 {
        Some(ArcCorner {
            center: (l + rb, b - rb),
            radius: rb,
            theta_start: PI,
            theta_end: FRAC_PI_2,
        })
    } else {
        None
    };

    let bottom_right = if rb > 0.5 {
        Some(ArcCorner {
            center: (r - rb, b - rb),
            radius: rb,
            theta_start: FRAC_PI_2,
            theta_end: 0.0,
        })
    } else {
        None
    };

    (top_left, bottom_left, bottom_right, top_right)
}

/// Builds a Direct2D path tracing the 3-sided outer border (left, bottom,
/// right — the top is fused to the bezel or is the panel's own top edge, so
/// it never gets a glow) for any drain progress in `[0.0, 1.0]`, returning
/// the geometry and the leading tip coordinate. Walks the same perimeter
/// samples `sample_notch_perimeter` produces as a polyline, so the two stay
/// in sync and neither needs to reason about Direct2D arc sweep directions.
fn build_3sided_border_geometry(
    factory: &ID2D1Factory,
    shape: &NotchShape,
    drain_fraction: f32,
) -> windows::core::Result<(ID2D1PathGeometry, (f32, f32))> {
    let points = sample_notch_perimeter(shape, 0);

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

/// Sample points along the 3-sided outer border (left, bottom, right) of the
/// notch, from the tip where the left edge meets the top — whether that's a
/// concave bezel shoulder, a rounded corner, or a hard corner — all the way
/// around to the matching tip on the right. Mirrors `NotchShape::build`'s
/// corner geometry so the glow always hugs the real silhouette.
fn sample_notch_perimeter(shape: &NotchShape, _step_count: usize) -> Vec<(f32, f32)> {
    let (top_left, bottom_left, bottom_right, top_right) = notch_corners(shape);
    let (l, t, r, b) = (shape.left, shape.top, shape.right, shape.bottom);

    let corner_steps = 14;
    let straight_steps = 18;
    let bottom_steps = 36;

    let sample_arc = |path: &mut Vec<(f32, f32)>, c: &ArcCorner| {
        for i in 0..=corner_steps {
            let f = i as f32 / corner_steps as f32;
            path.push(c.point_at(f));
        }
    };

    let mut path: Vec<(f32, f32)> = Vec::with_capacity(160);

    // 1. Top-left shoulder / rounded corner / hard corner.
    match &top_left {
        Some(c) => sample_arc(&mut path, c),
        None => path.push((l, t)),
    }

    // 2. Left edge straight down.
    let left_from = top_left.map(|c| c.end()).unwrap_or((l, t));
    let left_to = bottom_left.map(|c| c.start()).unwrap_or((l, b));
    for i in 0..=straight_steps {
        let f = i as f32 / straight_steps as f32;
        path.push((
            left_from.0 + (left_to.0 - left_from.0) * f,
            left_from.1 + (left_to.1 - left_from.1) * f,
        ));
    }

    // 3. Bottom-left corner.
    if let Some(c) = &bottom_left {
        sample_arc(&mut path, c);
    }

    // 4. Bottom edge across.
    let bottom_from = bottom_left.map(|c| c.end()).unwrap_or((l, b));
    let bottom_to = bottom_right.map(|c| c.start()).unwrap_or((r, b));
    for i in 0..=bottom_steps {
        let f = i as f32 / bottom_steps as f32;
        path.push((
            bottom_from.0 + (bottom_to.0 - bottom_from.0) * f,
            bottom_from.1,
        ));
    }

    // 5. Bottom-right corner.
    if let Some(c) = &bottom_right {
        sample_arc(&mut path, c);
    }

    // 6. Right edge straight up.
    let right_from = bottom_right.map(|c| c.end()).unwrap_or((r, b));
    let right_to = top_right.map(|c| c.start()).unwrap_or((r, t));
    for i in 0..=straight_steps {
        let f = i as f32 / straight_steps as f32;
        path.push((
            right_from.0 + (right_to.0 - right_from.0) * f,
            right_from.1 + (right_to.1 - right_from.1) * f,
        ));
    }

    // 7. Top-right shoulder / rounded corner / hard corner.
    match &top_right {
        Some(c) => sample_arc(&mut path, c),
        None => path.push((r, t)),
    }

    path
}
