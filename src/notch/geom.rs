//! The notch silhouette.
//!
//! One path builder covers both resting states. While the notch is fused to
//! the top bezel it grows *outward* at the very top through a pair of concave
//! shoulders, so the dark slab dissolves into the black bezel instead of
//! ending in two hard corners. As it detaches or expands, those shoulders
//! shrink to nothing and the top corners round off into a floating panel.

use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_F,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Factory, ID2D1PathGeometry, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_SMALL,
    D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
};

/// Anything below this is treated as a hard corner; feeding Direct2D a
/// degenerate arc produces artefacts on some drivers.
const MIN_RADIUS: f32 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotchShape {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub radius_top: f32,
    pub radius_bottom: f32,
    /// Concave shoulder radius. Zero for a plain rounded rectangle.
    pub flare: f32,
}

impl NotchShape {
    pub fn width(&self) -> f32 {
        (self.right - self.left).max(0.0)
    }

    pub fn height(&self) -> f32 {
        (self.bottom - self.top).max(0.0)
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
    /// Deliberately ignores the flare: the shoulders only *add* area at the
    /// very top, where the cursor is already inside the main body, so treating
    /// the shape as a rounded rectangle is both correct enough and branch-free.
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

    /// Build the filled path. Traversed clockwise in Direct2D's y-down space.
    pub fn build(&self, factory: &ID2D1Factory) -> windows::core::Result<ID2D1PathGeometry> {
        let w = self.width();
        let h = self.height();

        let (l, t, r, b) = (self.left, self.top, self.right, self.bottom);

        // Radii can never exceed half the shorter side, or the arcs overlap.
        let limit = (w.min(h) * 0.5).max(0.0);
        let rt = self.radius_top.clamp(0.0, limit);
        let rb = self.radius_bottom.clamp(0.0, limit);
        let flare = self.flare.clamp(0.0, (h * 0.5).max(0.0));
        let flared = flare > MIN_RADIUS;

        let geometry = unsafe { factory.CreatePathGeometry()? };
        let sink = unsafe { geometry.Open()? };

        unsafe {
            let start = if flared {
                D2D_POINT_2F { x: l - flare, y: t }
            } else {
                D2D_POINT_2F { x: l + rt, y: t }
            };
            sink.BeginFigure(start, D2D1_FIGURE_BEGIN_FILLED);

            // --- top edge, left to right -----------------------------------
            if flared {
                sink.AddLine(D2D_POINT_2F { x: r + flare, y: t });
                // Concave right shoulder: curves back inward as it descends.
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: D2D_POINT_2F { x: r, y: t + flare },
                    size: D2D_SIZE_F {
                        width: flare,
                        height: flare,
                    },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
            } else {
                sink.AddLine(D2D_POINT_2F { x: r - rt, y: t });
                if rt > MIN_RADIUS {
                    sink.AddArc(&D2D1_ARC_SEGMENT {
                        point: D2D_POINT_2F { x: r, y: t + rt },
                        size: D2D_SIZE_F {
                            width: rt,
                            height: rt,
                        },
                        rotationAngle: 0.0,
                        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                        arcSize: D2D1_ARC_SIZE_SMALL,
                    });
                } else {
                    sink.AddLine(D2D_POINT_2F { x: r, y: t });
                }
            }

            // --- right edge, bottom-right corner ---------------------------
            sink.AddLine(D2D_POINT_2F { x: r, y: b - rb });
            if rb > MIN_RADIUS {
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: D2D_POINT_2F { x: r - rb, y: b },
                    size: D2D_SIZE_F {
                        width: rb,
                        height: rb,
                    },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
            } else {
                sink.AddLine(D2D_POINT_2F { x: r, y: b });
            }

            // --- bottom edge, bottom-left corner ---------------------------
            sink.AddLine(D2D_POINT_2F { x: l + rb, y: b });
            if rb > MIN_RADIUS {
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: D2D_POINT_2F { x: l, y: b - rb },
                    size: D2D_SIZE_F {
                        width: rb,
                        height: rb,
                    },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
            } else {
                sink.AddLine(D2D_POINT_2F { x: l, y: b });
            }

            // --- left edge, back to the start ------------------------------
            if flared {
                sink.AddLine(D2D_POINT_2F { x: l, y: t + flare });
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: D2D_POINT_2F { x: l - flare, y: t },
                    size: D2D_SIZE_F {
                        width: flare,
                        height: flare,
                    },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
            } else {
                sink.AddLine(D2D_POINT_2F { x: l, y: t + rt });
                if rt > MIN_RADIUS {
                    sink.AddArc(&D2D1_ARC_SEGMENT {
                        point: D2D_POINT_2F { x: l + rt, y: t },
                        size: D2D_SIZE_F {
                            width: rt,
                            height: rt,
                        },
                        rotationAngle: 0.0,
                        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                        arcSize: D2D1_ARC_SIZE_SMALL,
                    });
                } else {
                    sink.AddLine(D2D_POINT_2F { x: l, y: t });
                }
            }

            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
        }

        Ok(geometry)
    }
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

/// Centres and radius of the Now Playing slide's three transport buttons
/// (previous / play-pause / next), centred along the body's bottom edge.
/// Shared by the painter and the window for the same draw/hit-test reason as
/// [`NotchShape::pin_button`].
pub fn media_transport_buttons(body: D2D_RECT_F) -> [(f32, f32, f32); 3] {
    let r = 15.0;
    let gap = 34.0;
    let cx = (body.left + body.right) * 0.5;
    let cy = body.bottom - r - 4.0;
    [
        (cx - gap, cy, r * 0.72),
        (cx, cy, r),
        (cx + gap, cy, r * 0.72),
    ]
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

/// Individual notification item rectangle in the list view.
pub fn notification_item_rect(body: D2D_RECT_F, index: usize) -> D2D_RECT_F {
    let list_top = body.top + 26.0;
    let item_h = 38.0;
    let iy = list_top + (index as f32) * (item_h + 6.0);
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
