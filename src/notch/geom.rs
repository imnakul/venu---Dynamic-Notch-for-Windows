//! The notch silhouette.
//!
//! One outline covers every resting state. While the notch is fused to the
//! top bezel it grows *outward* at the very top through a pair of concave
//! shoulders, so the slab melts into the black bezel instead of ending in two
//! hard corners — collapsed and expanded alike. Detached, the shoulders are
//! gone and the top corners round off into a floating panel.
//!
//! Every corner, convex or concave, is a continuous-curvature cubic rather
//! than a circular arc. A circle meeting a straight edge jumps from zero
//! curvature to `1/r` in one pixel, and the eye reads that jump as a kink —
//! most visibly where a shoulder turns into the side wall. The cubic eases
//! in from the straight edge instead, the way Apple's own hardware corners
//! do, at the same visual depth as the circle it replaces.

use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_BEZIER_SEGMENT, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D_POINT_2F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{ID2D1Factory, ID2D1PathGeometry};

/// Anything below this is treated as a hard corner.
const MIN_RADIUS: f32 = 0.5;

/// How far along each edge a smoothed corner starts, as a multiple of the
/// circular radius it stands in for. Together with [`CORNER_PULL`] this puts
/// the curve's midpoint at the same depth as the circle's (0.29 r), so
/// existing radii keep their visual size.
const CORNER_REACH: f32 = 1.5;

/// How far each control point is pulled from the curve's end toward the
/// corner. Close to 1 means curvature starts near zero at both ends, which is
/// what makes the corner continuous with its edges.
const CORNER_PULL: f32 = 0.82;

/// Samples per corner when the outline is flattened for the glow effects.
const CORNER_SAMPLES: usize = 14;

type Point = (f32, f32);

/// One piece of the rim, ending at the point it carries.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Segment {
    Line(Point),
    Cubic(Point, Point, Point),
}

/// The left, bottom and right edges of the silhouette as one open run, from
/// the top-left tip, down, across and back up to the top-right tip. The top
/// edge — fused to the bezel, or the panel's own top — closes it.
///
/// Fixed-size on purpose: built several times a frame, so no allocation.
/// Absent corners are zero-length lines, which Direct2D draws as nothing.
struct Rim {
    start: Point,
    segments: [Segment; 7],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotchShape {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub radius_top: f32,
    pub radius_bottom: f32,
    /// How far each concave shoulder reaches out along the bezel and down the
    /// side wall. Zero for a free-floating panel.
    pub flare: f32,
}

impl NotchShape {
    pub fn width(&self) -> f32 {
        (self.right - self.left).max(0.0)
    }

    pub fn height(&self) -> f32 {
        (self.bottom - self.top).max(0.0)
    }

    /// Whether the top edge is fused to the bezel. A fused top is never
    /// stroked or lit: it is the screen edge, not an edge of the panel.
    pub fn fused(&self) -> bool {
        self.flare > MIN_RADIUS
    }

    /// Inset every edge, used for the inner sheen ring and shadow rings.
    pub fn inset(&self, amount: f32) -> NotchShape {
        NotchShape {
            left: self.left + amount,
            top: self.top + amount,
            right: self.right - amount,
            bottom: self.bottom - amount,
            radius_top: (self.radius_top - amount).max(0.0),
            radius_bottom: (self.radius_bottom - amount).max(0.0),
            flare: (self.flare - amount).max(0.0),
        }
    }

    /// Centre and radius of the settings launcher button.
    pub fn settings_button(&self) -> (f32, f32, f32) {
        (self.right - 43.0, self.top + 19.0, 9.0)
    }

    /// Centre and radius of the pin toggle, in the shape's own coordinates.
    /// Inset from the top-right corner, clear of the shoulder curve — shared
    /// by the painter (to draw it) and the window (to hit-test clicks on it),
    /// so the two can never drift apart.
    pub fn pin_button(&self) -> (f32, f32, f32) {
        (self.right - 19.0, self.top + 19.0, 9.0)
    }

