//! Brush: paints the foreground color (its gray on a mask). Shift-click draws a straight line
//! from where the last stroke ended; `[` and `]` change the size, Shift with them the hardness.

use egui::{CursorIcon, Key, Modifiers, Painter};

use super::paint::{self, BrushSettings, Fields, Paint, StrokeDriver, StrokeStep};
use super::{PointerEvent, Tool, ToolCtx, ToolKind};
use crate::project::Project;
use crate::ui::canvas::ViewTransform;

#[derive(Default)]
pub struct BrushTool {
    pub settings: BrushSettings,
    driver: StrokeDriver,
}

/// Runs a brush or eraser stroke: `paint` says what it lays.
pub fn stroke_pointer(
    driver: &mut StrokeDriver,
    settings: &BrushSettings,
    event: &PointerEvent,
    ctx: &mut ToolCtx,
    name: &str,
    paint: impl Fn(&ToolCtx, bool) -> Paint<'static>,
) {
    match driver.pointer(event, ctx, settings, name, true) {
        StrokeStep::None => {}
        StrokeStep::Painted => {
            let Some(stroke) = driver.stroke.as_mut() else { return };
            let paint = paint(ctx, stroke.surface.mask);
            stroke.flush(ctx.project, settings.opacity, &paint);
        }
        StrokeStep::Finished(mut stroke) => {
            let paint = paint(ctx, stroke.surface.mask);
            stroke.flush(ctx.project, settings.opacity, &paint);
            ctx.project.finish_edit();
        }
    }
}

/// Ends a stroke left open by a tool switch.
pub fn end_stroke(driver: &mut StrokeDriver, settings: &BrushSettings, ctx: &mut ToolCtx, paint: impl Fn(&ToolCtx, bool) -> Paint<'static>) {
    if let StrokeStep::Finished(mut stroke) = driver.end() {
        let paint = paint(ctx, stroke.surface.mask);
        stroke.flush(ctx.project, settings.opacity, &paint);
        ctx.project.finish_edit();
    }
}

fn brush_paint(ctx: &ToolCtx, mask: bool) -> Paint<'static> {
    Paint::Color(paint::paint_color(ctx, mask))
}

impl Tool for BrushTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Brush
    }
    fn name(&self) -> &'static str {
        "Brush"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::PAINT_BRUSH
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::B)
    }
    fn options_ui(&mut self, ui: &mut egui::Ui, _ctx: &mut ToolCtx) {
        paint::options_ui(ui, &mut self.settings, Fields::ALL);
    }
    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        stroke_pointer(&mut self.driver, &self.settings, event, ctx, "Brush", brush_paint);
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
        end_stroke(&mut self.driver, &self.settings, ctx, brush_paint);
    }
    fn cancel(&mut self, ctx: &mut ToolCtx) {
        self.commit(ctx);
    }
}
