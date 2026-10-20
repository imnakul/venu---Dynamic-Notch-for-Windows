//! Border effects: short, periodic flourishes the notch plays on its own.
//!
//! Cost is the whole design constraint. An effect is idle between runs — the
//! schedule lives in [`NotchState`](super::state::NotchState) and the frame
//! key only changes while one is playing, so a 30-second interval means the
//! notch repaints for a couple of seconds out of every thirty-odd and not at
//! all in between. While one plays it is one or two Direct2D calls a frame: a
//! gradient brush slid along the rim (or across the glass), never a polyline
//! of hundreds of segments.

use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D1_GRADIENT_STOP, D2D_POINT_2F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DCRenderTarget, ID2D1GradientStopCollection, ID2D1PathGeometry, D2D1_EXTEND_MODE,
    D2D1_EXTEND_MODE_CLAMP, D2D1_EXTEND_MODE_WRAP, D2D1_GAMMA_2_2,
    D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
};

use crate::config::{BorderEffect, BorderEffectConfig};
use crate::notch::anim::{lerp, smoothstep};
use crate::notch::geom::NotchShape;

/// Hues of the rainbow border, wrapping back to the first.
const RAINBOW: [[f32; 3]; 7] = [
    [1.00, 0.30, 0.43],
    [1.00, 0.72, 0.30],
    [1.00, 0.88, 0.30],
    [0.30, 1.00, 0.53],
    [0.30, 0.76, 1.00],
    [0.61, 0.36, 1.00],
    [1.00, 0.30, 0.43],
];

/// Tilt of the glass reflection band, from vertical.
const SHINE_ANGLE_DEG: f32 = 20.0;

/// Gradient stops depend only on the effect and its colour, so they are built
/// once and kept until either changes or the render target is replaced.
#[derive(PartialEq)]
struct StopsKey {
    generation: u64,
    effect: BorderEffect,
    color: [u32; 4],
}

#[derive(Default)]
pub struct EffectPainter {
    stops: Option<(StopsKey, ID2D1GradientStopCollection)>,
}