    /// Cheap hit test used for hover and for `WM_NCHITTEST`.
    ///
    /// Deliberately ignores the flare and treats the smoothed corners as
    /// circles: the shoulders only *add* area at the very top, where the
    /// cursor is already inside the main body, and the corner curves differ
    /// from their circles by well under a pixel of hover slop.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        if x < self.left || x > self.right || y < self.top || y > self.bottom {
            return false;
        }

        let corner = |cx: f32, cy: f32, r: f32| -> bool {
            if r <= MIN_RADIUS {
                return true;
            }
            let dx = x - cx;
            let dy = y - cy;
            dx * dx + dy * dy <= r * r
        };

        let rt = self.radius_top;
        let rb = self.radius_bottom;

        if x < self.left + rt && y < self.top + rt {
            return corner(self.left + rt, self.top + rt, rt);
        }
        if x > self.right - rt && y < self.top + rt {
            return corner(self.right - rt, self.top + rt, rt);
        }
        if x < self.left + rb && y > self.bottom - rb {
            return corner(self.left + rb, self.bottom - rb, rb);
        }
        if x > self.right - rb && y > self.bottom - rb {
            return corner(self.right - rb, self.bottom - rb, rb);
        }

        true
    }

    /// The rim, with every corner's reach clamped so neighbouring curves can
    /// never overlap however small the shape gets mid-animation.
    fn rim(&self) -> Rim {
        let (l, t, r, b) = (self.left, self.top, self.right, self.bottom);
        let half_w = self.width() * 0.5;
        let h = self.height();

        // What the top corners take out of each side wall: the shoulder's
        // reach when fused, a rounded corner's when floating.
        let flare = self.flare.clamp(0.0, h * 0.5);
        let fused = flare > MIN_RADIUS;
        let top_reach = if fused {
            flare
        } else {
            (self.radius_top * CORNER_REACH).clamp(0.0, half_w.min(h * 0.5))
        };

        // The bottom corners get whatever height the top ones left over, and
        // may run wider than they are tall when the slab is short.
        let bottom_reach = self.radius_bottom.max(0.0) * CORNER_REACH;
        let bx = bottom_reach.min(half_w);
        let by = bottom_reach.min((h - top_reach).max(0.0));

        let (start, top_left, top_right) = if fused {
            (
                (l - flare, t),
                corner((l - flare, t), (l, t), (l, t + flare)),
                corner((r, t + flare), (r, t), (r + flare, t)),
            )
        } else {
            (
                (l + top_reach, t),
                corner((l + top_reach, t), (l, t), (l, t + top_reach)),
                corner((r, t + top_reach), (r, t), (r - top_reach, t)),
            )
        };

        Rim {
            start,
            segments: [
                top_left,
                Segment::Line((l, b - by)),
                corner((l, b - by), (l, b), (l + bx, b)),
                Segment::Line((r - bx, b)),
                corner((r - bx, b), (r, b), (r, b - by)),
                Segment::Line((r, t + top_reach)),
                top_right,
            ],
        }
    }

    /// Build the filled silhouette.
    pub fn build(&self, factory: &ID2D1Factory) -> windows::core::Result<ID2D1PathGeometry> {
        self.build_outline(factory, true)
    }

    /// Build the outline for a hairline or sheen. A fused top is left open:
    /// stroking it would draw a line along the very edge of the screen.
    pub fn build_edge(&self, factory: &ID2D1Factory) -> windows::core::Result<ID2D1PathGeometry> {
        self.build_outline(factory, !self.fused())
    }

    fn build_outline(
        &self,
        factory: &ID2D1Factory,
        closed: bool,
    ) -> windows::core::Result<ID2D1PathGeometry> {
        let rim = self.rim();
        let geometry = unsafe { factory.CreatePathGeometry()? };
        let sink = unsafe { geometry.Open()? };

        unsafe {
            let begin = if closed {
                D2D1_FIGURE_BEGIN_FILLED
            } else {
                D2D1_FIGURE_BEGIN_HOLLOW
            };
            sink.BeginFigure(pt(rim.start), begin);
            for segment in rim.segments {
                match segment {
                    Segment::Line(to) => sink.AddLine(pt(to)),
                    Segment::Cubic(c1, c2, to) => sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                        point1: pt(c1),
                        point2: pt(c2),
                        point3: pt(to),
                    }),
                }
            }
            // Closing draws the top edge straight back across to the start.
            sink.EndFigure(if closed {
                D2D1_FIGURE_END_CLOSED
            } else {
                D2D1_FIGURE_END_OPEN
            });
            sink.Close()?;
        }

        Ok(geometry)
    }

    /// The rim flattened into points, for the glow effects that walk the
    /// outline by length or colour it segment by segment. Built from the same
    /// [`Rim`] as the fill, so the glow can never drift off the silhouette.
    pub fn rim_points(&self) -> Vec<Point> {
        let rim = self.rim();
        let mut points = Vec::with_capacity(2 + rim.segments.len() * CORNER_SAMPLES);
        points.push(rim.start);
        let mut from = rim.start;

        for segment in rim.segments {
            match segment {
                Segment::Line(to) => {
                    // Long straight runs are split too, so the per-segment
                    // colour cycling stays as fine on the edges as it is
                    // round the corners.
                    let length = (to.0 - from.0).hypot(to.1 - from.1);
                    let steps = (length / 6.0).ceil().max(1.0) as usize;
                    for i in 1..=steps {
                        let f = i as f32 / steps as f32;
                        points.push((from.0 + (to.0 - from.0) * f, from.1 + (to.1 - from.1) * f));
                    }
                    from = to;
                }
                Segment::Cubic(c1, c2, to) => {
                    for i in 1..=CORNER_SAMPLES {
                        points.push(cubic_at(from, c1, c2, to, i as f32 / CORNER_SAMPLES as f32));
                    }
                    from = to;
                }
            }
        }

        points
    }
}

