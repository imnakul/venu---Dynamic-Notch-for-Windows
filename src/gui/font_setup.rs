use egui;

pub(super) fn setup_custom_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}

fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        "Geist".to_owned(),
        egui::FontData::from_static(include_bytes!("../../fonts/Geist.ttf")),
    );
    fonts.font_data.insert(
        "GeistMono".to_owned(),
        egui::FontData::from_static(include_bytes!("../../fonts/GeistMono.ttf")),
    );
    fonts.font_data.insert(
        "NotoSansDevanagari".to_owned(),
        egui::FontData::from_static(include_bytes!("../../NotoSansDevanagari.ttf")),
    );
    fonts.font_data.insert(
        "PlusJakartaSans".to_owned(),
        egui::FontData::from_static(include_bytes!("../../PlusJakartaSans.ttf")),
    );

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "Geist".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(1, "NotoSansDevanagari".to_owned());

    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "GeistMono".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(1, "NotoSansDevanagari".to_owned());

    // Keep the prior bundled family available to configs that explicitly
    // selected it. The defaults and migrated configurations use Geist.
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push("PlusJakartaSans".to_owned());

    fonts
}

#[cfg(test)]
mod tests {
    use super::setup_custom_fonts;
    use egui::{self, FontId};

    #[test]
    fn production_font_setup_renders_latin_and_devanagari_fallback() {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);

        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("Venu settings");
                ui.monospace("CPU 0.0% · नमस्ते");
            });
        });

        assert!(
            !output.shapes.is_empty(),
            "the headless frame should paint text"
        );
        ctx.fonts(|fonts| {
            assert!(fonts.has_glyphs(&FontId::proportional(18.0), "Venu नमस्ते"));
            assert!(fonts.has_glyphs(&FontId::monospace(18.0), "CPU 0.0% नमस्ते"));
        });
    }
}
