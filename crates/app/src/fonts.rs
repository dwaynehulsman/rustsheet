//! Uses San Francisco, the macOS system font, for the interface and cells,
//! with a bold instance taken from the same variable font. egui's bundled
//! fonts stay behind it as fallback.

use std::sync::Arc;

use egui::epaint::text::VariationCoords;
use egui::{FontData, FontDefinitions, FontFamily, FontTweak};

/// The family used for bold cell text.
pub fn bold_family() -> FontFamily {
    FontFamily::Name("bold".into())
}

/// The family for cell text with the given weight and slant.
pub fn family(bold: bool, italic: bool) -> FontFamily {
    match (bold, italic) {
        (false, false) => FontFamily::Proportional,
        (true, false) => bold_family(),
        (false, true) => FontFamily::Name("italic".into()),
        (true, true) => FontFamily::Name("bold-italic".into()),
    }
}

fn system_font(bytes: &Arc<Vec<u8>>, weight: f32) -> FontData {
    let tweak = FontTweak { coords: VariationCoords::new([(b"wght", weight)]), ..FontTweak::default() };
    FontData::from_owned(bytes.as_ref().clone()).tweak(tweak)
}

pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let proportional = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut bold = proportional.clone();

    if let Ok(sf) = std::fs::read("/System/Library/Fonts/SFNS.ttf").map(Arc::new) {
        defs.font_data.insert("sf".into(), Arc::new(system_font(&sf, 400.0)));
        defs.font_data.insert("sf-bold".into(), Arc::new(system_font(&sf, 700.0)));
        defs.families.entry(FontFamily::Proportional).or_default().insert(0, "sf".into());
        bold.insert(0, "sf-bold".into());
    }
    if let Ok(mono) = std::fs::read("/System/Library/Fonts/SFNSMono.ttf") {
        defs.font_data.insert("sf-mono".into(), Arc::new(FontData::from_owned(mono)));
        defs.families.entry(FontFamily::Monospace).or_default().insert(0, "sf-mono".into());
    }
    let mut italic = proportional.clone();
    let mut bold_italic = proportional.clone();
    if let Ok(sf) = std::fs::read("/System/Library/Fonts/SFNSItalic.ttf").map(Arc::new) {
        defs.font_data.insert("sf-italic".into(), Arc::new(system_font(&sf, 400.0)));
        defs.font_data.insert("sf-bold-italic".into(), Arc::new(system_font(&sf, 700.0)));
        italic.insert(0, "sf-italic".into());
        bold_italic.insert(0, "sf-bold-italic".into());
    }
    defs.families.insert(bold_family(), bold);
    defs.families.insert(family(false, true), italic);
    defs.families.insert(family(true, true), bold_italic);
    ctx.set_fonts(defs);
}