/// A smoothed corner from `from` (the previous segment's end) round `apex` to
/// `to`. Collapses to a line when there is no room for a curve.
fn corner(from: Point, apex: Point, to: Point) -> Segment {
    let reach_in = (apex.0 - from.0).abs() + (apex.1 - from.1).abs();
    let reach_out = (apex.0 - to.0).abs() + (apex.1 - to.1).abs();
    if reach_in <= MIN_RADIUS || reach_out <= MIN_RADIUS {
        return Segment::Line(to);
    }
    let pull = |p: Point| {
        (
            p.0 + (apex.0 - p.0) * CORNER_PULL,
            p.1 + (apex.1 - p.1) * CORNER_PULL,
        )
    };
    Segment::Cubic(pull(from), pull(to), to)
}

fn cubic_at(p0: Point, p1: Point, p2: Point, p3: Point, t: f32) -> Point {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (
        a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
        a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
    )
}

fn pt((x, y): Point) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

/// Content rectangle for a slide's body, derived from its panel. Shared by
/// the painter (to lay out a slide) and the window (to hit-test clicks
/// inside it), so a click always lands on what is actually drawn. Ignores
/// the painter's fade-in rise — that is a few pixels of animation polish,
/// not something a click needs to account for.
pub fn slide_body(panel: NotchShape) -> D2D_RECT_F {
    let g = super::theme::GUTTER;
    D2D_RECT_F {
        left: panel.left + g,
        top: panel.top + g * 0.72,
        right: panel.right - g,
        bottom: panel.bottom - g * 0.72 - 12.0,
    }
}

/// The round + button at the right end of the Status list's heading.
pub fn status_plus_button(body: D2D_RECT_F) -> (f32, f32, f32) {
    (body.right - 9.0, body.top + 6.0, 9.0)
}

