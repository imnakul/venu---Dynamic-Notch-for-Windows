//! Design tokens for the notch.
//!
//! The notch is a single instrument panel: one surface colour, one warm accent,
//! and a three-step type ramp. Keeping every value here means the painting code
//! never invents a colour inline — and it is what makes the three themes a
//! table lookup rather than a set of branches scattered through the painter.

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

use crate::config::NotchTheme;

/// sRGB + alpha, straight (not premultiplied).
pub type Rgba = [f32; 4];

// -- Palette -----------------------------------------------------------------

/// Every colour that changes with the theme.
///
/// The painter takes one of these and never reaches for a constant, so adding
/// a theme is a matter of adding a row here rather than hunting through the
/// drawing code for hardcoded greys.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Hairline separating the slab from whatever is behind it.
    pub edge: Rgba,
    /// Specular band along the top lip; sells the "glass" read.
    pub sheen: Rgba,
    /// Divider rules inside the panel.
    pub rule: Rgba,
    /// Fill behind inset content (list rows, thumbnails).
    pub well: Rgba,

    pub text_hi: Rgba,
    pub text_mid: Rgba,
    pub text_lo: Rgba,

    /// Colour the caption scrim and marquee edge-fades tend toward. Dark on
    /// dark themes, light on light ones.
    pub scrim: Rgba,

    /// How hard the drop shadow falls. A light panel on a light desktop needs
    /// more of it than a black slab does.
    pub shadow_strength: f32,

    /// Whether this theme wants a blurred capture of the screen behind it.
    pub blur_backdrop: bool,

    /// Downscale factor for the blur capture (e.g. 8 for frosted, 18 for heavy blur).
    pub blur_downscale: i32,
}

impl Palette {
    pub fn for_theme(theme: NotchTheme) -> Self {
        match theme {
            NotchTheme::Dark => Self {
                edge: [1.0, 1.0, 1.0, 0.085],
                sheen: [1.0, 1.0, 1.0, 0.055],
                rule: [1.0, 1.0, 1.0, 0.07],
                well: [1.0, 1.0, 1.0, 0.035],
                text_hi: [0.957, 0.957, 0.969, 1.0],
                text_mid: [0.604, 0.604, 0.651, 1.0],
                text_lo: [0.361, 0.361, 0.408, 1.0],
                scrim: [0.0, 0.0, 0.0, 1.0],
                shadow_strength: 1.0,
                blur_backdrop: false,
                blur_downscale: 0,
            },

            // Not merely the dark ramp inverted: dark-on-light needs more
            // contrast at the bottom of the ramp to stay legible, and far less
            // sheen, because a bright panel has no headroom for a highlight.
            NotchTheme::Light => Self {
                edge: [0.0, 0.0, 0.0, 0.12],
                sheen: [1.0, 1.0, 1.0, 0.5],
                rule: [0.0, 0.0, 0.0, 0.10],
                well: [0.0, 0.0, 0.0, 0.05],
                text_hi: [0.07, 0.07, 0.10, 1.0],
                text_mid: [0.35, 0.35, 0.40, 1.0],
                text_lo: [0.55, 0.55, 0.60, 1.0],
                scrim: [1.0, 1.0, 1.0, 1.0],
                shadow_strength: 1.35,
                blur_backdrop: false,
                blur_downscale: 0,
            },

            // Frosted sits on a live blur of the screen, so its type has to
            // survive an unknown background: the ramp is pushed brighter and
            // the edge harder than Dark's.
            NotchTheme::Frosted => Self {
                edge: [1.0, 1.0, 1.0, 0.20],
                sheen: [1.0, 1.0, 1.0, 0.16],
                rule: [1.0, 1.0, 1.0, 0.14],
                well: [1.0, 1.0, 1.0, 0.10],
                text_hi: [1.0, 1.0, 1.0, 1.0],
                text_mid: [0.86, 0.87, 0.91, 1.0],
                text_lo: [0.68, 0.69, 0.74, 1.0],
                scrim: [0.0, 0.0, 0.0, 1.0],
                shadow_strength: 0.8,
                blur_backdrop: true,
                blur_downscale: 8,
            },

            // Transparent offers pure clear see-through glass without blur.
            // Crisp edge definition and high-contrast type keep it easily readable over any desktop.
            NotchTheme::Transparent => Self {
                edge: [1.0, 1.0, 1.0, 0.28],
                sheen: [1.0, 1.0, 1.0, 0.22],
                rule: [1.0, 1.0, 1.0, 0.18],
                well: [0.0, 0.0, 0.0, 0.20],
                text_hi: [1.0, 1.0, 1.0, 1.0],
                text_mid: [0.90, 0.90, 0.94, 1.0],
                text_lo: [0.72, 0.73, 0.78, 1.0],
                scrim: [0.0, 0.0, 0.0, 0.85],
                shadow_strength: 1.1,
                blur_backdrop: false,
                blur_downscale: 0,
            },

            // Blurred uses deep, heavy diffusion blur (downscale factor 18)
            // for an ultra-smooth, creamy ambient glass look.
            NotchTheme::Blurred => Self {
                edge: [1.0, 1.0, 1.0, 0.16],
                sheen: [1.0, 1.0, 1.0, 0.12],
                rule: [1.0, 1.0, 1.0, 0.12],
                well: [1.0, 1.0, 1.0, 0.08],
                text_hi: [1.0, 1.0, 1.0, 1.0],
                text_mid: [0.84, 0.85, 0.89, 1.0],
                text_lo: [0.65, 0.66, 0.70, 1.0],
                scrim: [0.0, 0.0, 0.0, 1.0],
                shadow_strength: 0.85,
                blur_backdrop: true,
                blur_downscale: 18,
            },

            // Acrylic offers Windows Fluent Acrylic styling with balanced diffusion and rich ambient tint.
            NotchTheme::Acrylic => Self {
                edge: [1.0, 1.0, 1.0, 0.18],
                sheen: [1.0, 1.0, 1.0, 0.14],
                rule: [1.0, 1.0, 1.0, 0.12],
                well: [1.0, 1.0, 1.0, 0.06],
                text_hi: [0.98, 0.98, 1.0, 1.0],
                text_mid: [0.82, 0.83, 0.88, 1.0],
                text_lo: [0.62, 0.63, 0.68, 1.0],
                scrim: [0.0, 0.0, 0.0, 1.0],
                shadow_strength: 0.9,
                blur_backdrop: true,
                blur_downscale: 12,
            },
        }
    }
}

