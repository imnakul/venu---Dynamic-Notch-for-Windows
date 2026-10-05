use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush,
    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
};

use crate::config::{AppConfig, FontConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Text shaping depends on content and layout, never on scroll position.
struct CachedText {
    text: String,
    font: FontConfig,
    phrase_spacing: u32,
    width: u32,
    height: u32,
    edge: Edge,
    layout: IDWriteTextLayout,
    single_width: f32,
}

pub struct D2DRenderer {
    d2d_factory: ID2D1Factory,
    dwrite_factory: IDWriteFactory,
    font_collection: Option<IDWriteFontCollection>,
    dc_target: Option<ID2D1DCRenderTarget>,
    mem_dc: HDC,
    hbitmap: HBITMAP,
    old_hbitmap: HBITMAP,
    bits: *mut std::ffi::c_void,
    width: u32,
    height: u32,
    text: Option<CachedText>,
}

unsafe impl Send for D2DRenderer {}
unsafe impl Sync for D2DRenderer {}

impl D2DRenderer {
    pub fn new() -> windows::core::Result<Self> {
        let d2d_factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let dwrite_factory: IDWriteFactory =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let font_collection = crate::fonts::build_private_collection(&dwrite_factory);
        if font_collection.is_none() {
            eprintln!("[marquee] bundled fonts unavailable, using system fonts");
        }

        Ok(Self {
            d2d_factory,
            dwrite_factory,
            font_collection,
            dc_target: None,
            mem_dc: HDC::default(),
            hbitmap: HBITMAP::default(),
            old_hbitmap: HBITMAP::default(),
            bits: std::ptr::null_mut(),
            width: 0,
            height: 0,
            text: None,
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
                minLevel: windows::Win32::Graphics::Direct2D::D2D1_FEATURE_LEVEL_DEFAULT,
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

    fn text_layout(
        &mut self,
        width: u32,
        height: u32,
        config: &AppConfig,
        edge: Edge,
    ) -> windows::core::Result<(IDWriteTextLayout, f32)> {
        if let Some(cached) = &self.text {
            if cached.text == config.text
                && cached.font == config.font
                && cached.phrase_spacing == config.phrase_spacing
                && cached.width == width
                && cached.height == height
                && cached.edge == edge
            {
                return Ok((cached.layout.clone(), cached.single_width));
            }
        }

        unsafe {
            // Font setup
            let weight = if config.font.bold {
                DWRITE_FONT_WEIGHT_BOLD
            } else {
                DWRITE_FONT_WEIGHT_NORMAL
            };
            let style = if config.font.italic {
                DWRITE_FONT_STYLE_ITALIC
            } else {
                DWRITE_FONT_STYLE_NORMAL
            };

            let family = crate::fonts::family_for_collection(
                &config.font.family,
                self.font_collection.is_some(),
            );
            let family_hstr = HSTRING::from(family);
            let collection = if crate::fonts::is_bundled_family(&config.font.family) {
                self.font_collection.as_ref()
            } else {
                None
            };
            let text_format: IDWriteTextFormat = self.dwrite_factory.CreateTextFormat(
                PCWSTR(family_hstr.as_ptr()),
                collection,
                weight,
                style,
                DWRITE_FONT_STRETCH_NORMAL,
                config.font.size,
                PCWSTR(HSTRING::from("en-us").as_ptr()),
            )?;

            // Prevent DirectWrite from word wrapping onto line 2
            text_format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;

            // Repeat text string with customizable phrase spacing
            let spacing_str = " ".repeat(config.phrase_spacing as usize);
            let single_phrase_with_spacing = format!("{}{}", config.text, spacing_str);

            let is_vertical = matches!(edge, Edge::Left | Edge::Right);
            let screen_extent = if is_vertical {
                height as f32
            } else {
                width as f32
            };

            // Measure single phrase width first to dynamically compute required repetitions
            let single_text_u16: Vec<u16> = single_phrase_with_spacing.encode_utf16().collect();
            let single_layout: IDWriteTextLayout = self.dwrite_factory.CreateTextLayout(
                &single_text_u16,
                &text_format,
                10000.0,
                1000.0,
            )?;

            let mut metrics = std::mem::zeroed();
            let _ = single_layout.GetMetrics(&mut metrics);
            let single_width = if metrics.widthIncludingTrailingWhitespace > 1.0 {
                metrics.widthIncludingTrailingWhitespace
            } else if metrics.width > 1.0 {
                metrics.width
            } else {
                100.0
            };

            // Dynamically calculate repetitions so full_text always fills screen extent + extra buffer
            let repeat_count = ((screen_extent / single_width).ceil() as usize + 2).max(2);

            let mut full_text =
                String::with_capacity(single_phrase_with_spacing.len() * repeat_count);
            for _ in 0..repeat_count {
                full_text.push_str(&single_phrase_with_spacing);
            }

            let text_u16: Vec<u16> = full_text.encode_utf16().collect();

            // Set layout bounds large enough so DirectWrite never clips or wraps
            let max_layout_w = 100000.0;
            let max_layout_h = if is_vertical {
                width as f32
            } else {
                height as f32
            };

            let text_layout: IDWriteTextLayout = self.dwrite_factory.CreateTextLayout(
                &text_u16,
                &text_format,
                max_layout_w,
                max_layout_h,
            )?;

            self.text = Some(CachedText {
                text: config.text.clone(),
                font: config.font.clone(),
                phrase_spacing: config.phrase_spacing,
                width,
                height,
                edge,
                layout: text_layout.clone(),
                single_width,
            });
            Ok((text_layout, single_width))
        }
    }

    pub fn render_frame(
        &mut self,
        width: u32,
        height: u32,
        config: &AppConfig,
        offset: f32,
        edge: Edge,
    ) -> windows::core::Result<Option<HDC>> {
        if width == 0 || height == 0 {
            return Ok(None);
        }

        self.ensure_buffer(width, height)?;
        let (text_layout, single_width) = self.text_layout(width, height, config, edge)?;
        let is_vertical = matches!(edge, Edge::Left | Edge::Right);

        let dc_target = match self.dc_target.as_ref() {
            Some(target) => target,
            None => return Ok(None),
        };

        unsafe {
            dc_target.BeginDraw();

            // Background color (premultiplied alpha)
            let bg = config.colors.bg_color;
            let bg_color = D2D1_COLOR_F {
                r: bg[0] * bg[3],
                g: bg[1] * bg[3],
                b: bg[2] * bg[3],
                a: bg[3],
            };
            dc_target.Clear(Some(&bg_color));

            // Text Brush (premultiplied alpha)
            let fg = config.colors.text_color;
            let fg_color = D2D1_COLOR_F {
                r: fg[0] * fg[3],
                g: fg[1] * fg[3],
                b: fg[2] * fg[3],
                a: fg[3],
            };
            let text_brush: ID2D1SolidColorBrush =
                dc_target.CreateSolidColorBrush(&fg_color, None)?;

            let current_scroll = offset % single_width;

            if is_vertical {
                let y_start = -current_scroll;
                let x_center = (width as f32 - config.font.size) / 2.0;

                let point = D2D_POINT_2F {
                    x: x_center.max(0.0),
                    y: y_start,
                };

                dc_target.DrawTextLayout(
                    point,
                    &text_layout,
                    &text_brush,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
            } else {
                let x_start = -current_scroll;
                let y_center = (height as f32 - config.font.size) / 2.0;

                let point = D2D_POINT_2F {
                    x: x_start,
                    y: y_center.max(0.0),
                };

                dc_target.DrawTextLayout(
                    point,
                    &text_layout,
                    &text_brush,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
            }

            dc_target.EndDraw(None, None)?;
        }

        Ok(Some(self.mem_dc))
    }
}

impl Drop for D2DRenderer {
    fn drop(&mut self) {
        self.cleanup_buffer();
    }
}