/// The Status slide's right-hand list column: right of the divider, under
/// its "ON DECK" heading. Shared by the painter and click handling.
pub fn status_list_column(body: D2D_RECT_F) -> crate::notch::layout::Rect {
    let split = body.left + (body.right - body.left) * super::theme::SPLIT_RATIO;
    crate::notch::layout::Rect::new(split + 20.0, body.top + 24.0, body.right, body.bottom)
}

/// A Now Playing body at least this tall gets the centred, stacked player;
/// a shorter one lays art and text out side by side.
pub const MEDIA_STACKED_MIN_H: f32 = 200.0;

/// Previous / play-pause / next centred along the bottom of `area`.
fn centred_buttons(area: D2D_RECT_F) -> [(f32, f32, f32); 3] {
    let r = 15.0;
    let gap = 34.0;
    let cx = (area.left + area.right) * 0.5;
    let cy = area.bottom - r - 4.0;
    [
        (cx - gap, cy, r * 0.72),
        (cx, cy, r),
        (cx + gap, cy, r * 0.72),
    ]
}

/// The Now Playing slide's art box. Stacked: centred under the top edge,
/// sized to leave room for the text, timeline and buttons below; the width
/// is the art's own (`aspect` = width / height, 1 for album covers, 16:9 for
/// videos). Side by side: a square the height of the body.
pub fn media_art(body: D2D_RECT_F, aspect: f32) -> D2D_RECT_F {
    let w = body.right - body.left;
    let h = body.bottom - body.top;
    if h >= MEDIA_STACKED_MIN_H {
        let art_h = (h - 118.0).clamp(64.0, 320.0);
        let art_w = (art_h * aspect.clamp(0.5, 2.0)).min(w);
        let art_h = art_w / aspect.clamp(0.5, 2.0);
        let cx = (body.left + body.right) * 0.5;
        D2D_RECT_F {
            left: cx - art_w * 0.5,
            top: body.top,
            right: cx + art_w * 0.5,
            bottom: body.top + art_h,
        }
    } else {
        let side = h.min(w * 0.4).max(24.0);
        D2D_RECT_F {
            left: body.left,
            top: body.top,
            right: body.left + side,
            bottom: body.top + side,
        }
    }
}

/// Where the text, timeline and buttons go beside the art in the side by
/// side layout; the whole body when stacked.
pub fn media_text_column(body: D2D_RECT_F) -> D2D_RECT_F {
    if body.bottom - body.top >= MEDIA_STACKED_MIN_H {
        body
    } else {
        let art = media_art(body, 1.0);
        D2D_RECT_F {
            left: art.right + 18.0,
            ..body
        }
    }
}

/// Centres and radius of the Now Playing slide's three transport buttons
/// (previous / play-pause / next), along the bottom of the text column.
/// Shared by the painter and the window for the same draw/hit-test reason as
/// [`NotchShape::pin_button`].
pub fn media_transport_buttons(body: D2D_RECT_F) -> [(f32, f32, f32); 3] {
    centred_buttons(media_text_column(body))
}

/// The focus timer's reset / start-pause / skip buttons.
pub fn timer_buttons(body: D2D_RECT_F) -> [(f32, f32, f32); 3] {
    centred_buttons(body)
}

