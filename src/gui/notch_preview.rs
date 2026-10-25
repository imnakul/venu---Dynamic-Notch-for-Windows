//! A live miniature of the notch for the Shape and Layout tabs.
//!
//! Built from the very outline the real notch fills ([`NotchShape`]), the
//! user's [`ShapeConfig`], the theme's palette and the split/pod layout
//! maths in [`crate::notch::layout`], so what the sliders show is what the
//! notch draws. It is a static drawing: it asks egui for no frames of its own,
//! so an open Settings window costs nothing while you are not touching it.

use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Rounding, Sense, Shape, Stroke, Vec2};

use crate::config::{AppConfig, SlideKind};
use crate::notch::geom::NotchShape;
use crate::notch::layout::{self, PodKind};
use crate::notch::theme::Palette;

/// Which state of the notch to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The resting pill.
    Pill,
    /// The open panel (a scaled-down stand-in).
    Panel,
}

/// What to draw inside the pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// A plain face, for judging the silhouette.
    Plain,
    /// Split segments and satellite pods as configured on the Layout tab.
    Layout,
}

const HEIGHT: f32 = 156.0;
const SIDE_MARGIN: f32 = 18.0;

fn rgba(c: [f32; 4], alpha: f32) -> Color32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(byte(c[0]), byte(c[1]), byte(c[2]), byte(c[3] * alpha))
}

/// Draw the preview across the available width.
pub fn show(ui: &mut egui::Ui, cfg: &AppConfig, mode: Mode, content: Content) {
    let (frame, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), HEIGHT), Sense::hover());
    let painter = ui.painter_at(frame);
    paint_desktop(&painter, frame);

    let look = &cfg.notch.shape;
    let pal = palette(cfg);
    let floating = cfg.notch.offset_y > 0;
    let top = frame.top() + if floating { 12.0 } else { 0.0 };
    let cx = frame.center().x;

    if mode == Mode::Panel {
        let (w, h) = (300.0, 104.0);
        let pill_h = cfg.notch.collapsed_height as f32;
        let shape = slab(
            cx - w * 0.5,
            top,
            w,
            h,
            look.radius_at(pill_h, 1.0) * 0.8,
            look.shoulder_at(1.0) * 0.8,
            floating,
        );
        paint_slab(&painter, &shape, cfg, &pal, 1.0);
        let ink = rgba(pal.text_hi, 1.0);
        painter.text(
            Pos2::new(shape.left + 26.0, shape.top + 34.0),
            egui::Align2::LEFT_CENTER,
            "9:41",
            egui::FontId::monospace(26.0),
            ink,
        );
        painter.text(
            Pos2::new(shape.left + 28.0, shape.top + 62.0),
            egui::Align2::LEFT_CENTER,
            "TUESDAY",
            egui::FontId::monospace(9.0),
            rgba(pal.text_lo, 1.0),
        );
        return;
    }

    // -- the pill, with whatever the layout puts beside and around it -------
    let h = (cfg.notch.collapsed_height as f32).clamp(24.0, 52.0);
    let center_w = (cfg.notch.collapsed_width as f32).clamp(110.0, 240.0);
    let (left_kind, right_kind) = if content == Content::Layout {
        let [l, r] = layout::split_targets(&cfg.notch.split, None);
        (l, r)
    } else {
        (None, None)
    };
    let left_w = left_kind.map(layout::segment_width).unwrap_or(0.0);
    let right_w = right_kind.map(layout::segment_width).unwrap_or(0.0);

    let shoulder = if floating { 0.0 } else { look.shoulder_at(0.0) };
    let pod_shoulder = if floating {
        0.0
    } else {
        layout::pod_shoulder(shoulder)
    };
    let pods: Vec<(usize, PodKind)> = if content == Content::Layout && cfg.notch.pods.enabled {
        // A representative pair: an agent at work and a running timer.
        [PodKind::Agent, PodKind::Timer]
            .into_iter()
            .take(cfg.notch.pods.limit())
            .enumerate()
            .collect()
    } else {
        Vec::new()
    };
    let pod_reach: f32 = pods
        .iter()
        .map(|(_, k)| k.width() + layout::POD_GAP + shoulder + pod_shoulder * 2.0)
        .fold(0.0, f32::max);

    // Scale everything down if the real thing would not fit the column.
    let natural = left_w + center_w + right_w + 2.0 * (shoulder + pod_reach);
    let scale = ((frame.width() - SIDE_MARGIN * 2.0) / natural).min(1.0);
    let s = |v: f32| v * scale;

    let w = s(left_w + center_w + right_w);
    let radius = look.radius_at(h, 0.0);
    let main = slab(cx - w * 0.5, top, w, h, radius, shoulder, floating);

    // Pods first: in the notch they bud out from under the slab.
    for (index, kind) in &pods {
        let (l, r) = layout::pod_span(
            *index,
            1.0,
            main.left,
            main.right,
            main.flare,
            s(kind.width()),
            pod_shoulder,
        );
        let pod = slab(l, top, r - l, h, radius, pod_shoulder, floating);
        paint_slab(&painter, &pod, cfg, &pal, 1.0);
        paint_pod_face(&painter, &pod, *kind, cfg, &pal);
    }

    paint_slab(&painter, &main, cfg, &pal, 1.0);

    // Segments, left to right, with a hairline between them.
    let mut x = main.left;
    let parts = [
        (left_kind, s(left_w)),
        (None, s(center_w)),
        (right_kind, s(right_w)),
    ];
    let mut first = true;
    for (i, (kind, width)) in parts.into_iter().enumerate() {
        if width <= 0.5 {
            continue;
        }
        if !first {
            painter.line_segment(
                [
                    Pos2::new(x, main.top + h * 0.3),
                    Pos2::new(x, main.bottom - h * 0.3),
                ],
                Stroke::new(1.0_f32, rgba(pal.rule, 1.0)),
            );
        }
        first = false;
        let cell = Rect::from_min_max(Pos2::new(x, main.top), Pos2::new(x + width, main.bottom));
        let split_shown = left_kind.is_some() || right_kind.is_some();
        let text = match (i, kind) {
            (1, _) if split_shown => "Main face",
            (1, _) => "9:41",
            (_, Some(kind)) => sample(kind),
            _ => "",
        };
        painter.text(
            cell.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional((12.0 * scale).max(9.5)),
            rgba(if i == 1 { pal.text_mid } else { pal.text_hi }, 1.0),
        );
        x += width;
    }

    if content == Content::Plain {
        painter.circle_filled(
            Pos2::new(main.left + 20.0, main.top + h * 0.5),
            3.0,
            rgba(cfg.notch.accent, 1.0),
        );
    }
}

