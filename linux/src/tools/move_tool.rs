//! Move tool with transform controls: drag inside to move, drag a handle to scale (Shift keeps
//! proportions), drag just outside a corner to rotate (Shift snaps to 15°). Ctrl-click selects
//! the layer under the pointer. Arrow keys nudge.

use std::collections::HashMap;

use egui::{Color32, CursorIcon, Key, Modifiers, Painter, Stroke};

use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::doc::{Affine, Document, Id, LayerTransform};
use crate::project::Project;
use crate::ui::canvas::ViewTransform;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    Move,
    /// Handle index 0…7 clockwise from the top-left corner.
    Scale(usize),
    Rotate,
}

/// The box transform handles act on: a (possibly rotated) rectangle in document space.
#[derive(Clone, Copy, Debug)]
struct Frame {
    transform: LayerTransform,
}

impl Frame {
    /// Handle positions in unit coordinates, clockwise from the top-left corner.
    const HANDLES: [(f64, f64); 8] =
        [(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (1.0, 0.5), (1.0, 1.0), (0.5, 1.0), (0.0, 1.0), (0.0, 0.5)];

    fn handle(&self, index: usize) -> (f64, f64) {
        let (u, v) = Self::HANDLES[index];
        self.transform.point(u, v)
    }
}

#[derive(Default)]
pub struct MoveTool {
    drag: Option<Drag>,
    start: HashMap<Id, LayerTransform>,
    start_frame: Option<Frame>,
    pub show_controls: bool,
    pub auto_select: bool,
}

/// Layers a move acts on: the selected ones, with folders standing for everything inside them.
pub fn move_targets(doc: &Document) -> Vec<Id> {
    let mut out = Vec::new();
    for id in doc.selected_ids() {
        for member in doc.subtree(id) {
            if let Some(layer) = doc.layer(member) {
                if !layer.is_group && !out.contains(&member) {
                    out.push(member);
                }
            }
        }
    }
    out
}

fn frame_for(doc: &Document, targets: &[Id]) -> Option<Frame> {
    if targets.len() == 1 {
        let layer = doc.layer(targets[0])?;
        return Some(Frame { transform: layer.transform });
    }
    let mut b = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for id in targets {
        let layer = doc.layer(*id)?;
        if layer.adjustment.is_some() {
            continue;
        }
        let lb = layer.transform.bounds();
        b = (b.0.min(lb.0), b.1.min(lb.1), b.2.max(lb.2), b.3.max(lb.3));
    }
    (b.0 < b.2).then(|| Frame { transform: LayerTransform::rect(b.0, b.1, b.2 - b.0, b.3 - b.1) })
}

impl MoveTool {
    fn hit(&self, frame: &Frame, pos: (f64, f64), zoom: f32) -> Option<Drag> {
        let radius = 7.0 / zoom as f64;
        for i in 0..8 {
            let (hx, hy) = frame.handle(i);
            if (hx - pos.0).hypot(hy - pos.1) <= radius {
                return Some(Drag::Scale(i));
            }
        }
        if frame.transform.contains(pos.0, pos.1) {
            return Some(Drag::Move);
        }
        for i in [0, 2, 4, 6] {
            let (hx, hy) = frame.handle(i);
            if (hx - pos.0).hypot(hy - pos.1) <= radius * 4.0 {
                return Some(Drag::Rotate);
            }
        }
        None
    }

    fn apply(&self, doc: &mut Document, map: &Affine) {
        for (id, start) in &self.start {
            let Some(layer) = doc.layer_mut(*id) else { continue };
            let placed = start.unit_to_document().then(map);
            let mut t = LayerTransform::from_affine(&placed, start.rotation, start.flip_x);
            t.sampling = start.sampling;
            layer.transform = t;
        }
    }

    fn finish(&mut self, project: &mut Project) {
        if self.drag.take().is_some() {
            for id in self.start.keys() {
                if let Some(layer) = project.doc.layer_mut(*id) {
                    let rounded = layer.transform.rounded();
                    // Only snap when the drag didn't make the layer fractional on purpose.
                    if (rounded.size[0] - layer.transform.size[0]).abs() < 0.5 {
                        layer.transform.origin = rounded.origin;
                    }
                }
            }
            project.finish_edit();
            project.invalidate_all();
        }
        self.start.clear();
    }

