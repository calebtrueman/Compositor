//! Lasso: freehand (drag around an area) or polygonal (click corners; double-click, click the
//! first point or press Enter to close, Backspace removes the last point, Escape cancels; Shift
//! keeps segments to 45°). Modifiers combine with the selection as the Marquee's do.

use egui::{Color32, CursorIcon, Key, Modifiers, Painter, Stroke};

use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::doc::selection::SelectionOp;
use crate::project::Project;
use crate::selection::{self, raster, OutlineMove};
use crate::ui::canvas::ViewTransform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LassoMode {
    Freehand,
    Polygonal,
}

/// How close (in screen points) a click must come to the first point to close the polygon.
const CLOSE_RADIUS: f64 = 6.0;

pub struct LassoTool {
    pub lasso: LassoMode,
    pub feather: f64,
    pub anti_alias: bool,
    pub mode: SelectionOp,
    /// The outline being drawn, in document coordinates.
    points: Vec<(f64, f64)>,
    drawing: bool,
    op: SelectionOp,
    /// Shift chose Add at the press, so it doesn't constrain until pressed again.
    shift_latched: bool,
    shift_down: bool,
    outline: Option<OutlineMove>,
    /// Where a double-click just closed a polygon, so the click's own press doesn't start another.
    closed_at: Option<(f64, f64)>,
    zoom: f32,
}

impl Default for LassoTool {
    fn default() -> Self {
        LassoTool {
            lasso: LassoMode::Freehand,
            feather: 0.0,
            anti_alias: true,
            mode: SelectionOp::Replace,
            points: Vec::new(),
            drawing: false,
            op: SelectionOp::Replace,
            shift_latched: false,
            shift_down: false,
            outline: None,
            closed_at: None,
            zoom: 1.0,
        }
    }
}

/// `pos` moved so the segment from `from` runs at a multiple of 45°.
fn constrain_45(from: (f64, f64), pos: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (pos.0 - from.0, pos.1 - from.1);
    let step = std::f64::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    // Project onto the snapped direction.
    let length = dx * angle.cos() + dy * angle.sin();
    (from.0 + length * angle.cos(), from.1 + length * angle.sin())
}

impl LassoTool {
    fn polygonal_point(&self, pos: (f64, f64)) -> (f64, f64) {
        match self.points.last() {
            Some(last) if self.shift_down && !self.shift_latched => constrain_45(*last, pos),
            _ => pos,
        }
    }

    fn near_first(&self, pos: (f64, f64)) -> bool {
        self.points.len() >= 3
            && self.points.first().is_some_and(|f| (f.0 - pos.0).hypot(f.1 - pos.1) * self.zoom as f64 <= CLOSE_RADIUS)
    }

    fn start(&mut self, pos: (f64, f64), modifiers: Modifiers, project: &Project) {
        let has_selection = project.doc.selection.is_some();
        self.op = selection::op_for(self.mode, modifiers, has_selection);
        self.shift_latched = has_selection && modifiers.shift;
        self.points = vec![pos];
        self.drawing = true;
    }

    /// Closes the outline and selects inside it. Too few points select nothing (and a plain
    /// click in New mode deselects).
    fn close(&mut self, project: &mut Project) {
        if !self.drawing {
            return;
        }
        self.drawing = false;
        let points = std::mem::take(&mut self.points);
        let mask = raster::polygon_mask(&points, project.doc.width, project.doc.height, self.anti_alias);
        if crate::doc::selection::tight_bounds(&mask).is_some() {
            let name = if self.lasso == LassoMode::Polygonal { "Polygonal Lasso" } else { "Lasso" };
            selection::apply_shape(project, name, mask, self.feather, self.op);
        } else if self.op == SelectionOp::Replace {
            selection::deselect(project);
        }
    }

    fn abandon(&mut self) {
        self.drawing = false;
        self.points.clear();
    }

    fn polygon_in_progress(&self) -> bool {
        self.drawing && self.lasso == LassoMode::Polygonal
    }
}