/// The Quick Toggles slide's tiles, one per switch: a row of four on a wide
/// panel, two by two on a narrow one.
pub fn toggle_tiles(body: D2D_RECT_F) -> [D2D_RECT_F; 4] {
    let w = body.right - body.left;
    let top = body.top + 22.0;
    let h_avail = (body.bottom - top).max(40.0);
    let gap = 10.0;
    let (cols, rows) = if w >= 420.0 { (4, 1) } else { (2, 2) };
    let tile_w = (w - gap * (cols - 1) as f32) / cols as f32;
    let tile_h = ((h_avail - gap * (rows - 1) as f32) / rows as f32).min(96.0);
    let mut out = [D2D_RECT_F::default(); 4];
    for (i, tile) in out.iter_mut().enumerate() {
        let (c, r) = (i % cols, i / cols);
        let left = body.left + c as f32 * (tile_w + gap);
        let t = top + r as f32 * (tile_h + gap);
        *tile = D2D_RECT_F {
            left,
            top: t,
            right: left + tile_w,
            bottom: t + tile_h,
        };
    }
    out
}

/// "Clear All" button bounds in the Notifications slide header.
pub fn notification_clear_button(body: D2D_RECT_F) -> D2D_RECT_F {
    D2D_RECT_F {
        left: body.right - 68.0,
        top: body.top - 2.0,
        right: body.right,
        bottom: body.top + 18.0,
    }
}

const NOTIFICATION_LIST_TOP: f32 = 26.0;
const NOTIFICATION_ROW_GAP: f32 = 4.0;
const NOTIFICATION_ROW_MIN_HEIGHT: f32 = 22.0;
const NOTIFICATION_ROW_MAX_HEIGHT: f32 = 34.0;
pub const NOTIFICATION_MAX_VISIBLE_ROWS: usize = 4;

/// How many rows fit without crossing the slide body's lower edge.
pub fn notification_row_count(body: D2D_RECT_F) -> usize {
    let available = (body.bottom - body.top - NOTIFICATION_LIST_TOP).max(0.0);
    let fit = ((available + NOTIFICATION_ROW_GAP)
        / (NOTIFICATION_ROW_MIN_HEIGHT + NOTIFICATION_ROW_GAP))
        .floor() as usize;
    fit.min(NOTIFICATION_MAX_VISIBLE_ROWS)
}

/// Individual notification item rectangle in the responsive list view.
/// The painter and click handling share these bounds.
pub fn notification_item_rect(body: D2D_RECT_F, index: usize) -> D2D_RECT_F {
    let count = notification_row_count(body);
    let list_top = body.top + NOTIFICATION_LIST_TOP;
    if count == 0 || index >= count {
        return D2D_RECT_F {
            left: body.left,
            top: list_top,
            right: body.right,
            bottom: list_top,
        };
    }

    let available = (body.bottom - list_top).max(0.0);
    let item_h = ((available - NOTIFICATION_ROW_GAP * (count - 1) as f32) / count as f32)
        .clamp(NOTIFICATION_ROW_MIN_HEIGHT, NOTIFICATION_ROW_MAX_HEIGHT);
    let iy = list_top + (index as f32) * (item_h + NOTIFICATION_ROW_GAP);
    D2D_RECT_F {
        left: body.left,
        top: iy,
        right: body.right,
        bottom: iy + item_h,
    }
}

/// Back button bounds in the detailed notification view.
pub fn notification_back_button(body: D2D_RECT_F) -> D2D_RECT_F {
    D2D_RECT_F {
        left: body.left,
        top: body.top - 2.0,
        right: body.left + 58.0,
        bottom: body.top + 18.0,
    }
}

