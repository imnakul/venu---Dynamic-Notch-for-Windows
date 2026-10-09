//! Fonts shipped privately with Venu.
//!
//! The embedded DirectWrite collection keeps Geist available without
//! installing anything in Windows. System fallback remains active for scripts
//! and symbols that the bundled faces do not contain.

use windows::core::Interface;
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, IDWriteFactory5, IDWriteFontCollection,
};

pub use crate::font_families::{
    family_for_collection, is_bundled_family, GEIST, GEIST_MONO, SYSTEM_FALLBACK,
};

static BUNDLED_FONTS: [(&[u8], &str); 4] = [
    (include_bytes!("../fonts/Geist.ttf"), GEIST),
    (include_bytes!("../fonts/GeistMono.ttf"), GEIST_MONO),
    (
        include_bytes!("../PlusJakartaSans.ttf"),
        crate::font_families::BUNDLED_FAMILY_NAMES[2],
    ),
    (
        include_bytes!("../NotoSansDevanagari.ttf"),
        crate::font_families::BUNDLED_FAMILY_NAMES[3],
    ),
];

pub fn bundled_fonts() -> &'static [(&'static [u8], &'static str)] {
    &BUNDLED_FONTS
}

/// Build one in-memory DirectWrite collection for a renderer/factory pair.
/// Each renderer keeps the returned collection for its lifetime and passes it
/// only when the requested family is in this collection.
pub fn build_private_collection(factory: &IDWriteFactory) -> Option<IDWriteFontCollection> {
    let factory5: IDWriteFactory5 = factory.cast().ok()?;

    unsafe {
        let loader = factory5.CreateInMemoryFontFileLoader().ok()?;
        factory5.RegisterFontFileLoader(&loader).ok()?;
        let builder = factory5.CreateFontSetBuilder().ok()?;

        for (data, _) in bundled_fonts() {
            let file = loader
                .CreateInMemoryFontFileReference(
                    &factory5,
                    data.as_ptr() as *const std::ffi::c_void,
                    data.len() as u32,
                    None,
                )
                .ok()?;
            builder.AddFontFile(&file).ok()?;
        }

        let font_set = builder.CreateFontSet().ok()?;
        let collection = factory5.CreateFontCollectionFromFontSet(&font_set).ok()?;
        Some(collection.into())
    }
}
