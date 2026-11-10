use std::{fs, path::Path};

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText};

use crate::config::ThemeMode;

pub mod theme;

pub fn configure(context: &egui::Context, mode: ThemeMode) {
    theme::apply(context, mode);
}

pub fn configure_fonts(context: &egui::Context) {
    let Some((path, bytes)) = CJK_FONT_CANDIDATES
        .iter()
        .find_map(|path| fs::read(path).ok().map(|bytes| (*path, bytes)))
    else {
        tracing::warn!("no system CJK font was found; Chinese glyphs may be unavailable");
        return;
    };

    let mut fonts = FontDefinitions::default();
    let name = "edenvoice-cjk".to_owned();
    fonts
        .font_data
        .insert(name.clone(), FontData::from_owned(bytes).into());
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push(name.clone());
    }
    context.set_fonts(fonts);
    tracing::info!(font = %Path::new(path).display(), "configured CJK font fallback");
}

const CJK_FONT_CANDIDATES: &[&str] = &[
    // macOS
    "/System/Library/Fonts/STHeiti Medium.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/Library/Fonts/Arial Unicode.ttf",
    // Windows
    "C:\\Windows\\Fonts\\msyh.ttc",
    "C:\\Windows\\Fonts\\simhei.ttf",
    // Linux
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
];

pub fn status_dot(ui: &mut egui::Ui, color: Color32, label: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("●").color(color));
        ui.label(label);
    });
}
