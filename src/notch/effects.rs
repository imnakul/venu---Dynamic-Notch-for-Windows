//! Border effects: short flourishes the notch plays on its own.
//!
//! Cost is the whole design constraint. An effect is idle between runs — the
//! schedule lives in [`NotchState`](super::state::NotchState) and the frame
//! key only changes while one is playing, so a 30-second interval means the
//! notch repaints for a couple of seconds out of every thirty-odd and not at
//! all in between. While one plays it is one or two Direct2D calls a frame: a
//! gradient brush slid along the rim (or across the glass), never a polyline
//! of hundreds of segments.
//!
//! Each effect is described in two pure halves — its colour stops, and where
//! the gradient sits at a given moment — so the settings window can draw a
//! faithful live preview from the same maths the notch paints with.

use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D1_GRADIENT_STOP, D2D_POINT_2F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DCRenderTarget, ID2D1GradientStopCollection, ID2D1PathGeometry, D2D1_EXTEND_MODE_CLAMP,
    D2D1_EXTEND_MODE_WRAP, D2D1_GAMMA_2_2, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
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

/// Northern-lights greens drifting into violet, wrapping.
const AURORA: [[f32; 3]; 5] = [
    [0.20, 1.00, 0.62],
    [0.16, 0.78, 0.86],
    [0.55, 0.36, 1.00],
    [0.16, 0.78, 0.86],
    [0.20, 1.00, 0.62],
];

/// Embers to flame tips, wrapping.
const FIRE: [[f32; 3]; 5] = [
    [1.00, 0.18, 0.05],
    [1.00, 0.52, 0.08],
    [1.00, 0.86, 0.32],
    [1.00, 0.52, 0.08],
    [1.00, 0.18, 0.05],
];

/// Tilt of the glass reflection band, from vertical.
const SHINE_ANGLE_DEG: f32 = 20.0;

/// How a gradient behaves past its ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extend {
    Clamp,
    Wrap,
}

/// One colour stop, straight (not premultiplied) RGBA.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub position: f32,
    pub rgba: [f32; 4],
}

/// A gradient's start and end points.
pub type Axis = ((f32, f32), (f32, f32));

/// What one frame of an effect draws.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    /// Stroke the rim once per `(width, opacity)` pass, widest first, so a
    /// soft halo sits under a crisp core.
    Stroke(Vec<(f32, f32)>),
    /// Fill the whole slab.
    Fill,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Draw {
    pub axis: Axis,
    pub opacity: f32,
    pub mode: Mode,
}

/// The colour stops of `fx`. They depend only on the effect and its colour,
/// never on time, which is what lets the notch build them once.
pub fn stops(fx: &BorderEffectConfig) -> Option<(Vec<Stop>, Extend)> {
    let c = fx.color;
    let rgb = [c[0], c[1], c[2]];
    let white = [1.0, 1.0, 1.0];
    // Colours here are straight alpha; the ends of each band fade to the
    // band's own colour at zero alpha, not to black, so they never fringe.
    let at = |position: f32, rgb: [f32; 3], a: f32| Stop {
        position,
        rgba: [rgb[0], rgb[1], rgb[2], a],
    };
    let ramp = |hues: &[[f32; 3]]| -> Vec<Stop> {
        hues.iter()
            .enumerate()
            .map(|(i, hue)| at(i as f32 / (hues.len() - 1) as f32, *hue, 1.0))
            .collect()
    };

    Some(match fx.effect {
        BorderEffect::Off => return None,
        BorderEffect::Rainbow => (ramp(&RAINBOW), Extend::Wrap),
        BorderEffect::Aurora => (ramp(&AURORA), Extend::Wrap),
        BorderEffect::Fire => (ramp(&FIRE), Extend::Wrap),
        BorderEffect::SweepRight | BorderEffect::SweepLeft | BorderEffect::SweepBounce => (
            vec![at(0.0, rgb, 0.0), at(0.5, rgb, c[3]), at(1.0, rgb, 0.0)],
            Extend::Clamp,
        ),
        BorderEffect::Comet => (
            // A long fading tail behind a white-hot head.
            vec![
                at(0.0, rgb, 0.0),
                at(0.72, rgb, c[3] * 0.45),
                at(
                    0.95,
                    [
                        lerp(c[0], 1.0, 0.6),
                        lerp(c[1], 1.0, 0.6),
                        lerp(c[2], 1.0, 0.6),
                    ],
                    c[3],
                ),
                at(1.0, rgb, 0.0),
            ],
            Extend::Clamp,
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
            Extend::Clamp,
        ),
        BorderEffect::Breathe | BorderEffect::Neon | BorderEffect::Heartbeat => {
            (vec![at(0.0, rgb, c[3]), at(1.0, rgb, c[3])], Extend::Clamp)
        }
    })
}