/// The theme's palette with the user's finish strengths applied, exactly as
/// the painter does at the start of every frame.
fn palette(cfg: &AppConfig) -> Palette {
    let mut pal = Palette::for_theme(cfg.notch.theme);
    let look = &cfg.notch.shape;
    pal.edge[3] *= look.outline_strength();
    pal.sheen[3] *= look.sheen_strength();
    pal.shadow_strength *= look.shadow_strength();
    pal
}

fn slab(
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    radius: f32,
    shoulder: f32,
    floating: bool,
) -> NotchShape {
    NotchShape {
        left,
        top,
        right: left + width,
        bottom: top + height,
        radius_top: if floating { radius } else { 0.0 },
        radius_bottom: radius,
        flare: if floating { 0.0 } else { shoulder },
    }
}

/// A soft "desktop" behind the notch, so it reads as sitting on a screen.
fn paint_desktop(painter: &egui::Painter, frame: Rect) {
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
}

/// Shadow, glass, outline and sheen — the painter's slab, in egui.
fn paint_slab(
    painter: &egui::Painter,
    shape: &NotchShape,
    cfg: &AppConfig,
    pal: &Palette,
    alpha: f32,
) {
    // Shadow pooling below the slab.
    let strength = 0.16 * pal.shadow_strength * alpha;
    for i in (1..=6).rev() {
        let grow = i as f32 * 2.0;
        let a = strength * (1.0 - i as f32 / 7.0).powi(2);
        if a <= 0.004 {
            continue;
        }
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(shape.left - grow, shape.bottom - shape.radius_bottom),
                Pos2::new(shape.right + grow, shape.bottom + grow * 1.25),
            ),
            Rounding::same(shape.radius_bottom + grow),
            Color32::from_black_alpha((a * 255.0) as u8),
        );
    }

    // The slab: a fan from inside it round the rim.
    let rim: Vec<Pos2> = shape
        .rim_points()
        .into_iter()
        .map(|(x, y)| Pos2::new(x, y))
        .collect();
    let fill = rgba(cfg.notch.surface, alpha);
    let centre = Pos2::new(
        (shape.left + shape.right) * 0.5,
        shape.top + shape.height() * 0.45,
    );
    let mut mesh = Mesh::default();
    mesh.colored_vertex(centre, fill);
    for p in &rim {
        mesh.colored_vertex(*p, fill);
    }
    let n = rim.len() as u32;
    for i in 1..n {
        mesh.add_triangle(0, i, i + 1);
    }
    mesh.add_triangle(0, n, 1);
    painter.add(Shape::mesh(mesh));

    // Hairline round the free edges (never along a fused top: that is the
    // screen's edge), then the specular lip across the top of the slab.
    let edge = rgba(pal.edge, alpha);
    if edge.a() > 0 {
        painter.add(Shape::line(rim.clone(), Stroke::new(1.0_f32, edge)));
        if !shape.fused() {
            painter.line_segment([rim[rim.len() - 1], rim[0]], Stroke::new(1.0_f32, edge));
        }
    }
    let sheen = rgba(pal.sheen, alpha);
    if sheen.a() > 0 {
        let lip = shape.top + shape.height() * 0.42;
        let inner: Vec<Pos2> = shape
            .inset(0.9)
            .rim_points()
            .into_iter()
            .map(|(x, y)| Pos2::new(x, y))
            .collect();
        for pair in inner.windows(2) {
            if pair[0].y <= lip && pair[1].y <= lip {
                painter.line_segment([pair[0], pair[1]], Stroke::new(1.4_f32, sheen));
            }
        }
    }
}

