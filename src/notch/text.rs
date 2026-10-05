//! DirectWrite plumbing for the notch.
//!
//! The fonts module registers the bundled typefaces as a private, in-memory
//! collection so the notch gets Geist and Geist Mono without installing
//! anything. System font fallback still applies on top of the private
//! collection, so emoji, CJK and Devanagari in the marquee keep resolving.

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteInlineObject,
    IDWriteTextFormat, IDWriteTextLayout, IDWriteTextLayout1, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_TEXT_RANGE,
    DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING,
    DWRITE_WORD_WRAPPING_NO_WRAP,
};

/// Used whenever the configured family name is blank.
const FALLBACK_FAMILY: &str = crate::fonts::GEIST;

pub struct TextEngine {
    pub factory: IDWriteFactory,
    /// `None` when the private collection could not be built (pre-1607
    /// Windows, or a malformed font file). Everything degrades to system
    /// fonts rather than failing.
    private_collection: Option<IDWriteFontCollection>,
    private_families: Vec<String>,
}

unsafe impl Send for TextEngine {}
unsafe impl Sync for TextEngine {}

impl TextEngine {
    pub fn new() -> windows::core::Result<Self> {
        let factory: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };

        let (private_collection, private_families) =
            match crate::fonts::build_private_collection(&factory) {
                Some(collection) => (
                    Some(collection),
                    crate::fonts::bundled_fonts()
                        .iter()
                        .map(|(_, name)| name.to_string())
                        .collect(),
                ),
                None => {
                    eprintln!("[notch] private font collection unavailable, using system fonts");
                    (None, Vec::new())
                }
            };

        Ok(Self {
            factory,
            private_collection,
            private_families,
        })
    }

    fn is_private(&self, family: &str) -> bool {
        self.private_families
            .iter()
            .any(|f| f.eq_ignore_ascii_case(family))
    }

    /// The family to actually ask DirectWrite for, given what the user picked.
    pub fn resolve_family(requested: &str) -> &str {
        if requested.trim().is_empty() {
            FALLBACK_FAMILY
        } else {
            requested
        }
    }

    pub fn format(
        &self,
        family: &str,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
    ) -> windows::core::Result<IDWriteTextFormat> {
        let mut family = Self::resolve_family(family);
        let locale = HSTRING::from("en-us");

        // Only hand DirectWrite the private collection when the requested
        // family actually lives in it; otherwise use the system family list.
        let has_private = self.is_private(family) && self.private_collection.is_some();
        if crate::fonts::is_bundled_family(family) && !has_private {
            family = crate::fonts::SYSTEM_FALLBACK;
        }
        let family_hstr = HSTRING::from(family);
        let collection = if has_private {
            self.private_collection.clone()
        } else {
            None
        };

        let format = unsafe {
            self.factory.CreateTextFormat(
                PCWSTR(family_hstr.as_ptr()),
                collection.as_ref(),
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size.max(1.0),
                PCWSTR(locale.as_ptr()),
            )?
        };

        unsafe {
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        }

        Ok(format)
    }

    pub fn set_wrapping(format: &IDWriteTextFormat, wrapping: DWRITE_WORD_WRAPPING) {
        unsafe {
            let _ = format.SetWordWrapping(wrapping);
        }
    }

    /// Clip overflow with a real ellipsis glyph instead of a hard cut.
    pub fn set_ellipsis(&self, format: &IDWriteTextFormat) {
        unsafe {
            if let Ok(sign) = self.factory.CreateEllipsisTrimmingSign(format) {
                let sign: IDWriteInlineObject = sign;
                let trimming = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                let _ = format.SetTrimming(&trimming, &sign);
            }
        }
    }

    pub fn layout(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
        max_w: f32,
        max_h: f32,
    ) -> windows::core::Result<IDWriteTextLayout> {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            self.factory
                .CreateTextLayout(&utf16, format, max_w.max(1.0), max_h.max(1.0))
        }
    }

    /// Letter-spacing. Small tracked labels are unreadable without it, and it
    /// is the cheapest single thing that makes the panel look designed.
    pub fn set_tracking(layout: &IDWriteTextLayout, tracking: f32, char_count: u32) {
        if char_count == 0 {
            return;
        }
        if let Ok(layout1) = layout.cast::<IDWriteTextLayout1>() {
            let range = DWRITE_TEXT_RANGE {
                startPosition: 0,
                length: char_count,
            };
            unsafe {
                let _ = layout1.SetCharacterSpacing(0.0, tracking, 0.0, range);
            }
        }
    }

    /// `(width, height)` of the laid-out text, including trailing whitespace.
    pub fn measure(layout: &IDWriteTextLayout) -> (f32, f32) {
        unsafe {
            let mut metrics = std::mem::zeroed();
            if layout.GetMetrics(&mut metrics).is_err() {
                return (0.0, 0.0);
            }
            (
                metrics.widthIncludingTrailingWhitespace.max(metrics.width),
                metrics.height,
            )
        }
    }
}
