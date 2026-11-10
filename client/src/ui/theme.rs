use eframe::egui::{self, Color32, CornerRadius, Stroke, Theme, ThemePreference};

use crate::config::ThemeMode;

pub const SPACE_1: f32 = 4.0;
pub const SPACE_2: f32 = 8.0;
pub const SPACE_3: f32 = 12.0;
pub const SPACE_4: f32 = 16.0;
pub const SPACE_5: f32 = 20.0;
pub const SPACE_6: f32 = 24.0;
pub const SPACE_8: f32 = 32.0;

pub const RADIUS_SMALL: u8 = 6;
pub const RADIUS_MEDIUM: u8 = 10;
pub const RADIUS_LARGE: u8 = 14;
pub const RADIUS_XLARGE: u8 = 18;

pub const ANIMATION_FAST_SECONDS: f32 = 0.12;
pub const ANIMATION_NORMAL_SECONDS: f32 = 0.18;
pub const ANIMATION_SLOW_SECONDS: f32 = 0.24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Palette {
    pub background: Color32,
    pub surface: Color32,
    pub surface_secondary: Color32,
    pub surface_hover: Color32,
    pub surface_selected: Color32,
    pub separator: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
}

pub const DARK: Palette = Palette {
    background: Color32::from_rgb(18, 19, 22),
    surface: Color32::from_rgb(27, 28, 33),
    surface_secondary: Color32::from_rgb(34, 35, 41),
    surface_hover: Color32::from_rgb(41, 43, 50),
    surface_selected: Color32::from_rgb(45, 52, 71),
    separator: Color32::from_rgb(52, 54, 62),
    text_primary: Color32::from_rgb(242, 243, 247),
    text_secondary: Color32::from_rgb(181, 184, 194),
    text_muted: Color32::from_rgb(125, 128, 138),
    accent: Color32::from_rgb(108, 124, 255),
    success: Color32::from_rgb(67, 201, 140),
    warning: Color32::from_rgb(238, 177, 82),
    danger: Color32::from_rgb(255, 112, 112),
};

pub const LIGHT: Palette = Palette {
    background: Color32::from_rgb(246, 247, 249),
    surface: Color32::from_rgb(255, 255, 255),
    surface_secondary: Color32::from_rgb(238, 240, 244),
    surface_hover: Color32::from_rgb(232, 235, 241),
    surface_selected: Color32::from_rgb(225, 230, 249),
    separator: Color32::from_rgb(216, 219, 226),
    text_primary: Color32::from_rgb(27, 29, 35),
    text_secondary: Color32::from_rgb(84, 88, 99),
    text_muted: Color32::from_rgb(130, 134, 145),
    accent: Color32::from_rgb(75, 93, 230),
    success: Color32::from_rgb(35, 154, 99),
    warning: Color32::from_rgb(184, 119, 24),
    danger: Color32::from_rgb(205, 68, 68),
};

pub fn apply(context: &egui::Context, mode: ThemeMode) {
    context.set_theme(match mode {
        ThemeMode::System => ThemePreference::System,
        ThemeMode::Light => ThemePreference::Light,
        ThemeMode::Dark => ThemePreference::Dark,
    });
    context.set_visuals_of(Theme::Dark, visuals(DARK, true));
    context.set_visuals_of(Theme::Light, visuals(LIGHT, false));
    context.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(SPACE_2, SPACE_2);
        style.spacing.button_padding = egui::vec2(SPACE_3, SPACE_2);
        style.spacing.menu_margin = egui::Margin::same(SPACE_3 as i8);
        style.animation_time = ANIMATION_NORMAL_SECONDS;
    });
}

pub fn current(context: &egui::Context) -> Palette {
    match context.theme() {
        Theme::Dark => DARK,
        Theme::Light => LIGHT,
    }
}

fn visuals(palette: Palette, dark: bool) -> egui::Visuals {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = palette.background;
    visuals.window_fill = palette.surface;
    visuals.extreme_bg_color = palette.surface_secondary;
    visuals.faint_bg_color = palette.surface_secondary;
    visuals.override_text_color = Some(palette.text_primary);
    visuals.weak_text_color = Some(palette.text_secondary);
    visuals.selection.bg_fill = palette.accent;
    visuals.selection.stroke = Stroke::new(1.0, palette.accent);
    visuals.window_stroke = Stroke::new(1.0, palette.separator);
    visuals.window_corner_radius = CornerRadius::same(RADIUS_LARGE);
    visuals.menu_corner_radius = CornerRadius::same(RADIUS_MEDIUM);
    visuals.warn_fg_color = palette.warning;
    visuals.error_fg_color = palette.danger;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.separator);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(RADIUS_SMALL);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(RADIUS_SMALL);
    visuals.widgets.active.corner_radius = CornerRadius::same(RADIUS_SMALL);
    visuals.widgets.hovered.weak_bg_fill = palette.surface_hover;
    visuals.widgets.active.weak_bg_fill = palette.surface_selected;
    visuals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_and_dark_palettes_preserve_semantic_contrast_direction() {
        assert!(DARK.background.r() < DARK.text_primary.r());
        assert!(LIGHT.background.r() > LIGHT.text_primary.r());
        assert_ne!(DARK.surface_selected, DARK.surface_hover);
        assert_ne!(LIGHT.surface_selected, LIGHT.surface_hover);
    }

    #[test]
    fn design_tokens_follow_the_documented_scale() {
        assert_eq!([SPACE_1, SPACE_2, SPACE_3, SPACE_4], [4.0, 8.0, 12.0, 16.0]);
        assert_eq!(
            [
                ANIMATION_FAST_SECONDS,
                ANIMATION_NORMAL_SECONDS,
                ANIMATION_SLOW_SECONDS,
            ],
            [0.12, 0.18, 0.24]
        );
        assert_eq!([RADIUS_SMALL, RADIUS_XLARGE], [6, 18]);
    }
}
