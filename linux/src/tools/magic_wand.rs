//! Magic Wand: click to select pixels of a similar color, connected to the clicked one or
//! anywhere. Shift adds, Alt subtracts, Shift+Alt intersects; dragging inside the selection
//! moves its outline.

use egui::{CursorIcon, Key, Modifiers};

use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::doc::selection::SelectionOp;
use crate::project::Project;
use crate::selection::{self, modify, wand, OutlineMove};

pub struct MagicWandTool {
    pub mode: SelectionOp,
    outline: Option<OutlineMove>,
}

impl Default for MagicWandTool {
    fn default() -> Self {
        MagicWandTool { mode: SelectionOp::Replace, outline: None }
    }
}

impl MagicWandTool {
    fn select(&self, project: &mut Project, pos: (f64, f64), modifiers: Modifiers, cache: &crate::render::RenderCache) {
        let (x, y) = (pos.0.floor(), pos.1.floor());
        if x < 0.0 || y < 0.0 || x >= project.doc.width as f64 || y >= project.doc.height as f64 {
            return;
        }
        let settings = wand::settings();
        let op = selection::op_for(self.mode, modifiers, project.doc.selection.is_some());
        let Some(sample) = wand::sample_image(&project.doc, settings.sample_all_layers, cache) else { return };
        let mut mask = wand::wand_mask(&sample, (x as u32, y as u32), settings.sample_radius, settings.tolerance as i32, settings.contiguous);
        if settings.anti_alias {
            modify::soften_edges(&mut mask);
        }
        selection::apply(project, "Magic Wand", mask, op);
    }
}

impl Tool for MagicWandTool {
    fn kind(&self) -> ToolKind {
        ToolKind::MagicWand
    }
    fn name(&self) -> &'static str {
        "Magic Wand"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::MAGIC_WAND
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::W)
    }

    fn options_ui(&mut self, ui: &mut egui::Ui, _ctx: &mut ToolCtx) {
        selection::mode_buttons(ui, &mut self.mode);
        ui.separator();
        let mut s = wand::settings();
        egui::ComboBox::from_id_salt("wand-sample")
            .selected_text(["Point Sample", "3 by 3 Average", "5 by 5 Average"][s.sample_radius.min(2) as usize])
            .show_ui(ui, |ui| {
                for (r, name) in ["Point Sample", "3 by 3 Average", "5 by 5 Average"].iter().enumerate() {
                    ui.selectable_value(&mut s.sample_radius, r as u32, *name);
                }
            });
        ui.label("Tolerance");
        ui.add(egui::DragValue::new(&mut s.tolerance).range(0..=255))
            .on_hover_text("How far each channel may differ from the clicked color");
        ui.checkbox(&mut s.anti_alias, "Anti-alias");
        ui.checkbox(&mut s.contiguous, "Contiguous").on_hover_text("Only pixels connected to the clicked one");
        ui.checkbox(&mut s.sample_all_layers, "Sample All Layers");
        if s != wand::settings() {
            wand::set_settings(s);
        }
    }

    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        match event.phase {
            PointerPhase::Press => {
                if OutlineMove::starts_at(ctx.project, event.pos, event.modifiers, self.mode) {
                    // A drag moves the outline; a click (decided on release) selects as usual.
                    self.outline = OutlineMove::new(ctx.project);
                } else {
                    self.select(ctx.project, event.pos, event.modifiers, ctx.cache);
                }
            }
            PointerPhase::Drag => {
                if let Some(outline) = &mut self.outline {
                    outline.drag(ctx.project, event.press_origin, event.pos, event.modifiers.shift);
                }
            }
            PointerPhase::Release => {
                if let Some(outline) = self.outline.take() {
                    if !outline.finish(ctx.project) {
                        self.select(ctx.project, event.press_origin, Modifiers::NONE, ctx.cache);
                    }
                }
            }
            _ => {}
        }
    }

    fn cancel(&mut self, ctx: &mut ToolCtx) {
        if let Some(outline) = self.outline.take() {
            outline.finish(ctx.project);
        }
    }

    fn cursor(&self, project: &Project, hover: (f64, f64), modifiers: Modifiers) -> CursorIcon {
        if self.outline.is_none() && OutlineMove::starts_at(project, hover, modifiers, self.mode) {
            CursorIcon::Move
        } else {
            CursorIcon::Crosshair
        }
    }
}