impl Tool for LassoTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Lasso
    }
    fn name(&self) -> &'static str {
        "Lasso"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::LASSO
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::L)
    }

    fn options_ui(&mut self, ui: &mut egui::Ui, ctx: &mut ToolCtx) {
        // While a polygon is open the tool has the keyboard (see `captures_keyboard`), so its
        // keys are read here.
        if self.polygon_in_progress() && !ui.ctx().egui_wants_keyboard_input() {
            let (enter, escape, back) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Enter),
                    i.consume_key(Modifiers::NONE, Key::Escape),
                    i.consume_key(Modifiers::NONE, Key::Backspace) || i.consume_key(Modifiers::NONE, Key::Delete),
                )
            });
            if enter {
                self.close(ctx.project);
            } else if escape {
                self.abandon();
            } else if back {
                self.points.pop();
                if self.points.is_empty() {
                    self.abandon();
                }
            }
        }
        selection::mode_buttons(ui, &mut self.mode);
        ui.separator();
        use crate::ui::icons;
        let before = self.lasso;
        ui.selectable_value(&mut self.lasso, LassoMode::Freehand, egui::RichText::new(icons::LASSO).size(16.0)).on_hover_text("Freehand");
        ui.selectable_value(&mut self.lasso, LassoMode::Polygonal, egui::RichText::new(icons::POLYGON).size(16.0))
            .on_hover_text("Polygonal: click corners, double-click or Enter to close");
        if self.lasso != before {
            self.abandon();
        }
        ui.separator();
        ui.label("Feather");
        ui.add(egui::DragValue::new(&mut self.feather).range(0.0..=250.0).speed(0.2).max_decimals(1).suffix(" px"));
        ui.checkbox(&mut self.anti_alias, "Anti-alias");
    }

    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        self.zoom = ctx.zoom;
        self.shift_down = event.modifiers.shift;
        if !event.modifiers.shift {
            self.shift_latched = false;
        }
        match (self.lasso, event.phase) {
            (_, PointerPhase::Press) if self.outline.is_none() && !self.drawing => {
                if self.closed_at.take().is_some_and(|p| p == event.pos) {
                    return;
                }
                if OutlineMove::starts_at(ctx.project, event.pos, event.modifiers, self.mode) {
                    self.outline = OutlineMove::new(ctx.project);
                } else {
                    self.start(event.pos, event.modifiers, ctx.project);
                }
            }
            (LassoMode::Polygonal, PointerPhase::Press) if self.drawing => {
                if self.near_first(event.pos) {
                    self.close(ctx.project);
                    return;
                }
                let p = self.polygonal_point(event.pos);
                // The second click of a double-click lands on the point the first one made.
                let repeat = self.points.last().is_some_and(|l| (l.0 - p.0).hypot(l.1 - p.1) * (ctx.zoom as f64) < 2.0);
                if !repeat {
                    self.points.push(p);
                }
            }
            (_, PointerPhase::Drag) => {
                if let Some(outline) = &mut self.outline {
                    outline.drag(ctx.project, event.press_origin, event.pos, event.modifiers.shift);
                } else if self.drawing && self.lasso == LassoMode::Freehand {
                    let far = self.points.last().is_none_or(|l| (l.0 - event.pos.0).hypot(l.1 - event.pos.1) * ctx.zoom as f64 >= 1.0);
                    if far {
                        self.points.push(event.pos);
                    }
                }
            }
            (_, PointerPhase::Release) => {
                if let Some(outline) = self.outline.take() {
                    if !outline.finish(ctx.project) {
                        // A click inside the selection: deselect, or start a polygon there.
                        match self.lasso {
                            LassoMode::Freehand => selection::deselect(ctx.project),
                            LassoMode::Polygonal => self.start(event.press_origin, Modifiers::NONE, ctx.project),
                        }
                    }
                } else if self.drawing && self.lasso == LassoMode::Freehand {
                    self.close(ctx.project);
                }
            }
            (LassoMode::Polygonal, PointerPhase::DoubleClick) if self.drawing => {
                self.close(ctx.project);
                self.closed_at = Some(event.pos);
            }
            (_, PointerPhase::Hover) => self.closed_at = None,
            _ => {}
        }
    }

    fn commit(&mut self, ctx: &mut ToolCtx) {
        if self.polygon_in_progress() {
            self.close(ctx.project);
        }
    }

    fn cancel(&mut self, ctx: &mut ToolCtx) {
        self.abandon();
        if let Some(outline) = self.outline.take() {
            outline.finish(ctx.project);
        }
    }

    fn captures_keyboard(&self) -> bool {
        self.polygon_in_progress()
    }

    fn overlay(&self, painter: &Painter, view: &ViewTransform, _project: &Project, hover: Option<(f64, f64)>) {
        if !self.drawing {
            return;
        }
        let mut screen: Vec<egui::Pos2> = self.points.iter().map(|p| view.to_screen(p.0, p.1)).collect();
        if self.lasso == LassoMode::Polygonal {
            if let Some(h) = hover {
                let closing = self.near_first(h);
                let p = if closing { self.points[0] } else { self.polygonal_point(h) };
                screen.push(view.to_screen(p.0, p.1));
                if closing {
                    let r = egui::Rect::from_center_size(screen[0], egui::vec2(8.0, 8.0));
                    painter.rect_filled(r, 0.0, Color32::WHITE);
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Inside);
                }
            }
        }
        selection::draw_dashed(painter, &screen, false);
    }

    fn cursor(&self, project: &Project, hover: (f64, f64), modifiers: Modifiers) -> CursorIcon {
        if !self.drawing && self.outline.is_none() && OutlineMove::starts_at(project, hover, modifiers, self.mode) {
            CursorIcon::Move
        } else {
            CursorIcon::Crosshair
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_snap_to_45_degrees() {
        let p = constrain_45((0.0, 0.0), (10.0, 1.0));
        assert!((p.0 - 10.0).abs() < 1e-9 && p.1.abs() < 1e-9);
        let d = constrain_45((0.0, 0.0), (9.0, 11.0));
        assert!((d.0 - d.1).abs() < 1e-9);
    }
}
