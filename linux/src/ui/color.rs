//! Foreground/background swatches under the toolbar, as in Photoshop.

use egui::{Color32, Sense, Stroke, Vec2};

use crate::tools::Colors;

pub fn swatches(ui: &mut egui::Ui, colors: &mut Colors) {
    let size = Vec2::new(40.0, 40.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let swatch = Vec2::splat(24.0);
    let bg_rect = egui::Rect::from_min_size(rect.min + Vec2::new(14.0, 14.0), swatch);
    let fg_rect = egui::Rect::from_min_size(rect.min + Vec2::new(2.0, 2.0), swatch);
    let painter = ui.painter();
    painter.rect_filled(bg_rect, 2.0, colors.bg32());
    painter.rect_stroke(bg_rect, 2.0, Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
    painter.rect_filled(fg_rect, 2.0, colors.fg32());
    painter.rect_stroke(fg_rect, 2.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Inside);

    let bg_response = ui.interact(bg_rect, ui.id().with("bg-swatch"), Sense::click());
    let fg_response = ui.interact(fg_rect, ui.id().with("fg-swatch"), Sense::click());
    let fg_popup = egui::Popup::from_toggle_button_response(&fg_response);
    fg_popup.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.label("Foreground Color");
        picker(ui, &mut colors.foreground);
    });
    let bg_popup = egui::Popup::from_toggle_button_response(&bg_response);
    bg_popup.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.label("Background Color");
        picker(ui, &mut colors.background);
    });
    fg_response.on_hover_text("Foreground color (X swaps, D resets)");
    ui.horizontal(|ui| {
        if ui.small_button(crate::ui::icons::SWAP).on_hover_text("Swap (X)").clicked() {
            std::mem::swap(&mut colors.foreground, &mut colors.background);
        }
        if ui.small_button(crate::ui::icons::SQUARE_HALF).on_hover_text("Default colors (D)").clicked() {
            *colors = Colors::default();
        }
    });
}

/// An HSV picker with a hex field.
pub fn picker(ui: &mut egui::Ui, color: &mut [u8; 4]) {
    let mut c = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
    egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque);
    *color = c.to_srgba_unmultiplied();
    ui.horizontal(|ui| {
        ui.label("#");
        let mut hex = format!("{:02X}{:02X}{:02X}", color[0], color[1], color[2]);
        if ui.add(egui::TextEdit::singleline(&mut hex).desired_width(70.0)).changed() {
            if let Some(parsed) = parse_hex(&hex) {
                color[..3].copy_from_slice(&parsed);
            }
        }
    });
}

pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let t = text.trim().trim_start_matches('#');
    if t.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}
