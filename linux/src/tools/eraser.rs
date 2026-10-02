//! Eraser: clears the layer's pixels under the brush (on a mask it paints black, hiding).
//! Same brush settings and keys as the Brush.

use egui::{CursorIcon, Key, Modifiers, Painter};

use super::brush::{end_stroke, stroke_pointer};
use super::paint::{self, BrushSettings, Fields, Paint, StrokeDriver};
use super::{PointerEvent, Tool, ToolCtx, ToolKind};
use crate::project::Project;
use crate::ui::canvas::ViewTransform;

#[derive(Default)]
pub struct EraserTool {
    pub settings: BrushSettings,
    driver: StrokeDriver,
}

fn erase(_ctx: &ToolCtx, _mask: bool) -> Paint<'static> {
    Paint::Erase
}

impl Tool for EraserTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Eraser
    }
    fn name(&self) -> &'static str {
        "Eraser"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::ERASER
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::E)
    }
    fn options_ui(&mut self, ui: &mut egui::Ui, _ctx: &mut ToolCtx) {
        paint::options_ui(ui, &mut self.settings, Fields::ALL);
    }
    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        stroke_pointer(&mut self.driver, &self.settings, event, ctx, "Eraser", erase);
    }
    fn key(&mut self, key: Key, modifiers: Modifiers, _ctx: &mut ToolCtx) -> bool {
        self.driver.stroke.is_none() && self.settings.key(key, modifiers)
    }
    fn overlay(&self, painter: &Painter, view: &ViewTransform, _project: &Project, hover: Option<(f64, f64)>) {
        if let Some(pos) = hover {
            paint::draw_outline(painter, view, pos, self.settings.size);
        }
    }
    fn cursor(&self, project: &Project, _hover: (f64, f64), _modifiers: Modifiers) -> CursorIcon {
        paint::cursor(self.settings.size, project.view.zoom)
    }
    fn commit(&mut self, ctx: &mut ToolCtx) {
        end_stroke(&mut self.driver, &self.settings, ctx, erase);
    }
    fn cancel(&mut self, ctx: &mut ToolCtx) {
        self.commit(ctx);
    }
}