impl EffectPainter {
    /// Draw `fx` at `progress` (0 to 1 through the run). `fill` is the slab's
    /// silhouette and `rim` its stroked outline, both already built for the
    /// frame.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        t: &ID2D1DCRenderTarget,
        generation: u64,
        fx: &BorderEffectConfig,
        shape: &NotchShape,
        fill: &ID2D1PathGeometry,
        rim: &ID2D1PathGeometry,
        progress: f32,
    ) {
        let p = progress.clamp(0.0, 1.0);
        let ease = p * p * (3.0 - 2.0 * p);
        // Fade in and out so a run never pops on or off.
        let envelope = smoothstep(0.0, 0.18, p) * (1.0 - smoothstep(0.82, 1.0, p));

        let Some(stops) = self.stops(t, generation, fx) else {
            return;
        };

        match fx.effect {
            BorderEffect::Off => {}

            BorderEffect::Rainbow => {
                // A wrapping hue ramp, slid sideways under the stroke.
                let period = (shape.width() * 0.8).max(120.0);
                let x = shape.left - p * period * 1.5;
                let axis = ((x, 0.0), (x + period, 0.0));
                stroke(t, &stops, axis, rim, &[(6.0, 0.28), (1.8, 0.95)], envelope);
            }

            BorderEffect::SweepRight | BorderEffect::SweepLeft | BorderEffect::SweepBounce => {
                let q = match fx.effect {
                    BorderEffect::SweepRight => ease,
                    BorderEffect::SweepLeft => 1.0 - ease,
                    // Out and back, easing at each end.
                    _ => {
                        let half = if p < 0.5 { p * 2.0 } else { 2.0 - p * 2.0 };
                        half * half * (3.0 - 2.0 * half)
                    }
                };
                let band = (shape.width() * 0.45).max(60.0);
                let reach = shape.flare + band;
                // One-way sweeps enter and leave off the slab. The round trip
                // turns at the far shoulder instead, so its midpoint is seen.
                let far = if fx.effect == BorderEffect::SweepBounce {
                    shape.right + shape.flare * 0.5
                } else {
                    shape.right + reach
                };
                let centre = lerp(shape.left - reach, far, q);
                let axis = ((centre - band * 0.5, 0.0), (centre + band * 0.5, 0.0));
                // The band enters and leaves from outside the slab, so it
                // needs no envelope of its own.
                stroke(t, &stops, axis, rim, &[(7.0, 0.35), (2.0, 1.0)], 1.0);
            }

            BorderEffect::GlassShine => {
                // A tilted band of light crossing the glass, projected onto
                // an axis so the gradient brush does the whole job.
                let angle = SHINE_ANGLE_DEG.to_radians();
                let dir = (angle.cos(), angle.sin());
                let project = |x: f32, y: f32| x * dir.0 + y * dir.1;
                let (l, r) = (shape.left - shape.flare, shape.right + shape.flare);
                let corners = [
                    project(l, shape.top),
                    project(r, shape.top),
                    project(l, shape.bottom),
                    project(r, shape.bottom),
                ];
                let lo = corners.iter().copied().fold(f32::MAX, f32::min);
                let hi = corners.iter().copied().fold(f32::MIN, f32::max);
                let band = (shape.height() * 1.6).clamp(70.0, 240.0);
                let s = lerp(lo - band, hi, ease);
                let axis = (
                    (dir.0 * s, dir.1 * s),
                    (dir.0 * (s + band), dir.1 * (s + band)),
                );
                if let Some(brush) = linear_brush(t, &stops, axis) {
                    unsafe {
                        brush.SetOpacity(envelope);
                        t.FillGeometry(fill, &brush, None);
                    }
                }
            }

            BorderEffect::Breathe => {
                // One slow swell of the whole rim. Uniform colour, so the
                // gradient's axis is irrelevant; its stops are all `color`.
                let swell = (p * std::f32::consts::PI).sin().powi(2);
                let axis = ((shape.left, 0.0), (shape.right, 0.0));
                stroke(t, &stops, axis, rim, &[(6.0, 0.32), (1.6, 0.9)], swell);
            }
        }
    }

    fn stops(
        &mut self,
        t: &ID2D1DCRenderTarget,
        generation: u64,
        fx: &BorderEffectConfig,
    ) -> Option<ID2D1GradientStopCollection> {
        let key = StopsKey {
            generation,
            effect: fx.effect,
            color: fx.color.map(f32::to_bits),
        };
        if let Some((cached, stops)) = &self.stops {
            if *cached == key {
                return Some(stops.clone());
            }
        }

        let c = fx.color;
        // Colours here are straight alpha; the ends of each band fade to the
        // band's own colour at zero alpha, not to black, so they never fringe.
        let at = |position: f32, rgb: [f32; 3], a: f32| D2D1_GRADIENT_STOP {
            position,
            color: D2D1_COLOR_F {
                r: rgb[0],
                g: rgb[1],
                b: rgb[2],
                a,
            },
        };
        let rgb = [c[0], c[1], c[2]];
        let white = [1.0, 1.0, 1.0];

        let (list, extend): (Vec<D2D1_GRADIENT_STOP>, D2D1_EXTEND_MODE) = match fx.effect {
            BorderEffect::Off => return None,
            BorderEffect::Rainbow => (
                RAINBOW
                    .iter()
                    .enumerate()
                    .map(|(i, hue)| at(i as f32 / (RAINBOW.len() - 1) as f32, *hue, 1.0))
                    .collect(),
                D2D1_EXTEND_MODE_WRAP,
            ),
            BorderEffect::SweepRight | BorderEffect::SweepLeft | BorderEffect::SweepBounce => (
                vec![at(0.0, rgb, 0.0), at(0.5, rgb, c[3]), at(1.0, rgb, 0.0)],
                D2D1_EXTEND_MODE_CLAMP,
            ),
            BorderEffect::GlassShine => (
                // A broad band with a thin echo behind it, like light off a
                // curved pane.
                vec![
                    at(0.0, white, 0.0),
                    at(0.40, white, 0.0),
                    at(0.50, white, 0.20),
                    at(0.60, white, 0.0),
                    at(0.70, white, 0.0),
                    at(0.76, white, 0.09),
                    at(0.82, white, 0.0),
                    at(1.0, white, 0.0),
                ],
                D2D1_EXTEND_MODE_CLAMP,
            ),
            BorderEffect::Breathe => (
                vec![at(0.0, rgb, c[3]), at(1.0, rgb, c[3])],
                D2D1_EXTEND_MODE_CLAMP,
            ),
        };

        let stops = unsafe {
            t.CreateGradientStopCollection(&list, D2D1_GAMMA_2_2, extend)
                .ok()?
        };
        self.stops = Some((key, stops.clone()));
        Some(stops)
    }
}

type Axis = ((f32, f32), (f32, f32));

fn linear_brush(
    t: &ID2D1DCRenderTarget,
    stops: &ID2D1GradientStopCollection,
    ((x0, y0), (x1, y1)): Axis,
) -> Option<windows::Win32::Graphics::Direct2D::ID2D1LinearGradientBrush> {
    let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
        startPoint: D2D_POINT_2F { x: x0, y: y0 },
        endPoint: D2D_POINT_2F { x: x1, y: y1 },
    };
    unsafe { t.CreateLinearGradientBrush(&props, None, stops).ok() }
}

/// Stroke `rim` once per `(width, opacity)` pass, widest first, so a soft
/// halo sits under a crisp core.
fn stroke(
    t: &ID2D1DCRenderTarget,
    stops: &ID2D1GradientStopCollection,
    axis: Axis,
    rim: &ID2D1PathGeometry,
    passes: &[(f32, f32)],
    strength: f32,
) {
    if strength <= 0.004 {
        return;
    }
    let Some(brush) = linear_brush(t, stops, axis) else {
        return;
    };
    for &(width, opacity) in passes {
        unsafe {
            brush.SetOpacity(opacity * strength);
            t.DrawGeometry(rim, &brush, width, None);
        }
    }
}
