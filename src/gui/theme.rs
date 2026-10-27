//! The settings window's palette.
//!
//! The window is glass: Windows blurs whatever is behind it (see
//! [`super::glass`]) and every surface here is a translucent tint laid over
//! that blur. One number — the user's glass strength — decides how much of
//! the desktop shows through, and the whole palette is derived from it.
//!
//! Every colour in the settings UI comes from here rather than from a constant
//! at the call site. The accessors are free functions reading a thread-local,
//! so a call site stays as short as a constant was.

use std::cell::Cell;

use eframe::egui::{self, Color32, Rounding, Shadow, Stroke};

/// Every tone the settings window is allowed to use.
///
/// Kept flat and `Copy` on purpose: it is read dozens of times per frame, so
/// an allocation anywhere in here would be a mistake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The content area behind the pages.
    pub bg: Color32,
    /// The navigation rail. A shade deeper than `bg` so the two read as
    /// separate panes of the same glass.
    pub sidebar: Color32,
    /// Controls: fields, buttons, cards. A white film rather than a colour,
    /// so it stays glass whatever is behind it.
    pub surface: Color32,
    /// The same, under the cursor.
    pub surface_hover: Color32,
    /// The same, pressed.
    pub surface_active: Color32,
    /// Hairlines and control outlines.
    pub divider: Color32,
    /// Popups and menus. Nearly opaque: a list floating over glass over a
    /// busy desktop is unreadable otherwise.
    pub popup: Color32,
    /// Body copy and control labels.
    pub text_primary: Color32,
    /// Supporting copy.
    pub text_secondary: Color32,
    /// Section headings and hints — the quietest legible tone.
    pub text_tertiary: Color32,
    /// The one saturated colour in the window.
    pub accent: Color32,
    /// A wash of the accent, for the selected navigation row and tab.
    pub accent_wash: Color32,
    /// Type that sits on top of a filled accent.
    pub on_accent: Color32,
}

impl Palette {
    /// The palette for a glass strength from 0 (clearest) to 1 (solid).
    ///
    /// `blurred` is false when Windows refused the blur. The desktop would
    /// then show through sharp, so the glass is made solid instead. `accent`
    /// is the user's colour, as straight RGBA.
    pub fn glass(strength: f32, blurred: bool, accent: [f32; 4]) -> Palette {
        let byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        let rgb = [byte(accent[0]), byte(accent[1]), byte(accent[2])];
        // Dark type on a light accent, light type on a dark one.
        let luma = 0.2126 * accent[0] + 0.7152 * accent[1] + 0.0722 * accent[2];
        let s = if blurred {
            strength.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let tint = |rgb: [u8; 3], alpha: f32| {
            Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], (alpha * 255.0).round() as u8)
        };
        let film = |alpha: f32| Color32::from_white_alpha((alpha * 255.0).round() as u8);

        let bg = 0.28 + 0.70 * s;
        // A breath of the accent in the backgrounds: enough to make the
        // window feel like the user's, little enough that it never competes
        // with what is on it. A touch more on the rail than the page.
        let mix = |base: [u8; 3], amount: f32| -> [u8; 3] {
            let blend =
                |b: u8, a: u8| -> u8 { (b as f32 + (a as f32 - b as f32) * amount).round() as u8 };
            [
                blend(base[0], rgb[0]),
                blend(base[1], rgb[1]),
                blend(base[2], rgb[2]),
            ]
        };
        Palette {
            bg: tint(mix([16, 16, 22], 0.09), bg),
            sidebar: tint(mix([9, 9, 13], 0.12), (bg + 0.14).min(1.0)),
            surface: film(0.06),
            surface_hover: film(0.10),
            surface_active: film(0.15),
            divider: film(0.09),
            popup: tint([22, 22, 29], 0.97),
            text_primary: Color32::from_rgb(244, 244, 247),
            // A touch brighter than on an opaque panel: the glass behind them
            // can be bright.
            text_secondary: Color32::from_rgb(178, 178, 190),
            text_tertiary: Color32::from_rgb(128, 128, 142),
            accent: Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
            accent_wash: tint(rgb, 0.22),
            on_accent: if luma > 0.55 {
                Color32::from_rgb(10, 10, 14)
            } else {
                Color32::WHITE
            },
        }
    }
}

