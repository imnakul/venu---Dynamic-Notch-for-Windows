//! The newer pages of the open panel — the centred Now Playing player, the
//! Shelf and Clipboard — plus the music wave they and the pill share.
//!
//! A child module of `paint`, so it draws with the painter's own primitives
//! and caches without making them public.

use std::sync::Arc;

use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Bitmap, ID2D1DCRenderTarget, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_EXTRA_BOLD, DWRITE_FONT_WEIGHT_MEDIUM,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use super::{scale_matrix, Painter};
use crate::config::AppConfig;
use crate::notch::clipboard;
use crate::notch::geom;
use crate::notch::layout::{self, Rect, ShelfRow, WAVE_BARS};
use crate::notch::menu::{self as menu_model, MenuRow, MenuView};
use crate::notch::shelf::{self, Icon, ShelfEntry};
use crate::notch::state::NotchState;
use crate::notch::text::TextEngine;
use crate::notch::theme;
use windows::core::Interface;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::{
    ID2D1Factory, ID2D1Layer, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_LAYER_OPTIONS_NONE,
    D2D1_LAYER_PARAMETERS, D2D1_ROUNDED_RECT,
};

/// A warm red for destructive menu items.
const DANGER: theme::Rgba = [0.96, 0.40, 0.38, 1.0];

/// Segoe MDL2 Assets glyphs used here.
const GLYPH_PASTE: &str = "\u{E77F}";
const GLYPH_PIN: &str = "\u{E718}";

