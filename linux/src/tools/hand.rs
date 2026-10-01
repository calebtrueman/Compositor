//! Hand (pan) and Zoom tools. Panning itself happens in the canvas view, which knows the screen
//! geometry; these tools only claim the pointer.

use egui::{CursorIcon, Key, Modifiers};

use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::project::Project;

pub struct HandTool;

impl Tool for HandTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Hand
    }
    fn name(&self) -> &'static str {
        "Hand"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::HAND
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::H)
    }
    fn pointer(&mut self, _event: &PointerEvent, _ctx: &mut ToolCtx) {}
    fn cursor(&self, _project: &Project, _hover: (f64, f64), _modifiers: Modifiers) -> CursorIcon {
        CursorIcon::Grab
    }
}

pub struct ZoomTool;

impl Tool for ZoomTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Zoom
    }
    fn name(&self) -> &'static str {
        "Zoom"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::MAGNIFYING_GLASS
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::Z)
    }
    fn options_ui(&mut self, ui: &mut egui::Ui, ctx: &mut ToolCtx) {
        ui.label("Click to zoom in, Alt-click to zoom out");
        if ui.button("Fit on Screen").clicked() {
            ctx.project.view.request_fit();
        }
        if ui.button("100%").clicked() {
            ctx.project.view.request_zoom(1.0);
        }
    }
    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        if event.phase == PointerPhase::Press {
            let factor = if event.modifiers.alt { 0.5 } else { 2.0 };
            ctx.project.view.zoom_about_doc(event.pos, factor);
        }
    }
    fn cursor(&self, _project: &Project, _hover: (f64, f64), modifiers: Modifiers) -> CursorIcon {
        if modifiers.alt { CursorIcon::ZoomOut } else { CursorIcon::ZoomIn }
    }
}
