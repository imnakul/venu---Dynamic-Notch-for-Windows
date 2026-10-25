//! A live, miniature notch on the Border effects tab, playing the chosen
//! effect with the same maths the real notch paints it with
//! ([`crate::notch::effects`]), so what you pick is what you get.
//!
//! It only animates while the tab is on screen: egui asks for frames for as
//! long as it is drawn, and stops the moment it is not.

use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Sense, Shape, Stroke, Vec2};

use crate::config::{BorderEffect, BorderEffectConfig, ShapeConfig};
use crate::notch::effects::{self, Mode};
use crate::notch::geom::NotchShape;

use super::theme;

/// Pause between runs of a periodic effect in the preview. The real
/// interval is usually far longer; nobody wants to wait thirty seconds to see
/// what they picked.
const PREVIEW_GAP_SECS: f64 = 0.9;

fn rgba(c: [f32; 4], alpha: f32) -> Color32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(byte(c[0]), byte(c[1]), byte(c[2]), byte(c[3] * alpha))
}

/// Draw the preview. `open` previews the expanded panel rather than the pill;
/// `accent` is the notch's own accent and `look` its silhouette settings.
pub fn show(
    ui: &mut egui::Ui,
    fx: &BorderEffectConfig,
    open: bool,
    accent: [f32; 4],
    look: &ShapeConfig,
) {
    let height = 150.0;
    let (frame, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let painter = ui.painter_at(frame);

    // A soft "desktop" behind it, so the notch reads as sitting on a screen.
    let mut sky = Mesh::default();
    let top = Color32::from_rgb(38, 44, 74);
    let bottom = Color32::from_rgb(18, 20, 34);
    sky.colored_vertex(frame.left_top(), top);
    sky.colored_vertex(frame.right_top(), top);
    sky.colored_vertex(frame.right_bottom(), bottom);
    sky.colored_vertex(frame.left_bottom(), bottom);
    sky.add_triangle(0, 1, 2);
    sky.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(sky));

    let cx = frame.center().x;
    let (w, h) = if open { (300.0, 104.0) } else { (190.0, 36.0) };
    let shape = NotchShape {
        left: cx - w * 0.5,
        top: frame.top(),
        right: cx + w * 0.5,
        bottom: frame.top() + h,
        radius_top: 0.0,
        // The open panel here is a scaled-down stand-in, so its corners and
        // shoulders scale with it.
        radius_bottom: if open {
            look.radius_at(h, 1.0) * 0.8
        } else {
            look.radius_at(h, 0.0)
        },
        flare: if open {
            look.shoulder_at(1.0) * 0.8
        } else {
            look.shoulder_at(0.0)
        },
    };

    // The slab: a fan from its centre round the rim and back along the top.
    let rim: Vec<Pos2> = shape
        .rim_points()
        .into_iter()
        .map(|(x, y)| Pos2::new(x, y))
        .collect();
    let slab = Color32::from_rgb(10, 10, 13);
    let centre = Pos2::new(cx, shape.top + h * 0.45);
    let mut fill = Mesh::default();
    fill.colored_vertex(centre, slab);
    for p in &rim {
        fill.colored_vertex(*p, slab);
    }
    let n = rim.len() as u32;
    for i in 1..n {
        fill.add_triangle(0, i, i + 1);
    }
    fill.add_triangle(0, n, 1);
    painter.add(Shape::mesh(fill));

    // Something to look at inside it.
    let ink = Color32::from_rgb(236, 236, 242);
    if open {
        painter.text(
            Pos2::new(shape.left + 26.0, shape.top + 30.0),
            egui::Align2::LEFT_CENTER,
            "9:41",
            egui::FontId::monospace(26.0),
            ink,
        );
        painter.text(
            Pos2::new(shape.left + 28.0, shape.top + 58.0),
            egui::Align2::LEFT_CENTER,
            "TUESDAY",
            egui::FontId::monospace(9.0),
            theme::text_tertiary(),
        );
    } else {
        painter.circle_filled(
            Pos2::new(shape.left + 20.0, shape.top + h * 0.5),
            3.0,
            rgba(accent, 1.0),
        );
        painter.text(
            Pos2::new(shape.right - 18.0, shape.top + h * 0.5),
            egui::Align2::RIGHT_CENTER,
            "9:41",
            egui::FontId::monospace(13.0),
            ink,
        );
    }

    let shown = fx.scope.visibility(if open { 1.0 } else { 0.0 });
    if fx.effect == BorderEffect::Off || shown <= 0.0 {
        let note = if fx.effect == BorderEffect::Off {
            "Pick an effect to see it here"
        } else if open {
            "This effect only plays on the pill"
        } else {
            "This effect only plays on the open panel"
        };
        painter.text(
            Pos2::new(cx, frame.bottom() - 16.0),
            egui::Align2::CENTER_CENTER,
            note,
            egui::FontId::proportional(11.0),
            theme::text_tertiary(),
        );
        return;
    }

    // Where in a run we are. Periodic effects get a short gap between runs.
    let time = ui.input(|i| i.time);
    let duration = fx.duration_secs.max(0.1) as f64;
    let progress = if fx.continuous {
        Some((time % duration / duration) as f32)
    } else {
        let t = time % (duration + PREVIEW_GAP_SECS);
        (t <= duration).then(|| (t / duration) as f32)
    };
    ui.ctx().request_repaint();

    let Some(progress) = progress else {
        return;
    };
    let (Some(draw), Some((stops, extend))) =
        (effects::frame(fx, &shape, progress), effects::stops(fx))
    else {
        return;
    };
    let opacity = draw.opacity * shown;
    if opacity <= 0.004 {
        return;
    }
    let colour_at = |x: f32, y: f32, alpha: f32| -> Color32 {
        let c = effects::sample(&stops, extend, draw.axis, x, y);
        rgba(c, alpha)
    };

    match &draw.mode {
        Mode::Stroke(passes) => {
            for &(width, pass) in passes {
                let alpha = pass * opacity;
                for pair in rim.windows(2) {
                    let mid = pair[0].lerp(pair[1], 0.5);
                    painter.line_segment(
                        [pair[0], pair[1]],
                        Stroke::new(width, colour_at(mid.x, mid.y, alpha)),
                    );
                }
            }
        }
        Mode::Fill => {
            // The glass shine: sample the band in small cells over the slab.
            let cell = 3.0;
            let r = shape.radius_bottom;
            // Inside the slab, rounded bottom corners included.
            let inside = |x: f32, y: f32| {
                let corner_y = shape.bottom - r;
                if y <= corner_y {
                    return true;
                }
                let cx = x.clamp(shape.left + r, shape.right - r);
                (x - cx).powi(2) + (y - corner_y).powi(2) <= r * r
            };
            let mut y = shape.top;
            while y < shape.bottom {
                let mut x = shape.left;
                while x < shape.right {
                    if !inside(x + cell * 0.5, y + cell * 0.5) {
                        x += cell;
                        continue;
                    }
                    let c = colour_at(x + cell * 0.5, y + cell * 0.5, opacity);
                    if c.a() > 0 {
                        painter.rect_filled(
                            Rect::from_min_size(Pos2::new(x, y), Vec2::splat(cell)),
                            0.0,
                            c,
                        );
                    }
                    x += cell;
                }
                y += cell;
            }
        }
    }
}
