//! Marquee: drag a rectangle or ellipse to select it. With nothing selected Shift makes a square
//! or circle and Alt draws from the center; with a selection Shift adds, Alt subtracts and
//! Shift+Alt intersects (let go and press again to constrain instead). Dragging inside the
//! selection moves its outline. A click deselects.

use egui::{CursorIcon, Key, Modifiers, Painter};

use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::doc::selection::SelectionOp;
use crate::project::Project;
use crate::selection::{self, raster, OutlineMove};
use crate::ui::canvas::ViewTransform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarqueeShape {
    Rectangle,
    Ellipse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarqueeStyle {
    Normal,
    FixedRatio,
    FixedSize,
}

enum Drag {
    Draw {
        op: SelectionOp,
        /// Shift or Alt chose the combine mode at the press; they constrain only once pressed again.
        shift_latched: bool,
        alt_latched: bool,
        rect: Option<(f64, f64, f64, f64)>,
    },
    Outline(OutlineMove),
}

pub struct MarqueeTool {
    pub shape: MarqueeShape,
    pub style: MarqueeStyle,
    /// Width : height for Fixed Ratio.
    pub ratio: (f64, f64),
    /// Pixels for Fixed Size.
    pub size: (f64, f64),
    pub feather: f64,
    pub anti_alias: bool,
    pub mode: SelectionOp,
    drag: Option<Drag>,
}

impl Default for MarqueeTool {
    fn default() -> Self {
        MarqueeTool {
            shape: MarqueeShape::Rectangle,
            style: MarqueeStyle::Normal,
            ratio: (1.0, 1.0),
            size: (64.0, 64.0),
            feather: 0.0,
            anti_alias: true,
            mode: SelectionOp::Replace,
            drag: None,
        }
    }
}

/// The box a drag from `anchor` to `point` spans, in whole pixels `(x0, y0, x1, y1)`. `ratio`
/// fixes width / height; `center` grows the box around the anchor.
pub fn drag_box(anchor: (f64, f64), point: (f64, f64), ratio: Option<f64>, center: bool) -> (f64, f64, f64, f64) {
    let (ax, ay) = (anchor.0.round(), anchor.1.round());
    let mut dx = point.0.round() - ax;
    let mut dy = point.1.round() - ay;
    if let Some(r) = ratio.filter(|r| r.is_finite() && *r > 0.0) {
        let sign = |v: f64| if v < 0.0 { -1.0 } else { 1.0 };
        if dx.abs() > dy.abs() * r {
            dy = sign(dy) * (dx.abs() / r).round();
        } else {
            dx = sign(dx) * (dy.abs() * r).round();
        }
    }
    if center {
        (ax - dx.abs(), ay - dy.abs(), ax + dx.abs(), ay + dy.abs())
    } else {
        (ax.min(ax + dx), ay.min(ay + dy), ax.max(ax + dx), ay.max(ay + dy))
    }
}

impl MarqueeTool {
    fn rect_for(&self, anchor: (f64, f64), point: (f64, f64), square: bool, center: bool) -> (f64, f64, f64, f64) {
        match self.style {
            MarqueeStyle::FixedSize => {
                let (w, h) = (self.size.0.round().max(1.0), self.size.1.round().max(1.0));
                let (x, y) = (point.0.round(), point.1.round());
                if center {
                    let (x0, y0) = ((point.0 - w / 2.0).round(), (point.1 - h / 2.0).round());
                    (x0, y0, x0 + w, y0 + h)
                } else {
                    (x, y, x + w, y + h)
                }
            }
            MarqueeStyle::FixedRatio => drag_box(anchor, point, Some(self.ratio.0 / self.ratio.1.max(1e-6)), center),
            MarqueeStyle::Normal => drag_box(anchor, point, square.then_some(1.0), center),
        }
    }

    fn edit_name(&self) -> &'static str {
        match self.shape {
            MarqueeShape::Rectangle => "Rectangular Marquee",
            MarqueeShape::Ellipse => "Elliptical Marquee",
        }
    }

    fn finish_shape(&self, project: &mut Project, rect: (f64, f64, f64, f64), op: SelectionOp) {
        let (w, h) = (project.doc.width, project.doc.height);
        let mask = match self.shape {
            MarqueeShape::Rectangle => raster::rect_mask((rect.0 as i64, rect.1 as i64, rect.2 as i64, rect.3 as i64), w, h),
            MarqueeShape::Ellipse => raster::ellipse_mask(rect, w, h, self.anti_alias),
        };
        selection::apply_shape(project, self.edit_name(), mask, self.feather, op);
    }
}

