//! The painter for a FlashScreen frame.
//!
//! Everything — the text, the image, the background behind them — is laid out
//! as one block centred at the origin and then placed with a single Direct2D
//! transform, so every animation style is the same draw with a different
//! matrix and opacity: a slide is a translation, a zoom is a scale, a fade is
//! only an alpha.
//!
//! Backgrounds that need the desktop blurred reuse the notch's capture trick:
//! sample the screen at a fraction of the size and let bilinear scaling do
//! the blurring on the way back up.

use std::collections::HashMap;

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{GENERIC_READ, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT,
    D2D_POINT_2F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1BitmapBrush, ID2D1Brush, ID2D1DCRenderTarget,
    ID2D1Factory, ID2D1SolidColorBrush, D2D1_BITMAP_BRUSH_PROPERTIES,
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
    D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_GAMMA_2_2, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_METRICS,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeMedianCut, WICDecodeMetadataCacheOnLoad,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

use crate::config::{FlashAnim, FlashBackground, FlashConfig, FlashContent, FlashLayout};
use crate::notch::backdrop::BackdropCache;

/// How round the background's corners are. One place, so text and image can
/// never disagree about the shape they share.
const BG_RADIUS: f32 = 28.0;

/// Gap between the text and the image when a flash carries both.
const BLOCK_GAP: f32 = 32.0;

/// A built text layout: the DirectWrite layout plus the two sizes that matter
/// — the layout *box* (what is drawn, centred) and the measured *content*
/// (what the glyphs actually occupy, used to size the block).
struct TextPart {
    layout: IDWriteTextLayout,
    box_w: f32,
    box_h: f32,
    w: f32,
    h: f32,
}

/// A decoded image at the size this flash will draw it.
struct ImagePart {
    bitmap: ID2D1Bitmap,
    w: f32,
    h: f32,
}

/// Which leg of the flash a frame belongs to, and how far into it. `In` and
/// `Out` carry raw 0..1 progress; the easing is applied per style.
enum Phase {
    In(f32),
    Hold,
    Out(f32),
}

fn phase_of(elapsed: f32, cfg: &FlashConfig) -> Phase {
    let hold = cfg.safe_duration();
    let anim = cfg.safe_anim_secs();
    if elapsed >= hold {
        Phase::Out(((elapsed - hold) / anim).min(1.0))
    } else if elapsed < anim {
        Phase::In((elapsed / anim).min(1.0))
    } else {
        Phase::Hold
    }
}

/// Where this frame's content sits: how opaque, how far from centre, at what
/// size. One struct so every style stays comparable at a glance.
struct DrawParams {
    alpha: f32,
    offset_x: f32,
    scale: f32,
}

fn ease_out(p: f32) -> f32 {
    1.0 - (1.0 - p).powi(3)
}

fn ease_in(p: f32) -> f32 {
    p.powi(3)
}

fn draw_params(anim: FlashAnim, phase: Phase, screen_w: f32, block_w: f32) -> DrawParams {
    let full = DrawParams {
        alpha: 1.0,
        offset_x: 0.0,
        scale: 1.0,
    };

    match anim {
        // The block travels one way the whole time: in from one edge, brief
        // stillness in the centre, out through the other edge. The entry
        // decelerates into place and the exit accelerates away, which is what
        // makes a pass read as deliberate rather than a glitch.
        FlashAnim::SlideLeftToRight => {
            let travel = screen_w * 0.5 + block_w * 0.5;
            match phase {
                Phase::In(p) => DrawParams {
                    offset_x: -(1.0 - ease_out(p)) * travel,
                    ..full
                },
                Phase::Hold => full,
                Phase::Out(p) => DrawParams {
                    offset_x: ease_in(p) * travel,
                    ..full
                },
            }
        }
        FlashAnim::SlideRightToLeft => {
            let travel = screen_w * 0.5 + block_w * 0.5;
            match phase {
                Phase::In(p) => DrawParams {
                    offset_x: (1.0 - ease_out(p)) * travel,
                    ..full
                },
                Phase::Hold => full,
                Phase::Out(p) => DrawParams {
                    offset_x: -ease_in(p) * travel,
                    ..full
                },
            }
        }
        // Reveals by growing out of the centre, leaves by shrinking back into
        // it while fading — the flash equivalent of a breath.
        FlashAnim::ZoomCenter => match phase {
            Phase::In(p) => {
                let e = ease_out(p);
                DrawParams {
                    alpha: e,
                    scale: 0.55 + 0.45 * e,
                    ..full
                }
            }
            Phase::Hold => full,
            Phase::Out(p) => DrawParams {
                alpha: 1.0 - p,
                scale: 1.0 - 0.3 * ease_in(p),
                ..full
            },
        },
        FlashAnim::Fade => match phase {
            Phase::In(p) => DrawParams {
                alpha: ease_out(p),
                ..full
            },
            Phase::Hold => full,
            Phase::Out(p) => DrawParams {
                alpha: 1.0 - p,
                ..full
            },
        },
    }
}

fn scale_matrix(s: f32) -> Matrix3x2 {
    Matrix3x2 {
        M11: s,
        M12: 0.0,
        M21: 0.0,
        M22: s,
        M31: 0.0,
        M32: 0.0,
    }
}

fn scale_xy(sx: f32, sy: f32) -> Matrix3x2 {
    Matrix3x2 {
        M11: sx,
        M12: 0.0,
        M21: 0.0,
        M22: sy,
        M31: 0.0,
        M32: 0.0,
    }
}

pub struct FlashRenderer {
    d2d_factory: ID2D1Factory,
    dwrite_factory: IDWriteFactory,
    font_collection: Option<IDWriteFontCollection>,
    wic: Option<IWICImagingFactory>,
    dc_target: Option<ID2D1DCRenderTarget>,
    mem_dc: HDC,
    hbitmap: HBITMAP,
    old_hbitmap: HBITMAP,
    bits: *mut std::ffi::c_void,
    width: u32,
    height: u32,
    /// Decoded images by path. With several images in rotation the whole set
    /// stays decoded across flashes. Tied to the render target: if the buffer
    /// is ever rebuilt the cache is dropped with it, since a bitmap from one
    /// Direct2D target cannot be drawn on another.
    images: HashMap<String, ID2D1Bitmap>,
    /// Paths that failed to decode, remembered so they are not retried at
    /// sixty frames a second for the whole length of a flash.
    failed_images: std::collections::HashSet<String>,
    /// The desktop capture behind the block, for Frosted and Blur.
    backdrop: BackdropCache,
}

unsafe impl Send for FlashRenderer {}
unsafe impl Sync for FlashRenderer {}

impl FlashRenderer {
    pub fn new() -> windows::core::Result<Self> {
        let d2d_factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let dwrite_factory: IDWriteFactory =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let font_collection = crate::fonts::build_private_collection(&dwrite_factory);
        if font_collection.is_none() {
            eprintln!("[flash] bundled fonts unavailable, using system fonts");
        }
        let wic: Option<IWICImagingFactory> =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok() };

        Ok(Self {
            d2d_factory,
            dwrite_factory,
            font_collection,
            wic,
            dc_target: None,
            mem_dc: HDC::default(),
            hbitmap: HBITMAP::default(),
            old_hbitmap: HBITMAP::default(),
            bits: std::ptr::null_mut(),
            width: 0,
            height: 0,
            images: HashMap::new(),
            failed_images: std::collections::HashSet::new(),
            backdrop: BackdropCache::default(),
        })
    }

    fn ensure_buffer(&mut self, width: u32, height: u32) -> windows::core::Result<()> {
        if self.width == width
            && self.height == height
            && !self.mem_dc.is_invalid()
            && self.dc_target.is_some()
        {
            return Ok(());
        }

        self.cleanup_buffer();

        if width == 0 || height == 0 {
            return Ok(());
        }

        unsafe {
            let mem_dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32), // Top-down DIB
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };

            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let hbitmap = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;

            let old_hbitmap = SelectObject(mem_dc, hbitmap);

            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };

            let dc_target: ID2D1DCRenderTarget = self.d2d_factory.CreateDCRenderTarget(&props)?;
            let rect = RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            };
            dc_target.BindDC(mem_dc, &rect)?;

            self.mem_dc = mem_dc;
            self.hbitmap = hbitmap;
            self.old_hbitmap = HBITMAP(old_hbitmap.0);
            self.bits = bits;
            self.width = width;
            self.height = height;
            self.dc_target = Some(dc_target);
        }

        Ok(())
    }

    fn cleanup_buffer(&mut self) {
        unsafe {
            self.dc_target = None;
            // Device resources die with the target they were created on.
            self.images.clear();
            self.failed_images.clear();
            if !self.mem_dc.is_invalid() {
                if !self.old_hbitmap.is_invalid() {
                    SelectObject(self.mem_dc, self.old_hbitmap);
                }
                if !self.hbitmap.is_invalid() {
                    let _ = DeleteObject(self.hbitmap);
                }
                let _ = DeleteDC(self.mem_dc);
            }
            self.mem_dc = HDC::default();
            self.hbitmap = HBITMAP::default();
            self.old_hbitmap = HBITMAP::default();
            self.bits = std::ptr::null_mut();
            self.width = 0;
            self.height = 0;
        }
    }

    pub fn render_frame(
        &mut self,
        width: u32,
        height: u32,
        origin_x: i32,
        origin_y: i32,
        cfg: &FlashConfig,
        elapsed: f32,
        turn: u32,
    ) -> windows::core::Result<Option<HDC>> {
        if width == 0 || height == 0 {
            return Ok(None);
        }

        self.ensure_buffer(width, height)?;

        // Cloned rather than borrowed: the image cache below needs &mut self
        // in the same scope, and a COM pointer clone is only an AddRef.
        let Some(target) = self.dc_target.clone() else {
            return Ok(None);
        };

        let phase = phase_of(elapsed, cfg);

        unsafe {
            target.BeginDraw();
            // Fully transparent: between the frames of an animation the flash
            // is simply not there, and the layered window shows the desktop.
            target.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));

            let _ = self.paint_block(
                &target, cfg, phase, elapsed, width, height, origin_x, origin_y, turn,
            );

            target.EndDraw(None, None)?;
        }

        Ok(Some(self.mem_dc))
    }

    /// A fully transparent frame, pushed while the window is hidden so a
    /// later `SW_SHOWNOACTIVATE` can never resurrect the previous flash for
    /// the split second before the new first frame lands.
    pub fn render_blank(&mut self, width: u32, height: u32) -> windows::core::Result<Option<HDC>> {
        if width == 0 || height == 0 {
            return Ok(None);
        }

        self.ensure_buffer(width, height)?;
        let Some(target) = self.dc_target.clone() else {
            return Ok(None);
        };

        unsafe {
            target.BeginDraw();
            target.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            target.EndDraw(None, None)?;
        }

        Ok(Some(self.mem_dc))
    }

    /// Lay out this turn's content as one block, draw its background, then
    /// draw the parts inside — all in the transformed space so the whole
    /// block animates as a single thing.
    fn paint_block(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &FlashConfig,
        phase: Phase,
        elapsed: f32,
        w: u32,
        h: u32,
        origin_x: i32,
        origin_y: i32,
        turn: u32,
    ) -> Option<()> {
        let show_text = matches!(cfg.content, FlashContent::Text | FlashContent::Both);
        let show_image = matches!(cfg.content, FlashContent::Image | FlashContent::Both);

        let text = if show_text {
            self.build_text_layout(cfg, cfg.text_for_turn(turn), w, h)
        } else {
            None
        };
        let image = if show_image {
            self.build_image_part(t, cfg, turn, w, h)
        } else {
            None
        };

        // --- the block: both parts' footprint, and each part's centre in it.
        let (block_w, block_h) = match (&text, &image) {
            (Some(tp), Some(ip)) => match cfg.layout {
                FlashLayout::TextTop | FlashLayout::TextBottom => {
                    (tp.w.max(ip.w), tp.h + BLOCK_GAP + ip.h)
                }
                FlashLayout::TextLeft | FlashLayout::TextRight => {
                    (tp.w + BLOCK_GAP + ip.w, tp.h.max(ip.h))
                }
            },
            (Some(tp), None) => (tp.w, tp.h),
            (None, Some(ip)) => (ip.w, ip.h),
            (None, None) => return None,
        };

        let mut text_c = (0.0f32, 0.0f32);
        let mut image_c = (0.0f32, 0.0f32);
        if let (Some(tp), Some(ip)) = (&text, &image) {
            match cfg.layout {
                FlashLayout::TextTop => {
                    text_c = (0.0, -block_h * 0.5 + tp.h * 0.5);
                    image_c = (0.0, block_h * 0.5 - ip.h * 0.5);
                }
                FlashLayout::TextBottom => {
                    image_c = (0.0, -block_h * 0.5 + ip.h * 0.5);
                    text_c = (0.0, block_h * 0.5 - tp.h * 0.5);
                }
                FlashLayout::TextLeft => {
                    text_c = (-block_w * 0.5 + tp.w * 0.5, 0.0);
                    image_c = (block_w * 0.5 - ip.w * 0.5, 0.0);
                }
                FlashLayout::TextRight => {
                    image_c = (-block_w * 0.5 + ip.w * 0.5, 0.0);
                    text_c = (block_w * 0.5 - tp.w * 0.5, 0.0);
                }
            }
        }

        // The background travels with the block, so the slide distance has to
        // cover the padded block, not the bare content.
        let pad = cfg.bg_padding.clamp(0.0, 200.0);
        let p = draw_params(cfg.anim, phase, w as f32, block_w + pad * 2.0);

        unsafe {
            // Scale about the origin, then move to the screen centre — `A * B`
            // means "apply A, then B", so the other order would scale the
            // centre offset too.
            t.SetTransform(
                &(scale_matrix(p.scale)
                    * Matrix3x2::translation(w as f32 * 0.5 + p.offset_x, h as f32 * 0.5)),
            );

            let bg_rect = D2D_RECT_F {
                left: -block_w * 0.5 - pad,
                top: -block_h * 0.5 - pad,
                right: block_w * 0.5 + pad,
                bottom: block_h * 0.5 + pad,
            };
            let _ = self.paint_background(t, cfg, &bg_rect, p.alpha, w, h, origin_x, origin_y);

            if let Some(ip) = &image {
                let dest = D2D_RECT_F {
                    left: image_c.0 - ip.w * 0.5,
                    top: image_c.1 - ip.h * 0.5,
                    right: image_c.0 + ip.w * 0.5,
                    bottom: image_c.1 + ip.h * 0.5,
                };
                t.DrawBitmap(
                    &ip.bitmap,
                    Some(&dest),
                    p.alpha.clamp(0.0, 1.0),
                    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                    None,
                );
            }

            if let Some(tp) = &text {
                let brush = self.text_brush(t, cfg, turn, text_c, tp, p.alpha)?;
                t.DrawTextLayout(
                    D2D_POINT_2F {
                        x: text_c.0 - tp.box_w * 0.5,
                        y: text_c.1 - tp.box_h * 0.5,
                    },
                    &tp.layout,
                    &brush,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
                if cfg.shine {
                    let _ = self.paint_shine(t, cfg, elapsed, text_c, tp, p.alpha);
                }
            }

            t.SetTransform(&scale_matrix(1.0));
        }

        Some(())
    }

    fn build_text_layout(&self, cfg: &FlashConfig, text: &str, w: u32, h: u32) -> Option<TextPart> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }

        unsafe {
            let weight = if cfg.font.bold {
                DWRITE_FONT_WEIGHT_BOLD
            } else {
                DWRITE_FONT_WEIGHT_NORMAL
            };
            let style = if cfg.font.italic {
                DWRITE_FONT_STYLE_ITALIC
            } else {
                DWRITE_FONT_STYLE_NORMAL
            };

            let family = crate::fonts::family_for_collection(
                &cfg.font.family,
                self.font_collection.is_some(),
            );
            let family_hstr = HSTRING::from(family);
            let collection = if crate::fonts::is_bundled_family(&cfg.font.family) {
                self.font_collection.as_ref()
            } else {
                None
            };
            let text_format: IDWriteTextFormat = self
                .dwrite_factory
                .CreateTextFormat(
                    PCWSTR(family_hstr.as_ptr()),
                    collection,
                    weight,
                    style,
                    DWRITE_FONT_STRETCH_NORMAL,
                    cfg.font.size.clamp(8.0, 512.0),
                    PCWSTR(HSTRING::from("en-us").as_ptr()),
                )
                .ok()?;

            // A flash can be a word or a sentence: wrap inside most of the
            // screen, centred on both axes, and the animation moves the whole
            // block as one thing.
            let box_w = w as f32 * 0.92;
            let box_h = h as f32 * 0.92;
            let text_u16: Vec<u16> = text.encode_utf16().collect();
            let layout: IDWriteTextLayout = self
                .dwrite_factory
                .CreateTextLayout(&text_u16, &text_format, box_w, box_h)
                .ok()?;
            let _ = layout.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);

            let mut metrics = std::mem::zeroed::<DWRITE_TEXT_METRICS>();
            let _ = layout.GetMetrics(&mut metrics);

            Some(TextPart {
                layout,
                box_w,
                box_h,
                w: metrics
                    .widthIncludingTrailingWhitespace
                    .max(metrics.width)
                    .max(1.0),
                h: metrics.height.max(1.0),
            })
        }
    }

    fn build_image_part(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &FlashConfig,
        turn: u32,
        w: u32,
        h: u32,
    ) -> Option<ImagePart> {
        let path = cfg.image_for_turn(turn);
        if path.is_empty() {
            return None;
        }
        let bitmap = self.ensure_image(t, path)?;

        let size = unsafe { bitmap.GetSize() };
        if size.width <= 0.0 || size.height <= 0.0 {
            return None;
        }

        // Contain-fit: the requested size is a share of the screen height,
        // clamped down if the image is wider than the screen.
        let max_h = h as f32 * cfg.image_scale.clamp(0.1, 1.0);
        let max_w = w as f32 * 0.9;
        let scale = (max_h / size.height).min(max_w / size.width);

        Some(ImagePart {
            bitmap,
            w: size.width * scale,
            h: size.height * scale,
        })
    }

    /// The brush the text draws with: one solid colour rotating per flash, or
    /// — with Gradient on — a sweep across every colour, rotated so each
    /// flash's sweep starts on the next colour in the list.
    fn text_brush(
        &self,
        t: &ID2D1DCRenderTarget,
        cfg: &FlashConfig,
        turn: u32,
        text_c: (f32, f32),
        tp: &TextPart,
        anim_alpha: f32,
    ) -> Option<ID2D1Brush> {
        if cfg.gradient_text && cfg.text_colors.len() >= 2 {
            let n = cfg.text_colors.len();
            let mut stops: Vec<D2D1_GRADIENT_STOP> = Vec::with_capacity(n);
            for i in 0..n {
                let raw = cfg.text_colors[(turn as usize + i) % n];
                let a = (raw[3] * anim_alpha).clamp(0.0, 1.0);
                stops.push(D2D1_GRADIENT_STOP {
                    position: i as f32 / (n - 1) as f32,
                    color: D2D1_COLOR_F {
                        r: raw[0] * a,
                        g: raw[1] * a,
                        b: raw[2] * a,
                        a,
                    },
                });
            }

            unsafe {
                let collection = t
                    .CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
                    .ok()?;
                let gradient_props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: D2D_POINT_2F {
                        x: text_c.0 - tp.w * 0.5,
                        y: text_c.1,
                    },
                    endPoint: D2D_POINT_2F {
                        x: text_c.0 + tp.w * 0.5,
                        y: text_c.1,
                    },
                };
                let brush = t
                    .CreateLinearGradientBrush(&gradient_props, None, &collection)
                    .ok()?;
                Some(brush.cast().ok()?)
            }
        } else {
            let raw = cfg.color_for_turn(turn);
            let a = (raw[3] * anim_alpha).clamp(0.0, 1.0);
            let brush: ID2D1SolidColorBrush = unsafe {
                t.CreateSolidColorBrush(
                    &D2D1_COLOR_F {
                        r: raw[0] * a,
                        g: raw[1] * a,
                        b: raw[2] * a,
                        a,
                    },
                    None,
                )
                .ok()?
            };
            Some(brush.cast().ok()?)
        }
    }

    /// The shine: the same glyphs drawn a second time with a narrow diagonal
    /// band of white, sweeping left to right across the text. Outside the band
    /// the gradient clamps to fully transparent, so only the passing glare is
    /// added to whatever the base brush already drew.
    ///
    /// The wave waits for the text to finish appearing, then crosses once at
    /// a steady, reading-like pace — like eyes tracking the line — and is then
    /// gone. Starting it during the entry animation would tangle the two
    /// movements; repeating it would read as a loading spinner.
    fn paint_shine(
        &self,
        t: &ID2D1DCRenderTarget,
        cfg: &FlashConfig,
        elapsed: f32,
        text_c: (f32, f32),
        tp: &TextPart,
        anim_alpha: f32,
    ) -> Option<()> {
        let u = ((elapsed - cfg.safe_anim_secs()) / cfg.safe_shine_secs()).clamp(0.0, 1.0);

        // The band starts fully off the left edge and leaves fully off the
        // right, so the glare grows in and out rather than popping. Narrow
        // enough to read as a wave, not a flood — a glare covering half the
        // text is a flash within the flash.
        let band_w = (tp.w * 0.25).clamp(40.0, 300.0);
        let left = text_c.0 - tp.w * 0.5;
        let centre_x = left - band_w + u * (tp.w + band_w * 2.0);
        // Tilted, because a straight vertical glare reads as a scanline.
        let tilt = tp.h * 0.4;

        let a = 0.9 * anim_alpha.clamp(0.0, 1.0);
        let stops = [
            D2D1_GRADIENT_STOP {
                position: 0.0,
                color: D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                },
            },
            D2D1_GRADIENT_STOP {
                position: 0.5,
                color: D2D1_COLOR_F {
                    r: a,
                    g: a,
                    b: a,
                    a,
                },
            },
            D2D1_GRADIENT_STOP {
                position: 1.0,
                color: D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                },
            },
        ];

        unsafe {
            let collection = t
                .CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
                .ok()?;
            let gradient_props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: D2D_POINT_2F {
                    x: centre_x - band_w * 0.5,
                    y: text_c.1 - tilt,
                },
                endPoint: D2D_POINT_2F {
                    x: centre_x + band_w * 0.5,
                    y: text_c.1 + tilt,
                },
            };
            let brush = t
                .CreateLinearGradientBrush(&gradient_props, None, &collection)
                .ok()?;
            t.DrawTextLayout(
                D2D_POINT_2F {
                    x: text_c.0 - tp.box_w * 0.5,
                    y: text_c.1 - tp.box_h * 0.5,
                },
                &tp.layout,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
        }

        Some(())
    }

    fn ensure_image(&mut self, t: &ID2D1DCRenderTarget, path: &str) -> Option<ID2D1Bitmap> {
        if let Some(bitmap) = self.images.get(path) {
            return Some(bitmap.clone());
        }
        if self.failed_images.contains(path) {
            return None;
        }

        let bitmap = self.decode_image(t, path);
        match bitmap {
            Some(b) => {
                self.failed_images.remove(path);
                self.images.insert(path.to_string(), b.clone());
                Some(b)
            }
            None => {
                self.failed_images.insert(path.to_string());
                None
            }
        }
    }

    /// WIC decode, the same path the notch wallpaper takes. The flash module
    /// keeps its own copy rather than sharing the notch painter's because the
    /// cache is bound to this target's lifetime, not the notch's.
    fn decode_image(&self, t: &ID2D1DCRenderTarget, path: &str) -> Option<ID2D1Bitmap> {
        let wic = self.wic.as_ref()?;
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

        unsafe {
            let decoder = wic
                .CreateDecoderFromFilename(
                    PCWSTR(wide.as_ptr()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnLoad,
                )
                .ok()?;
            let frame = decoder.GetFrame(0).ok()?;
            let converter = wic.CreateFormatConverter().ok()?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeMedianCut,
                )
                .ok()?;

            t.CreateBitmapFromWicBitmap(&converter, None).ok()
        }
    }

    /// The background behind the content block — never the whole screen. At
    /// zero strength nothing is drawn at all, which is also how "off" reads.
    fn paint_background(
        &mut self,
        t: &ID2D1DCRenderTarget,
        cfg: &FlashConfig,
        rect: &D2D_RECT_F,
        anim_alpha: f32,
        w: u32,
        h: u32,
        origin_x: i32,
        origin_y: i32,
    ) -> Option<()> {
        let strength = cfg.bg_strength.clamp(0.0, 1.0);
        if strength <= 0.02 || cfg.bg_kind == FlashBackground::Transparent {
            return None;
        }

        let rounded = D2D1_ROUNDED_RECT {
            rect: *rect,
            radiusX: BG_RADIUS,
            radiusY: BG_RADIUS,
        };

        match cfg.bg_kind {
            FlashBackground::Transparent => None,
            FlashBackground::White => {
                self.fill_rounded(t, &rounded, [1.0, 1.0, 1.0, 1.0], strength * anim_alpha)
            }
            FlashBackground::Dark => {
                self.fill_rounded(t, &rounded, [0.04, 0.05, 0.08, 1.0], strength * anim_alpha)
            }
            FlashBackground::Frosted => {
                // Blur first, then the light wash that makes it read as milk
                // glass rather than a magnifier.
                let _ = self.fill_blurred(
                    t, rect, &rounded, strength, anim_alpha, w, h, origin_x, origin_y,
                );
                self.fill_rounded(
                    t,
                    &rounded,
                    [1.0, 1.0, 1.0, 1.0],
                    strength * 0.45 * anim_alpha,
                )
            }
            FlashBackground::Blur => self.fill_blurred(
                t, rect, &rounded, strength, anim_alpha, w, h, origin_x, origin_y,
            ),
        }
    }

    fn fill_rounded(
        &self,
        t: &ID2D1DCRenderTarget,
        rounded: &D2D1_ROUNDED_RECT,
        color: [f32; 4],
        alpha: f32,
    ) -> Option<()> {
        if alpha <= 0.01 {
            return None;
        }
        // Premultiplied, matching the target's alpha mode and the rest of
        // this codebase's brushes.
        let a = color[3].min(1.0) * alpha.clamp(0.0, 1.0);
        let brush: ID2D1SolidColorBrush = unsafe {
            t.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: color[0] * a,
                    g: color[1] * a,
                    b: color[2] * a,
                    a,
                },
                None,
            )
            .ok()?
        };
        unsafe { t.FillRoundedRectangle(rounded, &brush) };
        Some(())
    }

    /// Sample the desktop behind where the block rests, scaled down and
    /// blurred by being drawn back up, and clip it to the rounded rect. The
    /// downscale factor is the blur: bigger sample steps, blurrier result.
    fn fill_blurred(
        &mut self,
        t: &ID2D1DCRenderTarget,
        rect: &D2D_RECT_F,
        rounded: &D2D1_ROUNDED_RECT,
        strength: f32,
        anim_alpha: f32,
        w: u32,
        h: u32,
        origin_x: i32,
        origin_y: i32,
    ) -> Option<()> {
        let rect_w = rect.right - rect.left;
        let rect_h = rect.bottom - rect.top;
        if rect_w <= 1.0 || rect_h <= 1.0 {
            return None;
        }

        // Where the block rests on screen, ignoring the animation offset: the
        // capture is of the desktop, and the desktop is not moving.
        let sx = origin_x + (w as f32 * 0.5 - rect_w * 0.5) as i32;
        let sy = origin_y + (h as f32 * 0.5 - rect_h * 0.5) as i32;

        let downscale = if strength < 0.25 {
            3
        } else if strength < 0.6 {
            8
        } else {
            16
        };

        let (bitmap, _, _) =
            self.backdrop
                .sample(t, sx, sy, rect_w as i32, rect_h as i32, downscale)?;
        let size = unsafe { bitmap.GetSize() };
        if size.width <= 0.0 || size.height <= 0.0 {
            return None;
        }

        let props = D2D1_BITMAP_BRUSH_PROPERTIES {
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
        };
        let brush: ID2D1BitmapBrush =
            unsafe { t.CreateBitmapBrush(&bitmap, Some(&props), None).ok()? };

        unsafe {
            // Scale the capture to the rect *then* translate it there. The
            // bitmap is tiny and the scale factors are large; scaling the
            // translation too would throw it far off the rect.
            brush.SetTransform(
                &(scale_xy(rect_w / size.width, rect_h / size.height)
                    * Matrix3x2::translation(rect.left, rect.top)),
            );
            // The capture is opaque; this is what lets it fade in and out
            // with the rest of the flash instead of snapping in.
            brush.SetOpacity(anim_alpha.clamp(0.0, 1.0));
            t.FillRoundedRectangle(rounded, &brush);
        }

        Some(())
    }
}

impl Drop for FlashRenderer {
    fn drop(&mut self) {
        self.cleanup_buffer();
    }
}