// -- Type scale --------------------------------------------------------------

/// Tracked, uppercase micro-label ("TODAY", "RIGHT NOW").
pub const SIZE_LABEL: f32 = 10.5;
pub const TRACK_LABEL: f32 = 1.7;
pub const SIZE_BODY: f32 = 13.0;
pub const SIZE_LEAD: f32 = 18.0;
pub const SIZE_CLOCK: f32 = 56.0;
pub const SIZE_PILL: f32 = 13.0;

// -- Metrics -----------------------------------------------------------------

/// Breathing room around the slab inside the window, reserved for the shadow.
pub const SHADOW_PAD: i32 = 44;

/// How far the shadow reaches past the silhouette at a given expansion.
///
/// Shared by the painter and the window, which trims itself to the silhouette
/// plus this. If the two disagreed the shadow would be sliced off by the
/// window edge, so they read it from here rather than each holding a copy.
pub fn shadow_spread(expand: f32) -> f32 {
    crate::notch::anim::lerp(10.0, 34.0, expand.clamp(0.0, 1.0))
}
/// Panel padding when expanded.
pub const GUTTER: f32 = 26.0;
/// Padding inside the collapsed pill.
pub const PILL_GUTTER: f32 = 14.0;
/// Fraction of the panel width given to the left half of a split slide.
pub const SPLIT_RATIO: f32 = 0.44;
/// Corner radius of the expanded panel.
pub const RADIUS_EXPANDED: f32 = 26.0;
/// How far the concave shoulders reach along the bezel and down the side
/// walls while fused to it, collapsed and fully open. Reach, not radius: the
/// smoothed curve reads like a circle of about two thirds of this.
pub const FLARE: f32 = 16.0;
pub const FLARE_EXPANDED: f32 = 24.0;

/// Straight RGBA to the premultiplied colour a layered-window render target
/// needs. Every brush in the notch goes through here.
pub fn premul(c: Rgba) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c[0] * c[3],
        g: c[1] * c[3],
        b: c[2] * c[3],
        a: c[3],
    }
}

/// Scale a colour's alpha, e.g. to fade content in behind the opening shape.
pub fn fade(c: Rgba, alpha: f32) -> Rgba {
    [c[0], c[1], c[2], c[3] * alpha.clamp(0.0, 1.0)]
}