thread_local! {
    /// The palette the accessors below report. Set once per frame, before any
    /// of the UI is built. Thread-local because eframe only ever builds the UI
    /// on one thread, and this avoids a lock on something read this often.
    static CURRENT: Cell<Palette> = Cell::new(Palette::glass(0.6, true, [0.463, 0.580, 0.980, 1.0]));
}

/// Install the palette for this frame.
pub fn set(pal: Palette) {
    CURRENT.with(|c| c.set(pal));
}

/// Push the palette into egui's own style, so stock widgets — checkboxes,
/// sliders, combo boxes, scroll bars — are painted from it too.
///
/// Only called when the palette actually changes, which is while the glass
/// slider is being dragged and never otherwise.
pub fn apply_style(ctx: &egui::Context, pal: Palette) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;

    *v = egui::Visuals::dark();
    v.window_fill = pal.popup;
    v.panel_fill = pal.bg;
    v.window_stroke = Stroke::new(1.0, pal.divider);
    v.window_rounding = Rounding::same(10.0);
    v.menu_rounding = Rounding::same(10.0);
    v.window_shadow = Shadow {
        offset: egui::vec2(0.0, 8.0),
        blur: 24.0,
        spread: 0.0,
        color: Color32::from_black_alpha(110),
    };
    v.popup_shadow = v.window_shadow;
    v.extreme_bg_color = Color32::from_black_alpha(70);
    v.faint_bg_color = pal.surface;
    v.override_text_color = Some(pal.text_primary);
    v.hyperlink_color = pal.accent;
    v.selection.bg_fill = pal.accent;
    v.selection.stroke = Stroke::new(1.0, pal.on_accent);
    v.slider_trailing_fill = true;

    v.widgets.noninteractive.bg_fill = pal.surface;
    v.widgets.noninteractive.weak_bg_fill = pal.surface;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal.divider);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, pal.text_secondary);
    v.widgets.inactive.bg_fill = pal.surface_hover;
    v.widgets.inactive.weak_bg_fill = pal.surface;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, pal.divider);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, pal.text_primary);
    v.widgets.hovered.bg_fill = pal.surface_active;
    v.widgets.hovered.weak_bg_fill = pal.surface_hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, pal.accent.gamma_multiply(0.55));
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, pal.text_primary);
    v.widgets.active.bg_fill = pal.accent;
    v.widgets.active.weak_bg_fill = pal.surface_active;
    v.widgets.active.bg_stroke = Stroke::new(1.0, pal.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.0, pal.text_primary);
    v.widgets.open.bg_fill = pal.surface_hover;
    v.widgets.open.weak_bg_fill = pal.surface_hover;
    v.widgets.open.bg_stroke = Stroke::new(1.0, pal.divider);
    v.widgets.open.fg_stroke = Stroke::new(1.0, pal.text_primary);

    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = Rounding::same(7.0);
        w.expansion = 0.0;
    }

    ctx.set_style(style);
}

// -- accessors -----------------------------------------------------------
//
// One per token. They exist so a call site reads `theme::accent()` and not
// `theme::current().accent`, which at a hundred call sites is the difference
// between the colour layer being invisible and being in the way.

macro_rules! token {
    ($($name:ident),* $(,)?) => {
        $(
            #[inline]
            pub fn $name() -> Color32 {
                CURRENT.with(|c| c.get().$name)
            }
        )*
    };
}

token!(
    bg,
    sidebar,
    surface,
    surface_hover,
    surface_active,
    divider,
    text_primary,
    text_secondary,
    text_tertiary,
    accent,
    accent_wash,
    on_accent,
);
