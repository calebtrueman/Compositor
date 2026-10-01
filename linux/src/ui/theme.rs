//! A dark, compact look close to the macOS app's.

use egui::{Color32, CornerRadius, Visuals};

pub fn apply(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    let mut visuals = Visuals::dark();
    visuals.panel_fill = Color32::from_gray(38);
    visuals.window_fill = Color32::from_gray(44);
    visuals.extreme_bg_color = Color32::from_gray(28);
    visuals.faint_bg_color = Color32::from_gray(48);
    visuals.selection.bg_fill = Color32::from_rgb(38, 110, 210);
    visuals.window_corner_radius = CornerRadius::same(8);
    visuals.menu_corner_radius = CornerRadius::same(6);
    ctx.set_visuals_of(egui::Theme::Dark, visuals);
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 3.0);
        style.interaction.tooltip_delay = 0.4;
    });
}