/// Dismiss button bounds in the detailed notification view.
pub fn notification_dismiss_button(body: D2D_RECT_F) -> D2D_RECT_F {
    D2D_RECT_F {
        left: body.right - 72.0,
        top: body.bottom - 22.0,
        right: body.right,
        bottom: body.bottom + 2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fused(width: f32, height: f32, flare: f32, radius_bottom: f32) -> NotchShape {
        NotchShape {
            left: 100.0,
            top: 0.0,
            right: 100.0 + width,
            bottom: height,
            radius_top: 0.0,
            radius_bottom,
            flare,
        }
    }

    #[test]
    fn fused_rim_runs_tip_to_tip_inside_its_bounds() {
        let shape = fused(188.0, 34.0, 16.0, 17.0);
        let points = shape.rim_points();

        assert_eq!(points.first(), Some(&(84.0, 0.0)));
        assert_eq!(points.last(), Some(&(304.0, 0.0)));
        for &(x, y) in &points {
            assert!((84.0..=304.0).contains(&x), "x {x} escapes the shoulders");
            assert!((0.0..=34.0).contains(&y), "y {y} escapes the slab");
        }
    }

    #[test]
    fn corners_never_overlap_on_a_short_slab() {
        // Shoulders and bottom corners both want more height than there is.
        let shape = fused(120.0, 20.0, 16.0, 20.0);
        let points = shape.rim_points();

        // The left wall only ever descends and the right only ever climbs.
        let bottom_at = points
            .iter()
            .position(|p| p.1 >= 20.0 - 1e-3)
            .expect("rim reaches the bottom");
        assert!(points[..=bottom_at]
            .windows(2)
            .all(|w| w[1].1 >= w[0].1 - 1e-3));
        let last_bottom = points.len()
            - 1
            - points
                .iter()
                .rev()
                .position(|p| p.1 >= 20.0 - 1e-3)
                .unwrap();
        assert!(points[last_bottom..]
            .windows(2)
            .all(|w| w[1].1 <= w[0].1 + 1e-3));
    }

    #[test]
    fn smoothed_corner_matches_the_depth_of_its_circle() {
        // Midpoint of a corner cubic, measured from the apex along each axis,
        // should sit where a circle of the same radius puts it: r(1 - 1/sqrt 2).
        let r = 20.0;
        let reach = r * CORNER_REACH;
        let Segment::Cubic(c1, c2, to) = corner((0.0, reach), (0.0, 0.0), (reach, 0.0)) else {
            panic!("expected a curve");
        };
        let mid = cubic_at((0.0, reach), c1, c2, to, 0.5);
        let circle = r * (1.0 - std::f32::consts::FRAC_1_SQRT_2);
        assert!((mid.0 - circle).abs() < 0.2, "{} vs {circle}", mid.0);
        assert!((mid.1 - circle).abs() < 0.2, "{} vs {circle}", mid.1);
    }

    #[test]
    fn detached_rim_starts_after_its_top_corner() {
        let shape = NotchShape {
            radius_top: 10.0,
            flare: 0.0,
            ..fused(200.0, 60.0, 0.0, 10.0)
        };
        assert!(!shape.fused());
        let points = shape.rim_points();
        assert_eq!(points.first(), Some(&(115.0, 0.0)));
        assert_eq!(points.last(), Some(&(285.0, 0.0)));
    }
}

#[cfg(test)]
mod notification_layout_tests {
    use super::{notification_item_rect, notification_row_count};
    use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

    #[test]
    fn four_notification_rows_fit_inside_the_default_panel_body() {
        let body = D2D_RECT_F {
            left: 28.0,
            top: 16.0,
            right: 732.0,
            bottom: 177.0,
        };

        assert_eq!(notification_row_count(body), 4);
        let rows = (0..4)
            .map(|index| notification_item_rect(body, index))
            .collect::<Vec<_>>();
        for row in &rows {
            assert!(row.top >= body.top + 26.0);
            assert!(row.bottom <= body.bottom);
            assert!(row.bottom > row.top);
        }
        for pair in rows.windows(2) {
            assert!(pair[0].bottom < pair[1].top);
        }
    }

    #[test]
    fn notification_rows_shrink_gracefully_for_short_panels() {
        let body = D2D_RECT_F {
            left: 12.0,
            top: 4.0,
            right: 500.0,
            bottom: 84.0,
        };

        assert_eq!(notification_row_count(body), 2);
        let last = notification_item_rect(body, 1);
        assert!(last.bottom <= body.bottom);
        assert_eq!(
            notification_item_rect(body, 2).top,
            notification_item_rect(body, 2).bottom
        );
    }
}
