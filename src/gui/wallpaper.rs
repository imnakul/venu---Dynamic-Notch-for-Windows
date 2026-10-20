//! The wallpaper framing preview.
//!
//! The notch crops its wallpaper cover-fit: the image is scaled to fill the
//! panel and the overflow is pushed around by a focal point. That is easy to
//! get wrong blind, so this draws the exact same crop at panel aspect ratio
//! and lets the focal point be dragged directly on it.
//!
//! The maths here deliberately mirrors `Painter::fill_with_image`; if one
//! changes the other has to follow, or the preview stops being a preview.

use eframe::egui::{self, Color32, Rect, Rounding, Sense, Stroke, TextureHandle, Vec2};

use crate::config::WallpaperConfig;

/// A decoded image kept alive as a GPU texture, plus the path it came from so
/// a changed path can invalidate it.
pub struct PreviewCache {
    path: String,
    texture: Option<TextureHandle>,
}

impl PreviewCache {
    pub fn new() -> Self {
        Self {
            path: String::new(),
            texture: None,
        }
    }

    /// Decode `path` if it is not already the cached one. Returns the texture,
    /// or `None` when there is no image or it could not be read.
    fn texture(&mut self, ctx: &egui::Context, path: &str) -> Option<&TextureHandle> {
        let path = path.trim();

        if path != self.path {
            self.path = path.to_string();
            self.texture = None;

            if !path.is_empty() {
                match image::open(path) {
                    Ok(img) => {
                        // Downscaled before upload: the preview is a few
                        // hundred pixels wide and a 6000px photo would cost
                        // both memory and a visible hitch to upload whole.
                        let img = img.to_rgba8();
                        let (w, h) = img.dimensions();
                        let max = 1024;
                        let img = if w > max || h > max {
                            image::imageops::resize(
                                &img,
                                w.min(max).max(1),
                                h.min(max).max(1),
                                image::imageops::FilterType::Triangle,
                            )
                        } else {
                            img
                        };

                        let size = [img.width() as usize, img.height() as usize];
                        let colour = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                        self.texture = Some(ctx.load_texture(
                            "wallpaper_preview",
                            colour,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                    Err(e) => eprintln!("[settings] wallpaper preview failed for {path}: {e}"),
                }
            }
        }

        self.texture.as_ref()
    }
}

impl Default for PreviewCache {
    fn default() -> Self {
        Self::new()
    }
}

/// The visible region of the source image, in 0..1 texture coordinates.
///
/// Cover-fit means one axis is fully visible and the other is cropped; `zoom`
/// crops both further. The focal point decides which part of the cropped axis
/// survives — 0 keeps the left/top edge, 1 the right/bottom.
fn crop_uv(src: Vec2, dst: Vec2, focus_x: f32, focus_y: f32, zoom: f32) -> Rect {
    if src.x <= 0.0 || src.y <= 0.0 || dst.x <= 0.0 || dst.y <= 0.0 {
        return Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    }

    let scale = (dst.x / src.x).max(dst.y / src.y) * zoom;
    let fw = (dst.x / (scale * src.x)).clamp(0.0, 1.0);
    let fh = (dst.y / (scale * src.y)).clamp(0.0, 1.0);

    let u0 = focus_x * (1.0 - fw);
    let v0 = focus_y * (1.0 - fh);

    Rect::from_min_size(egui::pos2(u0, v0), egui::vec2(fw, fh))
}

/// Draw the preview and handle dragging. Returns true if the config changed.
pub fn preview(
    ui: &mut egui::Ui,
    cache: &mut PreviewCache,
    wallpaper: &mut WallpaperConfig,
    panel_aspect: f32,
    empty_hint: &str,
) -> bool {
    let mut changed = false;

    // The preview is the panel, at panel aspect, as wide as the column allows.
    let avail_w = ui.available_width().max(80.0);
    let width = avail_w;
    let height = (width / panel_aspect.max(0.2)).clamp(80.0, 320.0);

    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, height), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let rounding = Rounding::same(10.0);

    painter.rect_filled(rect, rounding, Color32::from_rgb(18, 18, 23));

    let path = wallpaper.path.clone();
    let (focus_x, focus_y, zoom) = wallpaper.sanitised();

    let src = cache
        .texture(ui.ctx(), &path)
        .map(|t| (t.id(), Vec2::new(t.size()[0] as f32, t.size()[1] as f32)));

    if let Some((id, src_size)) = src {
        let uv = crop_uv(src_size, rect.size(), focus_x, focus_y, zoom);

        // A textured rounded rect rather than `painter.image`, so the crop
        // gets the panel's rounded corners instead of square ones.
        painter.add(egui::Shape::Rect(egui::epaint::RectShape {
            rect,
            rounding,
            fill: Color32::WHITE,
            stroke: Stroke::NONE,
            blur_width: 0.0,
            fill_texture_id: id,
            uv,
        }));

        if response.dragged() {
            let delta = response.drag_delta();

            // A drag moves the *image*, so the focal point moves the other way.
            // Converted through the visible fraction so a pixel of drag moves
            // the same pixel of image regardless of zoom.
            let pan_x = 1.0 - uv.width();
            let pan_y = 1.0 - uv.height();

            if pan_x > 0.0005 {
                let du = -delta.x / rect.width() * uv.width();
                wallpaper.focus_x = (focus_x + du / pan_x).clamp(0.0, 1.0);
                changed = true;
            }
            if pan_y > 0.0005 {
                let dv = -delta.y / rect.height() * uv.height();
                wallpaper.focus_y = (focus_y + dv / pan_y).clamp(0.0, 1.0);
                changed = true;
            }
        }

        // Rule-of-thirds guides, only while dragging, so the preview stays
        // clean at rest but framing has something to snap the eye to.
        if response.dragged() || response.hovered() {
            let guide = Stroke::new(1.0, Color32::from_white_alpha(38));
            for i in 1..3 {
                let f = i as f32 / 3.0;
                let x = rect.left() + rect.width() * f;
                let y = rect.top() + rect.height() * f;
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    guide,
                );
                painter.line_segment(
                    [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                    guide,
                );
            }
        }
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            empty_hint,
            egui::FontId::proportional(12.0),
            Color32::from_rgb(110, 110, 120),
        );
    }

    painter.rect_stroke(
        rect,
        rounding,
        Stroke::new(1.0, Color32::from_rgb(48, 48, 56)),
    );

    if response.hovered() && src.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }

    changed
}
