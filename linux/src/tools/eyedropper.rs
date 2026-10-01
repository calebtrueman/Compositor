//! Eyedropper: click to pick the foreground color (Alt for background) from the merged image or
//! the active layer.

use egui::Key;

use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::render::{self, Region};

#[derive(Default)]
pub struct Eyedropper {
    pub current_layer_only: bool,
    /// Averaging radius: 0 is a single pixel, 1 is 3×3, 2 is 5×5.
    pub radius: u32,
}

impl Tool for Eyedropper {
    fn kind(&self) -> ToolKind {
        ToolKind::Eyedropper
    }
    fn name(&self) -> &'static str {
        "Eyedropper"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::EYEDROPPER
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::I)
    }
    fn options_ui(&mut self, ui: &mut egui::Ui, _ctx: &mut ToolCtx) {
        egui::ComboBox::from_label("Sample Size")
            .selected_text(match self.radius {
                0 => "Point Sample",
                1 => "3 by 3 Average",
                _ => "5 by 5 Average",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.radius, 0, "Point Sample");
                ui.selectable_value(&mut self.radius, 1, "3 by 3 Average");
                ui.selectable_value(&mut self.radius, 2, "5 by 5 Average");
            });
        ui.checkbox(&mut self.current_layer_only, "Current Layer Only");
    }
    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        if !matches!(event.phase, PointerPhase::Press | PointerPhase::Drag) {
            return;
        }
        if let Some(color) = sample(ctx, event.pos, self.radius, self.current_layer_only) {
            if event.modifiers.alt {
                ctx.colors.background = color;
            } else {
                ctx.colors.foreground = color;
            }
        }
    }
}

/// The color at a document point, averaged over a square of `radius`.
pub fn sample(ctx: &ToolCtx, pos: (f64, f64), radius: u32, layer_only: bool) -> Option<[u8; 4]> {
    let doc = &ctx.project.doc;
    let (x, y) = (pos.0.floor() as i64, pos.1.floor() as i64);
    if x < 0 || y < 0 || x >= doc.width as i64 || y >= doc.height as i64 {
        return None;
    }
    let r = radius as i64;
    let region = Region { x: x - r, y: y - r, width: (2 * r + 1) as usize, height: (2 * r + 1) as usize, scale: 1.0 };
    let buffer = if layer_only {
        let layer = doc.active_layer()?;
        render::place_layer(layer, region, ctx.cache)?
    } else {
        render::composite(doc, region, ctx.cache)
    };
    let mut sum = [0.0f32; 4];
    for p in &buffer.px {
        for i in 0..4 {
            sum[i] += p[i];
        }
    }
    if sum[3] <= 0.0 {
        return None;
    }
    let inv = 1.0 / sum[3];
    Some([render::to_u8(sum[0] * inv), render::to_u8(sum[1] * inv), render::to_u8(sum[2] * inv), 255])
}