    pub fn nudge(project: &mut Project, dx: f64, dy: f64) {
        let targets = move_targets(&project.doc);
        if targets.is_empty() {
            return;
        }
        project.edit("Nudge", |doc| {
            for id in targets {
                if let Some(layer) = doc.layer_mut(id) {
                    layer.transform.origin[0] += dx;
                    layer.transform.origin[1] += dy;
                }
            }
        });
    }
}

fn topmost_at(doc: &Document, pos: (f64, f64), project: &Project) -> Option<Id> {
    for layer in doc.layers.iter().rev() {
        if layer.is_group || layer.adjustment.is_some() || !doc.is_effectively_visible(layer.id) {
            continue;
        }
        if let Some((x, y)) = super::layer_pixel_at(project, layer.id, pos) {
            if layer.image.as_ref().is_some_and(|i| i.get_pixel(x, y)[3] > 10) {
                return Some(layer.id);
            }
        }
    }
    None
}

impl Tool for MoveTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Move
    }
    fn name(&self) -> &'static str {
        "Move"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::ARROWS_OUT_CARDINAL
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::V)
    }

    fn set_transform_controls(&mut self, on: bool) {
        self.show_controls = on;
    }

    fn options_ui(&mut self, ui: &mut egui::Ui, _ctx: &mut ToolCtx) {
        ui.checkbox(&mut self.auto_select, "Auto-Select").on_hover_text("Click selects the layer under the pointer (or hold Ctrl)");
        ui.checkbox(&mut self.show_controls, "Show Transform Controls");
    }

    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        match event.phase {
            PointerPhase::Press => {
                if self.auto_select || event.modifiers.command {
                    let hit = topmost_at(&ctx.project.doc, event.pos, ctx.project);
                    if let Some(id) = hit {
                        if event.modifiers.shift {
                            if !ctx.project.doc.selected.contains(&id) {
                                ctx.project.doc.selected.push(id);
                            }
                        } else {
                            ctx.project.doc.selected.clear();
                            ctx.project.doc.active = Some(id);
                        }
                    }
                }
                let targets = move_targets(&ctx.project.doc);
                let Some(frame) = frame_for(&ctx.project.doc, &targets) else { return };
                let drag = if self.show_controls {
                    self.hit(&frame, event.pos, ctx.zoom).unwrap_or(Drag::Move)
                } else {
                    Drag::Move
                };
                self.start = targets
                    .iter()
                    .filter_map(|id| ctx.project.doc.layer(*id).map(|l| (*id, l.transform)))
                    .collect();
                self.start_frame = Some(frame);
                self.drag = Some(drag);
                let name = match drag {
                    Drag::Move => "Move",
                    Drag::Scale(_) => "Scale",
                    Drag::Rotate => "Rotate",
                };
                ctx.project.begin_edit(name);
            }
            PointerPhase::Drag => {
                let (Some(drag), Some(frame)) = (self.drag, self.start_frame) else { return };
                let (sx, sy) = event.press_origin;
                let (x, y) = event.pos;
                let map = match drag {
                    Drag::Move => {
                        let (mut dx, mut dy) = (x - sx, y - sy);
                        if event.modifiers.shift {
                            if dx.abs() > dy.abs() {
                                dy = 0.0
                            } else {
                                dx = 0.0
                            }
                        }
                        Affine::translate(dx, dy)
                    }
                    Drag::Rotate => {
                        let (cx, cy) = frame.transform.center();
                        let mut angle = (y - cy).atan2(x - cx) - (sy - cy).atan2(sx - cx);
                        if event.modifiers.shift {
                            let step = 15f64.to_radians();
                            let target = frame.transform.radians() + angle;
                            angle = (target / step).round() * step - frame.transform.radians();
                        }
                        let (sin, cos) = angle.sin_cos();
                        Affine::translate(-cx, -cy)
                            .then(&Affine { a: cos, b: sin, c: -sin, d: cos, tx: 0.0, ty: 0.0 })
                            .then(&Affine::translate(cx, cy))
                    }
                    Drag::Scale(handle) => {
                        let t = frame.transform;
                        let to_doc = t.unit_to_document();
                        let Some(to_unit) = to_doc.inverse() else { return };
                        let (hu, hv) = Frame::HANDLES[handle];
                        let anchor = (1.0 - hu, 1.0 - hv);
                        let (pu, pv) = to_unit.apply(x, y);
                        let mut fx = if hu == 0.5 { 1.0 } else { (pu - anchor.0) / (hu - anchor.0) };
                        let mut fy = if hv == 0.5 { 1.0 } else { (pv - anchor.1) / (hv - anchor.1) };
                        if event.modifiers.alt {
                            // Scale around the center.
                            fx = if hu == 0.5 { 1.0 } else { (pu - 0.5) / (hu - 0.5) };
                            fy = if hv == 0.5 { 1.0 } else { (pv - 0.5) / (hv - 0.5) };
                        }
                        let corner = hu != 0.5 && hv != 0.5;
                        if event.modifiers.shift && corner {
                            let f = if fx.abs() > fy.abs() { fx } else { fy };
                            fx = f.abs() * fx.signum();
                            fy = f.abs() * fy.signum();
                        }
                        let pivot = if event.modifiers.alt { (0.5, 0.5) } else { anchor };
                        // In unit space: scale about the pivot, then back to the document.
                        let unit_map = Affine::translate(-pivot.0, -pivot.1)
                            .then(&Affine::scale(fx, fy))
                            .then(&Affine::translate(pivot.0, pivot.1));
                        to_unit.then(&unit_map).then(&to_doc)
                    }
                };
                self.apply(&mut ctx.project.doc, &map);
                ctx.project.invalidate_all();
            }
            PointerPhase::Release => self.finish(ctx.project),
            _ => {}
        }
    }

    fn key(&mut self, key: Key, modifiers: Modifiers, ctx: &mut ToolCtx) -> bool {
        let step = if modifiers.shift { 10.0 } else { 1.0 };
        let (dx, dy) = match key {
            Key::ArrowLeft => (-step, 0.0),
            Key::ArrowRight => (step, 0.0),
            Key::ArrowUp => (0.0, -step),
            Key::ArrowDown => (0.0, step),
            _ => return false,
        };
        MoveTool::nudge(ctx.project, dx, dy);
        true
    }

    fn overlay(&self, painter: &Painter, view: &ViewTransform, project: &Project, _hover: Option<(f64, f64)>) {
        let targets = move_targets(&project.doc);
        let Some(frame) = frame_for(&project.doc, &targets) else { return };
        let corners = frame.transform.corners().map(|(x, y)| view.to_screen(x, y));
        let stroke = Stroke::new(1.0, Color32::from_rgb(0, 140, 255));
        painter.add(egui::Shape::closed_line(corners.to_vec(), stroke));
        if self.show_controls {
            for i in 0..8 {
                let (x, y) = frame.handle(i);
                let p = view.to_screen(x, y);
                let r = egui::Rect::from_center_size(p, egui::vec2(8.0, 8.0));
                painter.rect_filled(r, 0.0, Color32::WHITE);
                painter.rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
            }
        }
    }

    fn cursor(&self, project: &Project, hover: (f64, f64), _modifiers: Modifiers) -> CursorIcon {
        if !self.show_controls {
            return CursorIcon::Move;
        }
        let targets = move_targets(&project.doc);
        let Some(frame) = frame_for(&project.doc, &targets) else { return CursorIcon::Default };
        match self.hit(&frame, hover, project.view.zoom) {
            Some(Drag::Scale(i)) => match i % 4 {
                0 => CursorIcon::ResizeNwSe,
                1 => CursorIcon::ResizeVertical,
                2 => CursorIcon::ResizeNeSw,
                _ => CursorIcon::ResizeHorizontal,
            },
            Some(Drag::Rotate) => CursorIcon::Alias,
            Some(Drag::Move) => CursorIcon::Move,
            None => CursorIcon::Default,
        }
    }
}

/// Free Transform: the Move tool with its handles showing.
pub fn show_controls(tools: &mut super::Tools, on: bool) {
    if let Some(tool) = tools.get_mut(ToolKind::Move) {
        tool.set_transform_controls(on);
    }
}