/// A soft bump centred on `at`, `width` wide.
fn bump(p: f32, at: f32, width: f32) -> f32 {
    let d = (p - at) / width;
    (-d * d).exp()
}

/// Where the gradient sits at `progress` (0 to 1 through a run), and how
/// strongly it shows.
pub fn frame(fx: &BorderEffectConfig, shape: &NotchShape, progress: f32) -> Option<Draw> {
    let p = progress.clamp(0.0, 1.0);
    let ease = p * p * (3.0 - 2.0 * p);
    // Fade in and out so a run never pops on or off. A looping effect has no
    // ends to fade, so it holds steady instead.
    let envelope = if fx.continuous {
        1.0
    } else {
        smoothstep(0.0, 0.18, p) * (1.0 - smoothstep(0.82, 1.0, p))
    };
    let stroke = |passes: &[(f32, f32)]| Mode::Stroke(passes.to_vec());
    let flat = ((shape.left, 0.0), (shape.right, 0.0));

    let draw = match fx.effect {
        BorderEffect::Off => return None,

        BorderEffect::Rainbow | BorderEffect::Aurora => {
            // A wrapping hue ramp, slid sideways under the stroke. Looping,
            // it slides exactly one period per run, so one run hands over to
            // the next without a seam.
            let aurora = fx.effect == BorderEffect::Aurora;
            let period = (shape.width() * if aurora { 1.6 } else { 0.8 }).max(120.0);
            let travel = if fx.continuous { 1.0 } else { 1.5 };
            let x = shape.left - p * period * travel;
            // The aurora also swells and settles, twice a run.
            let swell = if aurora {
                0.78 + 0.22 * (p * std::f32::consts::TAU * 2.0).sin()
            } else {
                1.0
            };
            Draw {
                axis: ((x, 0.0), (x + period, 0.0)),
                opacity: envelope * swell,
                mode: if aurora {
                    stroke(&[(9.0, 0.30), (2.2, 0.85)])
                } else {
                    stroke(&[(6.0, 0.28), (1.8, 0.95)])
                },
            }
        }

        BorderEffect::Fire => {
            // Flames climb quickly and never hold still.
            let period = (shape.width() * 0.5).max(90.0);
            let x = shape.left - p * period * 3.0;
            let t = p * std::f32::consts::TAU;
            let flicker = 0.82 + 0.10 * (t * 7.0).sin() + 0.08 * (t * 13.0 + 1.3).sin();
            Draw {
                axis: ((x, 0.0), (x + period, 0.0)),
                opacity: envelope * flicker,
                mode: stroke(&[(8.0, 0.32), (2.0, 0.95)]),
            }
        }

        BorderEffect::SweepRight
        | BorderEffect::SweepLeft
        | BorderEffect::SweepBounce
        | BorderEffect::Comet => {
            let q = match fx.effect {
                BorderEffect::SweepLeft => 1.0 - ease,
                // Out and back, easing at each end.
                BorderEffect::SweepBounce => {
                    let half = if p < 0.5 { p * 2.0 } else { 2.0 - p * 2.0 };
                    half * half * (3.0 - 2.0 * half)
                }
                // A comet keeps its speed; it does not ease.
                BorderEffect::Comet => p,
                _ => ease,
            };
            let comet = fx.effect == BorderEffect::Comet;
            let band = (shape.width() * if comet { 0.6 } else { 0.45 }).max(if comet {
                90.0
            } else {
                60.0
            });
            let reach = shape.flare + band;
            // One-way sweeps enter and leave off the slab. The round trip
            // turns at the far shoulder instead, so its midpoint is seen.
            let far = if fx.effect == BorderEffect::SweepBounce {
                shape.right + shape.flare * 0.5
            } else {
                shape.right + reach
            };
            let centre = lerp(shape.left - reach, far, q);
            Draw {
                axis: ((centre - band * 0.5, 0.0), (centre + band * 0.5, 0.0)),
                // The band enters and leaves from outside the slab, so it
                // needs no envelope of its own.
                opacity: 1.0,
                mode: if comet {
                    stroke(&[(6.0, 0.40), (1.8, 1.0)])
                } else {
                    stroke(&[(7.0, 0.35), (2.0, 1.0)])
                },
            }
        }

        BorderEffect::GlassShine => {
            // A tilted band of light crossing the glass, projected onto an
            // axis so the gradient brush does the whole job.
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
            Draw {
                axis: (
                    (dir.0 * s, dir.1 * s),
                    (dir.0 * (s + band), dir.1 * (s + band)),
                ),
                opacity: if fx.continuous {
                    1.0
                } else {
                    smoothstep(0.0, 0.18, p) * (1.0 - smoothstep(0.82, 1.0, p))
                },
                mode: Mode::Fill,
            }
        }

        BorderEffect::Breathe => Draw {
            // One slow swell of the whole rim.
            axis: flat,
            opacity: (p * std::f32::consts::PI).sin().powi(2),
            mode: stroke(&[(6.0, 0.32), (1.6, 0.9)]),
        },

        BorderEffect::Neon => {
            // A sign stuttering on, holding, and switching off.
            const STUTTER: [f32; 10] = [1.0, 0.0, 0.9, 0.0, 0.0, 1.0, 0.3, 1.0, 1.0, 1.0];
            let on = if p < 0.2 {
                STUTTER[((p / 0.2) * STUTTER.len() as f32) as usize % STUTTER.len()]
            } else if fx.continuous {
                1.0
            } else {
                1.0 - smoothstep(0.85, 1.0, p)
            };
            // A faint hum, so it reads as gas rather than paint.
            let hum = 0.94 + 0.06 * (p * 180.0).sin();
            Draw {
                axis: flat,
                opacity: on * hum,
                mode: stroke(&[(11.0, 0.22), (4.5, 0.45), (1.6, 1.0)]),
            }
        }

        BorderEffect::Heartbeat => Draw {
            // Lub-dub, then rest.
            axis: flat,
            opacity: bump(p, 0.16, 0.05).max(0.75 * bump(p, 0.34, 0.06)),
            mode: stroke(&[(8.0, 0.35), (2.0, 1.0)]),
        },
    };
    Some(draw)
}