impl Tool for MarqueeTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Marquee
    }
    fn name(&self) -> &'static str {
        "Marquee"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::SELECTION
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::M)
    }

    fn options_ui(&mut self, ui: &mut egui::Ui, _ctx: &mut ToolCtx) {
        selection::mode_buttons(ui, &mut self.mode);
        ui.separator();
        use crate::ui::icons;
        ui.selectable_value(&mut self.shape, MarqueeShape::Rectangle, egui::RichText::new(icons::SQUARE).size(16.0))
            .on_hover_text("Rectangle");
        ui.selectable_value(&mut self.shape, MarqueeShape::Ellipse, egui::RichText::new(icons::CIRCLE).size(16.0))
            .on_hover_text("Ellipse");
        ui.separator();
        ui.label("Feather");
        ui.add(egui::DragValue::new(&mut self.feather).range(0.0..=250.0).speed(0.2).max_decimals(1).suffix(" px"));
        ui.add_enabled(self.shape == MarqueeShape::Ellipse, egui::Checkbox::new(&mut self.anti_alias, "Anti-alias"));
        ui.separator();
        egui::ComboBox::from_label("Style")
            .selected_text(match self.style {
                MarqueeStyle::Normal => "Normal",
                MarqueeStyle::FixedRatio => "Fixed Ratio",
                MarqueeStyle::FixedSize => "Fixed Size",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.style, MarqueeStyle::Normal, "Normal");
                ui.selectable_value(&mut self.style, MarqueeStyle::FixedRatio, "Fixed Ratio");
                ui.selectable_value(&mut self.style, MarqueeStyle::FixedSize, "Fixed Size");
            });
        match self.style {
            MarqueeStyle::FixedRatio => {
                ui.label("W");
                ui.add(egui::DragValue::new(&mut self.ratio.0).range(0.01..=1000.0).speed(0.05).max_decimals(3));
                if ui.small_button(icons::ARROWS_LEFT_RIGHT).on_hover_text("Swap width and height").clicked() {
                    self.ratio = (self.ratio.1, self.ratio.0);
                }
                ui.label("H");
                ui.add(egui::DragValue::new(&mut self.ratio.1).range(0.01..=1000.0).speed(0.05).max_decimals(3));
            }
            MarqueeStyle::FixedSize => {
                ui.label("W");
                ui.add(egui::DragValue::new(&mut self.size.0).range(1.0..=30_000.0).suffix(" px"));
                if ui.small_button(icons::ARROWS_LEFT_RIGHT).on_hover_text("Swap width and height").clicked() {
                    self.size = (self.size.1, self.size.0);
                }
                ui.label("H");
                ui.add(egui::DragValue::new(&mut self.size.1).range(1.0..=30_000.0).suffix(" px"));
            }
            MarqueeStyle::Normal => {}
        }
    }

    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        match event.phase {
            PointerPhase::Press => {
                if OutlineMove::starts_at(ctx.project, event.pos, event.modifiers, self.mode) {
                    self.drag = OutlineMove::new(ctx.project).map(Drag::Outline);
                    return;
                }
                let has_selection = ctx.project.doc.selection.is_some();
                let op = selection::op_for(self.mode, event.modifiers, has_selection);
                let rect = (self.style == MarqueeStyle::FixedSize).then(|| self.rect_for(event.pos, event.pos, false, event.modifiers.alt && !has_selection));
                self.drag = Some(Drag::Draw {
                    op,
                    shift_latched: has_selection && event.modifiers.shift,
                    alt_latched: has_selection && event.modifiers.alt,
                    rect,
                });
            }
            PointerPhase::Drag => {
                if let Some(Drag::Outline(outline)) = &mut self.drag {
                    outline.drag(ctx.project, event.press_origin, event.pos, event.modifiers.shift);
                    return;
                }
                let Some(Drag::Draw { shift_latched, alt_latched, .. }) = &mut self.drag else { return };
                if !event.modifiers.shift {
                    *shift_latched = false;
                }
                if !event.modifiers.alt {
                    *alt_latched = false;
                }
                let square = event.modifiers.shift && !*shift_latched;
                let center = event.modifiers.alt && !*alt_latched;
                let new_rect = self.rect_for(event.press_origin, event.pos, square, center);
                if let Some(Drag::Draw { rect, .. }) = &mut self.drag {
                    *rect = Some(new_rect);
                }
                *ctx.status = Some(format!("W {:.0}  H {:.0}", new_rect.2 - new_rect.0, new_rect.3 - new_rect.1));
            }
            PointerPhase::Release => match self.drag.take() {
                Some(Drag::Outline(outline)) => {
                    if !outline.finish(ctx.project) {
                        // A click inside the selection deselects, as anywhere else.
                        selection::deselect(ctx.project);
                    }
                }
                Some(Drag::Draw { op, rect, .. }) => match rect {
                    Some(r) if r.2 - r.0 >= 1.0 && r.3 - r.1 >= 1.0 => self.finish_shape(ctx.project, r, op),
                    _ if op == SelectionOp::Replace => selection::deselect(ctx.project),
                    _ => {}
                },
                None => {}
            },
            _ => {}
        }
    }

    fn cancel(&mut self, ctx: &mut ToolCtx) {
        if let Some(Drag::Outline(outline)) = self.drag.take() {
            outline.finish(ctx.project);
        }
    }

    fn overlay(&self, painter: &Painter, view: &ViewTransform, _project: &Project, _hover: Option<(f64, f64)>) {
        let Some(Drag::Draw { rect: Some(r), .. }) = &self.drag else { return };
        let points: Vec<egui::Pos2> = match self.shape {
            MarqueeShape::Rectangle => {
                vec![view.to_screen(r.0, r.1), view.to_screen(r.2, r.1), view.to_screen(r.2, r.3), view.to_screen(r.0, r.3)]
            }
            MarqueeShape::Ellipse => {
                let (cx, cy) = ((r.0 + r.2) / 2.0, (r.1 + r.3) / 2.0);
                let (rx, ry) = ((r.2 - r.0) / 2.0, (r.3 - r.1) / 2.0);
                let n = 96;
                (0..n)
                    .map(|i| {
                        let t = i as f64 / n as f64 * std::f64::consts::TAU;
                        view.to_screen(cx + rx * t.cos(), cy + ry * t.sin())
                    })
                    .collect()
            }
        };
        selection::draw_dashed(painter, &points, true);
    }

    fn cursor(&self, project: &Project, hover: (f64, f64), modifiers: Modifiers) -> CursorIcon {
        if self.drag.is_none() && OutlineMove::starts_at(project, hover, modifiers, self.mode) {
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
    fn drag_boxes() {
        assert_eq!(drag_box((10.0, 10.0), (4.0, 20.0), None, false), (4.0, 10.0, 10.0, 20.0));
        // Square: the longer side wins.
        assert_eq!(drag_box((10.0, 10.0), (14.0, 20.0), Some(1.0), false), (10.0, 10.0, 20.0, 20.0));
        assert_eq!(drag_box((10.0, 10.0), (4.0, 12.0), Some(1.0), true), (4.0, 4.0, 16.0, 16.0));
        // 2:1
        assert_eq!(drag_box((0.0, 0.0), (10.0, 10.0), Some(2.0), false), (0.0, 0.0, 20.0, 10.0));
    }
}