fn d2d(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

fn rect(r: D2D_RECT_F) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

/// "3:07", or "1:02:45" past an hour.
pub fn clock_text(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The shelf entry under a point of the open Shelf page, if any.
pub fn shelf_entry_at(body: D2D_RECT_F, x: f32, y: f32) -> Option<ShelfEntry> {
    let snap = shelf::snapshot();
    let layout = layout::shelf_layout(rect(body), snap.pinned.len(), snap.recent.len());
    let tile = layout.tiles.iter().find(|tile| tile.rect.contains(x, y))?;
    let list = match tile.row {
        ShelfRow::Pinned => &snap.pinned,
        ShelfRow::Recent => &snap.recent,
    };
    list.get(tile.index).cloned()
}

impl Painter {
    // -- buttons, hover and press -------------------------------------------------

    /// A round icon button: a soft chip with a glyph, lit in the accent when
    /// `active` (a held pin). Hover and press come from `paint_feedback`.
    pub(super) fn chip_button(
        &self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        (cx, cy, r): (f32, f32, f32),
        glyph: &str,
        active: bool,
        alpha: f32,
    ) {
        let chip = r + 4.0;
        if active {
            self.dot(t, cx, cy, chip, theme::fade(cfg.notch.accent, alpha * 0.22));
            self.glyph(t, glyph, 12.0, cx, cy, theme::fade(cfg.notch.accent, alpha));
        } else {
            self.dot(t, cx, cy, chip, theme::fade(self.pal.well, alpha * 1.3));
            self.glyph(
                t,
                glyph,
                12.0,
                cx,
                cy,
                theme::fade(self.pal.text_mid, alpha),
            );
        }
    }

    /// Hover highlight and click ripple over the panel's clickable targets.
    /// The highlight fades in and out on its own spring; a click sends a
    /// ripple out from where it landed, clipped to the target's shape.
    pub(super) fn paint_feedback(
        &self,
        t: &ID2D1DCRenderTarget,
        factory: &ID2D1Factory,
        cfg: &AppConfig,
        state: &NotchState,
        alpha: f32,
    ) {
        if let Some(target) = state.hot_shown {
            let a = state.hot_amount.value.clamp(0.0, 1.0) * alpha;
            if a > 0.004 {
                self.fill_rrect(
                    t,
                    d2d(target.rect),
                    target.radius,
                    theme::fade(self.pal.text_hi, a * 0.075),
                );
            }
        }

        let Some(press) = state.press else {
            return;
        };
        let p = press.progress();
        let ease = 1.0 - (1.0 - p).powi(3);
        let fade = (1.0 - p).powi(2) * alpha;
        let rect = d2d(press.target.rect);

        // A brief accent wash over the whole target…
        self.fill_rrect(
            t,
            rect,
            press.target.radius,
            theme::fade(cfg.notch.accent, fade * 0.16),
        );

        // …and a ripple from the click, inside the target's own shape.
        let reach = {
            let dx = (press.x - rect.left).max(rect.right - press.x);
            let dy = (press.y - rect.top).max(rect.bottom - press.y);
            (dx * dx + dy * dy).sqrt()
        };
        let rr = D2D1_ROUNDED_RECT {
            rect,
            radiusX: press.target.radius,
            radiusY: press.target.radius,
        };
        let Ok(mask) = (unsafe { factory.CreateRoundedRectangleGeometry(&rr) }) else {
            return;
        };
        let params = D2D1_LAYER_PARAMETERS {
            contentBounds: rect,
            geometricMask: std::mem::ManuallyDrop::new(mask.cast().ok()),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: 1.0,
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        unsafe { t.PushLayer(&params, None::<&ID2D1Layer>) };
        drop(std::mem::ManuallyDrop::into_inner(params.geometricMask));
        self.dot(
            t,
            press.x,
            press.y,
            (reach * ease).max(2.0),
            theme::fade(cfg.notch.accent, fade * 0.28),
        );
        unsafe { t.PopLayer() };
    }

    // -- the right-click menu ------------------------------------------------------

    /// The menu card: the notch's own surface and type, a highlight that
    /// slides between items, and a quick scale-and-fade from where it was
    /// opened.
    pub(super) fn paint_menu(&self, t: &ID2D1DCRenderTarget, cfg: &AppConfig, menu: &MenuView) {
        let a = menu.alpha();
        if a <= 0.004 {
            return;
        }
        let open = menu.open.value.clamp(0.0, 1.05);
        let (ax, ay) = menu.anchor;
        let scale = 0.94 + 0.06 * open.min(1.0);
        let lift = -6.0 * (1.0 - open.min(1.0));
        unsafe {
            t.SetTransform(
                &(Matrix3x2::translation(-ax, -ay)
                    * scale_matrix(scale)
                    * Matrix3x2::translation(ax, ay + lift)),
            );
        }

        let card = d2d(menu.rect);
        let r = menu_model::RADIUS;

        // Soft shadow: a few widening, fading plates under the card.
        let depth = self.pal.shadow_strength.max(0.6);
        for i in 0..4 {
            let grow = 2.0 + i as f32 * 3.0;
            let plate = D2D_RECT_F {
                left: card.left - grow,
                top: card.top - grow * 0.4,
                right: card.right + grow,
                bottom: card.bottom + grow * 1.4,
            };
            let fall = 0.075 * (1.0 - i as f32 / 4.0) * depth;
            self.fill_rrect(t, plate, r + grow, [0.0, 0.0, 0.0, fall * a]);
        }

        // The card: the notch's surface, kept solid enough to read on glass
        // themes, with the same hairline edge.
        let surface = cfg.notch.surface;
        let fill = [surface[0], surface[1], surface[2], surface[3].max(0.95) * a];
        self.fill_rrect(t, card, r, fill);
        self.stroke_rrect(t, card, r, 1.0, theme::fade(self.pal.edge, a));

        // The highlight, under the rows.
        let hl_alpha = menu.hl_alpha.value.clamp(0.0, 1.0) * a;
        let lit = menu.pressed.or(menu.hot);
        let lit_danger = lit.is_some_and(|slot| {
            matches!(
                menu.rows.get(slot.row),
                Some(MenuRow::Item { danger: true, .. })
            )
        });
        if hl_alpha > 0.004 {
            let tone = if lit_danger { DANGER } else { cfg.notch.accent };
            let strength = if menu.pressed.is_some() { 0.32 } else { 0.17 };
            self.fill_rrect(
                t,
                d2d(menu.highlight()),
                8.0,
                theme::fade(tone, hl_alpha * strength),
            );
        }

        let family = &cfg.notch.font_family;
        let left = card.left + menu_model::PAD;
        let right = card.right - menu_model::PAD;
        let mut y = card.top + menu_model::PAD;
        for (index, row) in menu.rows.iter().enumerate() {
            let lit_here = lit.is_some_and(|slot| slot.row == index);
            match row {
                MenuRow::Separator => {
                    let ry = (y + menu_model::SEP_H * 0.5).round();
                    self.fill_rrect(
                        t,
                        D2D_RECT_F {
                            left: left + 8.0,
                            top: ry - 0.5,
                            right: right - 8.0,
                            bottom: ry + 0.5,
                        },
                        0.5,
                        theme::fade(self.pal.rule, a),
                    );
                    y += menu_model::SEP_H;
                    continue;
                }
                MenuRow::Item {
                    label,
                    glyph,
                    toggle,
                    danger,
                    ..
                } => {
                    let cy = y + menu_model::ROW_H * 0.5;
                    let tone = if *danger && lit_here {
                        DANGER
                    } else if lit_here {
                        cfg.notch.accent
                    } else {
                        self.pal.text_mid
                    };
                    self.glyph(t, glyph, 12.0, left + 16.0, cy, theme::fade(tone, a));
                    let text_tone = if *danger && lit_here {
                        DANGER
                    } else {
                        self.pal.text_hi
                    };
                    if let Some((layout, _, h)) =
                        self.measure_text(family, label, 12.5, DWRITE_FONT_WEIGHT_MEDIUM)
                    {
                        self.draw_layout(
                            t,
                            &layout,
                            left + 34.0,
                            cy - h * 0.5,
                            theme::fade(text_tone, a),
                        );
                    }
                    if let Some(on) = toggle {
                        self.switch(t, cfg, right - 10.0, cy, *on, a);
                    }
                }
                MenuRow::Pair(first, second) => {
                    let cy = y + menu_model::ROW_H * 0.5;
                    let mid = (left + right) * 0.5;
                    self.fill_rrect(
                        t,
                        D2D_RECT_F {
                            left: mid - 0.5,
                            top: cy - 8.0,
                            right: mid + 0.5,
                            bottom: cy + 8.0,
                        },
                        0.5,
                        theme::fade(self.pal.rule, a),
                    );
                    for (half, item) in [(0u8, first), (1u8, second)] {
                        let hot = lit.is_some_and(|s| s.row == index && s.half == half);
                        let tone = if hot {
                            cfg.notch.accent
                        } else {
                            self.pal.text_mid
                        };
                        let (from, to) = if half == 0 { (left, mid) } else { (mid, right) };
                        let centre = (from + to) * 0.5;
                        if let Some((layout, w, h)) =
                            self.measure_text(family, item.label, 12.5, DWRITE_FONT_WEIGHT_MEDIUM)
                        {
                            let group = w + 20.0;
                            let start = centre - group * 0.5;
                            // Arrows point outward: ‹ Previous | Next ›.
                            let (glyph_x, text_x) = if half == 0 {
                                (start + 6.0, start + 20.0)
                            } else {
                                (start + w + 14.0, start)
                            };
                            self.glyph(t, item.glyph, 11.0, glyph_x, cy, theme::fade(tone, a));
                            self.draw_layout(
                                t,
                                &layout,
                                text_x,
                                cy - h * 0.5,
                                theme::fade(self.pal.text_hi, a),
                            );
                        }
                    }
                }
            }
            y += menu_model::ROW_H;
        }

        unsafe { t.SetTransform(&Matrix3x2::identity()) };
    }

    /// A small on/off switch ending at `right`, centred on `cy`.
    fn switch(
        &self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        right: f32,
        cy: f32,
        on: bool,
        a: f32,
    ) {
        let (w, h) = (26.0, 15.0);
        let track = D2D_RECT_F {
            left: right - w,
            top: cy - h * 0.5,
            right,
            bottom: cy + h * 0.5,
        };
        if on {
            self.fill_rrect(t, track, h * 0.5, theme::fade(cfg.notch.accent, a));
            self.dot(t, right - h * 0.5, cy, h * 0.5 - 2.0, [1.0, 1.0, 1.0, a]);
        } else {
            self.fill_rrect(t, track, h * 0.5, theme::fade(self.pal.well, a * 2.6));
            self.dot(
                t,
                right - w + h * 0.5,
                cy,
                h * 0.5 - 2.0,
                theme::fade(self.pal.text_lo, a),
            );
        }
    }

    // -- the music wave -------------------------------------------------------

    /// Equaliser bars centred on `(cx, cy)`. `bars` come from the audio
    /// meter (at rest when nothing plays); `height` is the tallest a bar
    /// gets.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn wave(
        &self,
        t: &ID2D1DCRenderTarget,
        cx: f32,
        cy: f32,
        height: f32,
        bar_w: f32,
        bars: &[f32; WAVE_BARS],
        color: theme::Rgba,
    ) {
        let gap = bar_w;
        let span = bar_w * WAVE_BARS as f32 + gap * (WAVE_BARS - 1) as f32;
        let left = cx - span * 0.5;
        for (i, level) in bars.iter().enumerate() {
            let x = left + i as f32 * (bar_w + gap);
            let half = (height * level.clamp(0.1, 1.0) * 0.5).max(bar_w * 0.5);
            self.fill_rrect(
                t,
                D2D_RECT_F {
                    left: x,
                    top: cy - half,
                    right: x + bar_w,
                    bottom: cy + half,
                },
                bar_w * 0.5,
                color,
            );
        }
    }

    /// Bars for a face: live while playing, a quiet flat line when paused.
    pub(super) fn wave_levels(&self, state: &NotchState) -> [f32; WAVE_BARS] {
        if self.media.is_playing() {
            state.wave
        } else {
            [0.3; WAVE_BARS]
        }
    }

    // -- Now Playing ------------------------------------------------------------

    /// Now Playing. A tall panel gets the centred player: the art large in the
    /// middle (16:9 for a video, square for an album), the title and artist
    /// under it, the timeline, then the buttons. A short, wide panel lays the
    /// same things out side by side.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn paint_now_playing(
        &mut self,
        t: &ID2D1DCRenderTarget,
        factory: &windows::Win32::Graphics::Direct2D::ID2D1Factory,
        cfg: &AppConfig,
        state: &NotchState,
        body: D2D_RECT_F,
        alpha: f32,
        generation: u64,
    ) {
        let family = &cfg.notch.font_family;
        let now = self.media.snapshot();

        if !now.has_session {
            let cx = (body.left + body.right) * 0.5;
            let cy = (body.top + body.bottom) * 0.5;
            self.wave(
                t,
                cx,
                cy - 18.0,
                18.0,
                4.0,
                &[0.3; WAVE_BARS],
                theme::fade(self.pal.text_lo, alpha * 0.8),
            );
            self.centred_label(
                t,
                family,
                "NOTHING PLAYING",
                theme::SIZE_LABEL,
                DWRITE_FONT_WEIGHT_EXTRA_BOLD,
                body,
                cy + 2.0,
                theme::fade(self.pal.text_lo, alpha),
            );
            self.centred_label(
                t,
                family,
                "Play something in any app and it shows up here.",
                11.5,
                DWRITE_FONT_WEIGHT_MEDIUM,
                body,
                cy + 20.0,
                theme::fade(self.pal.text_lo, alpha * 0.8),
            );
            return;
        }

        let stacked = body.bottom - body.top >= geom::MEDIA_STACKED_MIN_H;
        let aspect = if stacked {
            self.media_art_aspect(t, &now, generation)
        } else {
            1.0
        };
        let art = geom::media_art(body, aspect);
        let radius = if stacked { 12.0 } else { 9.0 };

        // A soft halo in the accent behind the art, so it sits in the panel
        // rather than being pasted on it.
        if stacked {
            for (grow, a) in [(10.0, 0.05), (5.0, 0.08)] {
                self.fill_rrect(
                    t,
                    D2D_RECT_F {
                        left: art.left - grow,
                        top: art.top - grow * 0.6,
                        right: art.right + grow,
                        bottom: art.bottom + grow,
                    },
                    radius + grow,
                    theme::fade(cfg.notch.accent, alpha * a),
                );
            }
        }
        let drawn = self.fill_media_art(t, &now, art, radius, alpha, generation);
        if !drawn {
            self.fill_rrect(t, art, radius, theme::fade(self.pal.well, alpha * 2.0));
            let side = (art.right - art.left).min(art.bottom - art.top);
            self.wave(
                t,
                (art.left + art.right) * 0.5,
                (art.top + art.bottom) * 0.5,
                side * 0.36,
                (side * 0.07).max(3.0),
                &self.wave_levels(state),
                theme::fade(cfg.notch.accent, alpha * 0.9),
            );
        }

        let title = if now.title.trim().is_empty() {
            "Unknown title"
        } else {
            now.title.trim()
        };
        let mut byline = now.artist.trim().to_owned();
        if !now.album.trim().is_empty() && now.album.trim() != title {
            if !byline.is_empty() {
                byline.push_str("  \u{00B7}  ");
            }
            byline.push_str(now.album.trim());
        }
        let source = now.source.trim();

        let column = geom::media_text_column(body);
        let [prev, _, _] = geom::media_transport_buttons(body);
        let buttons_top = prev.1 - 15.0;

        if stacked {
            let mut y = art.bottom + 12.0;
            y += self.centred_label(
                t,
                family,
                title,
                15.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                column,
                y,
                theme::fade(self.pal.text_hi, alpha),
            );
            let sub = match (byline.is_empty(), source.is_empty()) {
                (false, false) => format!("{byline}  \u{00B7}  {source}"),
                (false, true) => byline.clone(),
                (true, false) => source.to_owned(),
                (true, true) => String::new(),
            };
            if !sub.is_empty() {
                y += 2.0;
                self.centred_label(
                    t,
                    family,
                    &sub,
                    12.0,
                    DWRITE_FONT_WEIGHT_MEDIUM,
                    column,
                    y,
                    theme::fade(self.pal.text_mid, alpha),
                );
            }
            // The timeline sits just above the buttons, wherever they are.
            let line_y = buttons_top - 12.0;
            let inset = ((column.right - column.left) * 0.12).min(56.0);
            self.paint_timeline(
                t,
                cfg,
                &now,
                column.left + inset,
                column.right - inset,
                line_y,
                alpha,
            );
        } else {
            let mut y = column.top + 2.0;
            y += self.label(
                t,
                family,
                title,
                15.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                0.0,
                column.right - column.left,
                column.left,
                y,
                theme::fade(self.pal.text_hi, alpha),
            );
            if !byline.is_empty() {
                y += 2.0
                    + self.label(
                        t,
                        family,
                        &byline,
                        12.0,
                        DWRITE_FONT_WEIGHT_MEDIUM,
                        0.0,
                        column.right - column.left,
                        column.left,
                        y + 2.0,
                        theme::fade(self.pal.text_mid, alpha),
                    );
            }
            if !source.is_empty() {
                self.label(
                    t,
                    family,
                    &source.to_uppercase(),
                    theme::SIZE_LABEL - 1.0,
                    DWRITE_FONT_WEIGHT_BOLD,
                    theme::TRACK_LABEL,
                    column.right - column.left,
                    column.left,
                    y + 6.0,
                    theme::fade(self.pal.text_lo, alpha),
                );
            }
            self.paint_timeline(
                t,
                cfg,
                &now,
                column.left,
                column.right,
                buttons_top - 12.0,
                alpha,
            );
        }

        self.paint_media_controls(t, factory, cfg, body, now.playing, alpha);
    }

    /// The art's width over its height, from the decoded bitmap. A 4:3
    /// thumbnail is how YouTube letterboxes a 16:9 video, so it is shown as
    /// 16:9 and the black bars are cropped away.
    fn media_art_aspect(
        &mut self,
        t: &ID2D1DCRenderTarget,
        now: &crate::notch::media::NowPlaying,
        generation: u64,
    ) -> f32 {
        if now.art.is_none() {
            return 1.0;
        }
        // Decoding is cached; drawing it at zero size primes the cache.
        let probe = D2D_RECT_F::default();
        let _ = self.fill_media_art(t, now, probe, 0.0, 0.0, generation);
        let Some(cached) = &self.media_art else {
            return 1.0;
        };
        let size = unsafe { cached.bitmap.GetSize() };
        if size.width <= 0.0 || size.height <= 0.0 {
            return 1.0;
        }
        let aspect = size.width / size.height;
        if aspect > 1.2 {
            16.0 / 9.0
        } else {
            1.0
        }
    }

    /// Elapsed time, a thin progress line, total time. Nothing when the app
    /// does not report a length (a live stream, some browsers).
    #[allow(clippy::too_many_arguments)]
    fn paint_timeline(
        &self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        now: &crate::notch::media::NowPlaying,
        left: f32,
        right: f32,
        cy: f32,
        alpha: f32,
    ) {
        if now.duration_secs <= 0.0 || right - left < 80.0 {
            return;
        }
        let position = now.position_now();
        let elapsed = clock_text(position);
        let total = clock_text(now.duration_secs);
        let mono = crate::fonts::GEIST_MONO;
        let size = 10.0;
        let Some((elapsed_layout, ew, eh)) =
            self.measure_text(mono, &elapsed, size, DWRITE_FONT_WEIGHT_MEDIUM)
        else {
            return;
        };
        let Some((total_layout, tw, _)) =
            self.measure_text(mono, &total, size, DWRITE_FONT_WEIGHT_MEDIUM)
        else {
            return;
        };
        let dim = theme::fade(self.pal.text_lo, alpha);
        self.draw_layout(t, &elapsed_layout, left, cy - eh * 0.5, dim);
        self.draw_layout(t, &total_layout, right - tw, cy - eh * 0.5, dim);

        let track_left = left + ew + 10.0;
        let track_right = right - tw - 10.0;
        if track_right - track_left < 20.0 {
            return;
        }
        let track = D2D_RECT_F {
            left: track_left,
            top: cy - 1.5,
            right: track_right,
            bottom: cy + 1.5,
        };
        self.fill_rrect(t, track, 1.5, theme::fade(self.pal.well, alpha * 2.2));
        let fraction = (position / now.duration_secs).clamp(0.0, 1.0) as f32;
        let filled = D2D_RECT_F {
            right: track_left + (track_right - track_left) * fraction,
            ..track
        };
        if filled.right > filled.left + 1.0 {
            self.fill_rrect(t, filled, 1.5, theme::fade(cfg.notch.accent, alpha));
            self.dot(
                t,
                filled.right,
                cy,
                3.5,
                theme::fade(self.pal.text_hi, alpha),
            );
        }
    }

    /// One line centred across `area`, top at `y`. Returns its height.
    #[allow(clippy::too_many_arguments)]
    fn centred_label(
        &self,
        t: &ID2D1DCRenderTarget,
        family: &str,
        text: &str,
        size: f32,
        weight: windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT,
        area: D2D_RECT_F,
        y: f32,
        color: theme::Rgba,
    ) -> f32 {
        let Ok(format) = self.text.format(family, size, weight) else {
            return 0.0;
        };
        self.text.set_ellipsis(&format);
        unsafe {
            let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        }
        let width = (area.right - area.left).max(10.0);
        let Ok(layout) = self.text.layout(text, &format, width, size * 2.4) else {
            return 0.0;
        };
        self.draw_layout(t, &layout, area.left, y, color);
        TextEngine::measure(&layout).1
    }

    // -- Shelf ------------------------------------------------------------------

    pub(super) fn paint_shelf(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        body: D2D_RECT_F,
        alpha: f32,
        generation: u64,
    ) {
        let family = &cfg.notch.font_family;
        let snap = shelf::snapshot();

        if snap.pinned.is_empty() && snap.recent.is_empty() {
            let cy = (body.top + body.bottom) * 0.5;
            self.glyph(
                t,
                GLYPH_PIN,
                20.0,
                (body.left + body.right) * 0.5,
                cy - 26.0,
                theme::fade(self.pal.text_lo, alpha),
            );
            self.centred_label(
                t,
                family,
                "Drop files, folders or apps here",
                13.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                body,
                cy - 6.0,
                theme::fade(self.pal.text_hi, alpha),
            );
            self.centred_label(
                t,
                family,
                "Or add them in Settings \u{203A} Slides \u{203A} Shelf. Venu keeps the path, never a copy.",
                11.0,
                DWRITE_FONT_WEIGHT_MEDIUM,
                body,
                cy + 14.0,
                theme::fade(self.pal.text_lo, alpha),
            );
            return;
        }

        let layout = layout::shelf_layout(rect(body), snap.pinned.len(), snap.recent.len());
        for &(row, x, y) in &layout.headings {
            let heading = match row {
                ShelfRow::Pinned => "PINNED".to_owned(),
                ShelfRow::Recent => format!("NEWEST IN {}", snap.folder_name.to_uppercase()),
            };
            self.label(
                t,
                family,
                &heading,
                theme::SIZE_LABEL - 1.0,
                DWRITE_FONT_WEIGHT_EXTRA_BOLD,
                theme::TRACK_LABEL,
                body.right - x - 60.0,
                x,
                y,
                theme::fade(self.pal.text_lo, alpha),
            );
            if row == ShelfRow::Pinned && layout.pinned_overflow > 0 {
                self.label(
                    t,
                    family,
                    &format!("+{} MORE IN SETTINGS", layout.pinned_overflow),
                    theme::SIZE_LABEL - 1.5,
                    DWRITE_FONT_WEIGHT_BOLD,
                    theme::TRACK_LABEL,
                    160.0,
                    body.right - 150.0,
                    y,
                    theme::fade(self.pal.text_lo, alpha * 0.8),
                );
            }
        }

        for tile in &layout.tiles {
            let list = match tile.row {
                ShelfRow::Pinned => &snap.pinned,
                ShelfRow::Recent => &snap.recent,
            };
            let Some(entry) = list.get(tile.index) else {
                continue;
            };
            let shown = if entry.exists { alpha } else { alpha * 0.4 };
            let r = d2d(tile.rect);
            self.fill_rrect(t, r, 10.0, theme::fade(self.pal.well, shown * 1.2));

            let icon = D2D_RECT_F {
                left: (r.left + r.right) * 0.5 - 16.0,
                top: r.top + 6.0,
                right: (r.left + r.right) * 0.5 + 16.0,
                bottom: r.top + 38.0,
            };
            let drawn = snap.icons.get(&entry.path).is_some_and(|icon_px| {
                self.draw_icon(t, &entry.path, icon_px, icon, shown, generation)
            });
            if !drawn {
                self.fill_rrect(t, icon, 7.0, theme::fade(self.pal.well, shown * 2.5));
            }
            self.centred_label(
                t,
                family,
                &entry.name,
                9.5,
                DWRITE_FONT_WEIGHT_MEDIUM,
                D2D_RECT_F {
                    left: r.left + 3.0,
                    right: r.right - 3.0,
                    ..r
                },
                r.top + 41.0,
                theme::fade(self.pal.text_mid, shown),
            );
        }
    }

    /// Draw a shell icon, uploading it to the render target the first time.
    fn draw_icon(
        &self,
        t: &ID2D1DCRenderTarget,
        path: &str,
        icon: &Arc<Icon>,
        dst: D2D_RECT_F,
        alpha: f32,
        generation: u64,
    ) -> bool {
        let mut cache = self.icons.borrow_mut();
        if cache.generation != generation {
            cache.bitmaps.clear();
            cache.generation = generation;
        }
        let stale = cache
            .bitmaps
            .get(path)
            .is_none_or(|(source, _)| !Arc::ptr_eq(source, icon));
        if stale {
            let Some(bitmap) = upload_icon(t, icon) else {
                return false;
            };
            cache
                .bitmaps
                .insert(path.to_owned(), (Arc::clone(icon), bitmap));
        }
        let Some((_, bitmap)) = cache.bitmaps.get(path) else {
            return false;
        };
        // Fit inside the box, keeping the picture's shape.
        let (w, h) = (icon.width as f32, icon.height as f32);
        let scale = ((dst.right - dst.left) / w).min((dst.bottom - dst.top) / h);
        let (dw, dh) = (w * scale, h * scale);
        let cx = (dst.left + dst.right) * 0.5;
        let cy = (dst.top + dst.bottom) * 0.5;
        let fit = D2D_RECT_F {
            left: cx - dw * 0.5,
            top: cy - dh * 0.5,
            right: cx + dw * 0.5,
            bottom: cy + dh * 0.5,
        };
        unsafe {
            t.DrawBitmap(
                bitmap,
                Some(&fit),
                alpha,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
        true
    }

    // -- Clipboard --------------------------------------------------------------

    pub(super) fn paint_clipboard(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &AppConfig,
        body: D2D_RECT_F,
        alpha: f32,
    ) {
        let family = &cfg.notch.font_family;
        let cy = (body.top + body.bottom) * 0.5;

        if !cfg.notch.clipboard.enabled {
            self.glyph(
                t,
                GLYPH_PASTE,
                20.0,
                (body.left + body.right) * 0.5,
                cy - 26.0,
                theme::fade(self.pal.text_lo, alpha),
            );
            self.centred_label(
                t,
                family,
                "Clipboard history is off",
                13.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                body,
                cy - 6.0,
                theme::fade(self.pal.text_hi, alpha),
            );
            self.centred_label(
                t,
                family,
                "Turn it on in Settings \u{203A} Slides \u{203A} Clipboard. Kept in memory only.",
                11.0,
                DWRITE_FONT_WEIGHT_MEDIUM,
                body,
                cy + 14.0,
                theme::fade(self.pal.text_lo, alpha),
            );
            return;
        }

        let items = clipboard::items();
        self.label(
            t,
            family,
            &format!("CLIPBOARD  \u{00B7}  {}", items.len()),
            theme::SIZE_LABEL - 1.0,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
            theme::TRACK_LABEL,
            body.right - body.left - 70.0,
            body.left,
            body.top,
            theme::fade(self.pal.text_lo, alpha),
        );

        if items.is_empty() {
            self.centred_label(
                t,
                family,
                "Copy some text and it shows up here. Click a row to copy it again.",
                12.0,
                DWRITE_FONT_WEIGHT_MEDIUM,
                body,
                cy - 4.0,
                theme::fade(self.pal.text_lo, alpha),
            );
            return;
        }

        let clear = d2d(layout::clipboard_clear_button(rect(body)));
        self.label(
            t,
            family,
            "CLEAR",
            theme::SIZE_LABEL - 1.0,
            DWRITE_FONT_WEIGHT_BOLD,
            theme::TRACK_LABEL,
            clear.right - clear.left,
            clear.left + 8.0,
            body.top,
            theme::fade(cfg.notch.accent, alpha),
        );

        let rows = layout::clipboard_rows(rect(body), items.len());
        for (row, text) in rows.iter().zip(items.iter()) {
            let r = d2d(*row);
            self.fill_rrect(t, r, 7.0, theme::fade(self.pal.well, alpha * 1.1));
            self.label(
                t,
                family,
                &clipboard::one_line(text),
                11.5,
                DWRITE_FONT_WEIGHT_MEDIUM,
                0.0,
                r.right - r.left - 20.0,
                r.left + 10.0,
                r.top + 6.0,
                theme::fade(self.pal.text_hi, alpha * 0.92),
            );
        }
    }
}

/// Upload premultiplied BGRA pixels as a bitmap for this render target.
fn upload_icon(t: &ID2D1DCRenderTarget, icon: &Icon) -> Option<ID2D1Bitmap> {
    let props = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
    };
    unsafe {
        t.CreateBitmap(
            D2D_SIZE_U {
                width: icon.width,
                height: icon.height,
            },
            Some(icon.bgra.as_ptr() as *const std::ffi::c_void),
            icon.width * 4,
            &props,
        )
        .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::clock_text;

    #[test]
    fn times_read_like_a_player() {
        assert_eq!(clock_text(0.0), "0:00");
        assert_eq!(clock_text(187.4), "3:07");
        assert_eq!(clock_text(3765.0), "1:02:45");
    }
}