/// The colour of a gradient at `(x, y)`, for drawing the preview on the CPU.
pub fn sample(stops: &[Stop], extend: Extend, axis: Axis, x: f32, y: f32) -> [f32; 4] {
    let ((x0, y0), (x1, y1)) = axis;
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len2 = (dx * dx + dy * dy).max(1e-6);
    let mut t = ((x - x0) * dx + (y - y0) * dy) / len2;
    t = match extend {
        Extend::Clamp => t.clamp(0.0, 1.0),
        Extend::Wrap => t.rem_euclid(1.0),
    };
    let Some(first) = stops.first() else {
        return [0.0; 4];
    };
    if t <= first.position {
        return first.rgba;
    }
    for pair in stops.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t <= b.position {
            let span = (b.position - a.position).max(1e-6);
            let f = (t - a.position) / span;
            return [
                lerp(a.rgba[0], b.rgba[0], f),
                lerp(a.rgba[1], b.rgba[1], f),
                lerp(a.rgba[2], b.rgba[2], f),
                lerp(a.rgba[3], b.rgba[3], f),
            ];
        }
    }
    stops.last().map(|s| s.rgba).unwrap_or([0.0; 4])
}

/// Gradient stops are built once and kept until the effect, its colour or
/// the render target changes.
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
    /// Draw `fx` at `progress` (0 to 1 through the run), scaled by
    /// `strength`. `fill` is the slab's silhouette and `rim` its stroked
    /// outline, both already built for the frame.
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
        strength: f32,
    ) {
        let Some(draw) = frame(fx, shape, progress) else {
            return;
        };
        let opacity = draw.opacity * strength;
        if opacity <= 0.004 {
            return;
        }
        let Some(stops) = self.stops(t, generation, fx) else {
            return;
        };
        let ((x0, y0), (x1, y1)) = draw.axis;
        let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
            startPoint: D2D_POINT_2F { x: x0, y: y0 },
            endPoint: D2D_POINT_2F { x: x1, y: y1 },
        };
        let Ok(brush) = (unsafe { t.CreateLinearGradientBrush(&props, None, &stops) }) else {
            return;
        };
        unsafe {
            match &draw.mode {
                Mode::Fill => {
                    brush.SetOpacity(opacity);
                    t.FillGeometry(fill, &brush, None);
                }
                Mode::Stroke(passes) => {
                    for &(width, pass) in passes {
                        brush.SetOpacity(pass * opacity);
                        t.DrawGeometry(rim, &brush, width, None);
                    }
                }
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

        let (list, extend) = stops(fx)?;
        let list: Vec<D2D1_GRADIENT_STOP> = list
            .iter()
            .map(|s| D2D1_GRADIENT_STOP {
                position: s.position,
                color: D2D1_COLOR_F {
                    r: s.rgba[0],
                    g: s.rgba[1],
                    b: s.rgba[2],
                    a: s.rgba[3],
                },
            })
            .collect();
        let extend = match extend {
            Extend::Clamp => D2D1_EXTEND_MODE_CLAMP,
            Extend::Wrap => D2D1_EXTEND_MODE_WRAP,
        };
        let stops = unsafe {
            t.CreateGradientStopCollection(&list, D2D1_GAMMA_2_2, extend)
                .ok()?
        };
        self.stops = Some((key, stops.clone()));
        Some(stops)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> NotchShape {
        NotchShape {
            left: 100.0,
            top: 0.0,
            right: 300.0,
            bottom: 40.0,
            radius_top: 0.0,
            radius_bottom: 12.0,
            flare: 16.0,
        }
    }

    #[test]
    fn every_effect_but_off_draws_something_mid_run() {
        for effect in BorderEffect::ALL {
            let fx = BorderEffectConfig {
                effect,
                ..Default::default()
            };
            let drawn = frame(&fx, &shape(), 0.5);
            assert_eq!(drawn.is_some(), effect != BorderEffect::Off, "{effect:?}");
            assert_eq!(
                stops(&fx).is_some(),
                effect != BorderEffect::Off,
                "{effect:?}"
            );
        }
    }

    #[test]
    fn sampling_follows_the_stops() {
        let s = [
            Stop {
                position: 0.0,
                rgba: [0.0, 0.0, 0.0, 0.0],
            },
            Stop {
                position: 1.0,
                rgba: [1.0, 1.0, 1.0, 1.0],
            },
        ];
        let axis = ((0.0, 0.0), (10.0, 0.0));
        assert_eq!(sample(&s, Extend::Clamp, axis, 5.0, 3.0)[3], 0.5);
        assert_eq!(sample(&s, Extend::Clamp, axis, 20.0, 0.0)[3], 1.0);
        assert!((sample(&s, Extend::Wrap, axis, 12.5, 0.0)[3] - 0.25).abs() < 1e-5);
    }
}