/// Something representative for a side segment to show.
fn sample(kind: SlideKind) -> &'static str {
    match kind {
        SlideKind::Clock => "9:41 AM",
        SlideKind::Stats => "CPU 12%  RAM 41%",
        SlideKind::Media => "Lofi Study",
        SlideKind::Marquee => "Stay focused...",
        SlideKind::Timer => "FOCUS 24:13",
        SlideKind::Calendar => "12m Standup",
        SlideKind::Usage => "Context 64%",
        SlideKind::Notifications => "2 new",
        SlideKind::Status => "Ship the notch",
        SlideKind::Wallpaper => "Photo",
        SlideKind::Controls => "Toggles",
    }
}

fn paint_pod_face(
    painter: &egui::Painter,
    pod: &NotchShape,
    kind: PodKind,
    cfg: &AppConfig,
    pal: &Palette,
) {
    let cy = (pod.top + pod.bottom) * 0.5;
    let ink = rgba(pal.text_hi, 1.0);
    let mono = egui::FontId::monospace(11.0);
    match kind {
        PodKind::Agent => {
            let colour = cfg.notch.notifications.get_app_color("Claude");
            painter.circle_filled(Pos2::new(pod.left + 16.0, cy), 4.0, rgba(colour, 1.0));
            painter.text(
                Pos2::new(pod.right - 12.0, cy),
                egui::Align2::RIGHT_CENTER,
                "0:42",
                mono,
                ink,
            );
        }
        _ => {
            let centre = Pos2::new(pod.left + 16.0, cy);
            painter.circle_stroke(centre, 6.0, Stroke::new(2.0_f32, rgba(pal.rule, 1.0)));
            painter.add(Shape::line(
                (0..=18)
                    .map(|i| {
                        let a = -std::f32::consts::FRAC_PI_2
                            + i as f32 / 18.0 * std::f32::consts::TAU * 0.62;
                        centre + Vec2::new(a.cos(), a.sin()) * 6.0
                    })
                    .collect(),
                Stroke::new(2.0_f32, rgba(cfg.notch.accent, 1.0)),
            ));
            painter.text(
                Pos2::new(pod.right - 12.0, cy),
                egui::Align2::RIGHT_CENTER,
                "24:13",
                mono,
                ink,
            );
        }
    }
}
