//! Destructive filters and image-wide operations (Image > Adjustments, Image Size, Canvas
//! Size, the Filter menu). Placeholder until the filter set lands.

use crate::app::MenuCtx;

pub fn adjustments_menu(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    let has = ctx.project.is_some();
    if ui.add_enabled(has, egui::Button::new("Invert").shortcut_text("Ctrl+I")).clicked() {
        if let Some(p) = ctx.project.as_deref_mut() {
            crate::adjust::invert_active(p);
        }
        ui.close();
    }
}

pub fn image_size_items(_ui: &mut egui::Ui, _ctx: &mut MenuCtx) {}

pub fn filter_menu(ui: &mut egui::Ui, _ctx: &mut MenuCtx) {
    ui.label("No filters yet");
}
