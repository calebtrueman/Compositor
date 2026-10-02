//! Move tool with transform controls: drag inside to move, drag a handle to scale (Shift keeps
//! proportions), drag just outside a corner to rotate (Shift snaps to 15°). Ctrl-click selects
//! the layer under the pointer. Arrow keys nudge. With a selection, dragging inside it moves the
//! selected pixels of the active layer (Alt-drag moves a copy).

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Color32, CursorIcon, Key, Modifiers, Painter, Stroke};
use image::{GrayImage, RgbaImage};
use rayon::prelude::*;

use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::doc::{Affine, Document, Id, Layer, LayerTransform, MaskPixels, Selection};
use crate::project::{EditTarget, Project};
use crate::render::{self, Region, RenderCache};
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
    pixels: Option<PixelMove>,
    pub show_controls: bool,
    pub auto_select: bool,
}

/// Selected pixels being dragged. Nothing changes until the pointer first moves; then the
/// pixels are lifted out of the layer into a floating image, shown meanwhile by a temporary
/// layer above it, and composited back on release. One undo step.
struct PixelMove {
    layer: Id,
    duplicate: bool,
    selection: Selection,
    offset: (i64, i64),
    lifted: Option<Lifted>,
    /// Nothing visible was selected on the layer, so the drag does nothing.
    empty: bool,
}

struct Lifted {
    floating: Arc<RgbaImage>,
    /// Document position of the floating image's top-left pixel before the move.
    origin: (i64, i64),
    preview: Id,
}

/// The active layer, when a press at `pos` should move its selected pixels: inside the selection,
/// on a plain pixel layer's image (a scaled or turned layer is fine unless it has a mask).
fn pixel_move_layer(project: &Project, pos: (f64, f64)) -> Option<Id> {
    if project.target != EditTarget::Image {
        return None;
    }
    let selection = project.doc.selection.as_ref()?;
    if selection.coverage(pos.0.floor() as i64, pos.1.floor() as i64) < 128 {
        return None;
    }
    let layer = project.doc.active_layer()?;
    let image = layer.image.as_ref()?;
    let plain = !layer.is_group && layer.adjustment.is_none() && layer.text.is_none() && layer.shape.is_none();
    let placeable = layer.transform.is_pixel_aligned(image.width(), image.height()) || layer.mask.is_none();
    (plain && placeable).then_some(layer.id)
}

/// Lifts the selected pixels of `pm.layer` into a floating image (cutting them from the layer
/// unless duplicating) and starts the undo step. Changes nothing and returns `None` when no
/// visible pixels are selected there.
fn lift(project: &mut Project, pm: &PixelMove, cache: &RenderCache) -> Option<Lifted> {
    let layer = project.doc.layer(pm.layer)?;
    let image = layer.image.clone()?;
    // Work in whole document pixels: a scaled or turned layer is first redrawn at canvas resolution.
    let (mut image, transform) = if layer.transform.is_pixel_aligned(image.width(), image.height()) {
        (image, layer.transform)
    } else {
        let b = layer.transform.bounds();
        let (x0, y0, x1, y1) = (b.0.floor(), b.1.floor(), b.2.ceil(), b.3.ceil());
        let region = Region { x: x0 as i64, y: y0 as i64, width: (x1 - x0) as usize, height: (y1 - y0) as usize, scale: 1.0 };
        let placed = render::place_layer(layer, region, cache)?.to_rgba8();
        let mut t = LayerTransform::rect(x0, y0, x1 - x0, y1 - y0);
        t.sampling = layer.transform.sampling;
        (Arc::new(placed), t)
    };
    let (ox, oy) = (transform.origin[0] as i64, transform.origin[1] as i64);
    let (w, h) = (image.width() as i64, image.height() as i64);
    let sb = pm.selection.bounds;
    let (rx0, ry0) = ((sb.0 as i64).max(ox), (sb.1 as i64).max(oy));
    let (rx1, ry1) = ((sb.2 as i64).min(ox + w), (sb.3 as i64).min(oy + h));
    if rx0 >= rx1 || ry0 >= ry1 {
        return None;
    }
    let (rw, rh) = ((rx1 - rx0) as usize, (ry1 - ry0) as usize);
    let selection = &pm.selection;
    let mut floating = RgbaImage::new(rw as u32, rh as u32);
    floating.par_chunks_mut(rw * 4).enumerate().for_each(|(j, row)| {
        let y = ry0 + j as i64;
        for (i, out) in row.chunks_exact_mut(4).enumerate() {
            let x = rx0 + i as i64;
            let p = image.get_pixel((x - ox) as u32, (y - oy) as u32);
            let c = selection.coverage(x, y) as u32;
            out.copy_from_slice(&[p[0], p[1], p[2], ((p[3] as u32 * c + 127) / 255) as u8]);
        }
    });
    if floating.pixels().all(|p| p[3] == 0) {
        return None;
    }
    if !pm.duplicate {
        let pixels = Arc::make_mut(&mut image);
        let stride = w as usize * 4;
        pixels.par_chunks_mut(stride).enumerate().skip((ry0 - oy) as usize).take(rh).for_each(|(py, row)| {
            let y = oy + py as i64;
            for x in rx0..rx1 {
                let c = selection.coverage(x, y) as u32;
                let a = &mut row[(x - ox) as usize * 4 + 3];
                *a = ((*a as u32 * (255 - c) + 127) / 255) as u8;
            }
        });
    }
    project.begin_edit(if pm.duplicate { "Duplicate Pixels" } else { "Move Pixels" });
    let layer = project.doc.layer_mut(pm.layer)?;
    layer.image = Some(image);
    layer.transform = transform;
    layer.rasterized();
    let floating = Arc::new(floating);
    let mut preview = Layer::blank("Moving Pixels", LayerTransform::rect(rx0 as f64, ry0 as f64, rw as f64, rh as f64));
    preview.image = Some(floating.clone());
    preview.parent = layer.parent;
    preview.opacity = layer.opacity;
    preview.blend = layer.blend;
    preview.clip_source = layer.clip_source;
    let preview_id = preview.id;
    let index = project.doc.index_of(pm.layer)?;
    project.doc.layers.insert(index + 1, preview);
    Some(Lifted { floating, origin: (rx0, ry0), preview: preview_id })
}

/// Composites straight-alpha `floating` over a pixel-aligned layer with its top-left pixel at
/// document position `at`, growing the layer's image to hold it.
pub fn paste_into(layer: &mut Layer, floating: &RgbaImage, at: (i64, i64)) {
    let Some(image) = layer.image.as_mut() else { return };
    let (ox, oy) = (layer.transform.origin[0].round() as i64, layer.transform.origin[1].round() as i64);
    let (w, h) = (image.width() as i64, image.height() as i64);
    let (fw, fh) = (floating.width() as i64, floating.height() as i64);
    let (nx0, ny0) = (ox.min(at.0), oy.min(at.1));
    let (nx1, ny1) = ((ox + w).max(at.0 + fw), (oy + h).max(at.1 + fh));
    if (nx0, ny0, nx1, ny1) != (ox, oy, ox + w, oy + h) {
        let mut grown = RgbaImage::new((nx1 - nx0) as u32, (ny1 - ny0) as u32);
        image::imageops::replace(&mut grown, &**image, ox - nx0, oy - ny0);
        *image = Arc::new(grown);
        let old = layer.transform;
        layer.transform.origin = [nx0 as f64, ny0 as f64];
        layer.transform.size = [(nx1 - nx0) as f64, (ny1 - ny0) as f64];
        if let Some(mask) = layer.mask.as_mut().filter(|m| m.linked) {
            match &mask.pixels {
                // A mask the image's size grows with it, extended by its corner value.
                MaskPixels::Pixels(gray) if gray.dimensions() == (w as u32, h as u32) => {
                    let fill = gray.get_pixel(0, 0)[0];
                    let mut grown = GrayImage::from_pixel((nx1 - nx0) as u32, (ny1 - ny0) as u32, image::Luma([fill]));
                    image::imageops::replace(&mut grown, &**gray, ox - nx0, oy - ny0);
                    mask.pixels = MaskPixels::Pixels(Arc::new(grown));
                }
                // Any other mask stays where it was.
                MaskPixels::Pixels(_) => {
                    mask.linked = false;
                    mask.placement = Some(old);
                }
                MaskPixels::Uniform(_) => {}
            }
        }
    }
    let pixels = Arc::make_mut(image);
    let stride = pixels.width() as usize * 4;
    let (dx, dy) = ((at.0 - nx0) as usize, (at.1 - ny0) as usize);
    pixels.par_chunks_mut(stride).skip(dy).take(fh as usize).enumerate().for_each(|(j, row)| {
        for i in 0..fw as usize {
            let src = floating.get_pixel(i as u32, j as u32);
            let sa = src[3] as f32 / 255.0;
            if sa <= 0.0 {
                continue;
            }
            let dst = &mut row[(dx + i) * 4..(dx + i) * 4 + 4];
            let da = dst[3] as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            for c in 0..3 {
                let v = (src[c] as f32 * sa + dst[c] as f32 * da * (1.0 - sa)) / oa;
                dst[c] = v.round().clamp(0.0, 255.0) as u8;
            }
            dst[3] = (oa * 255.0).round() as u8;
        }
    });
    layer.rasterized();
}

impl PixelMove {
    fn drag(&mut self, project: &mut Project, offset: (i64, i64), cache: &RenderCache) {
        if offset == self.offset {
            return;
        }
        if self.lifted.is_none() {
            if self.empty {
                return;
            }
            self.lifted = lift(project, self, cache);
            if self.lifted.is_none() {
                self.empty = true;
                return;
            }
        }
        self.offset = offset;
        let Some(lifted) = &self.lifted else { return };
        if let Some(preview) = project.doc.layer_mut(lifted.preview) {
            preview.transform.origin = [(lifted.origin.0 + offset.0) as f64, (lifted.origin.1 + offset.1) as f64];
        }
        project.doc.selection = crate::selection::translated(&self.selection, offset.0, offset.1);
        project.invalidate_all();
    }

    fn finish(self, project: &mut Project) {
        let Some(lifted) = self.lifted else { return };
        project.doc.layers.retain(|l| l.id != lifted.preview);
        let at = (lifted.origin.0 + self.offset.0, lifted.origin.1 + self.offset.1);
        if let Some(layer) = project.doc.layer_mut(self.layer) {
            paste_into(layer, &lifted.floating, at);
        }
        project.finish_edit();
        project.invalidate_all();
    }
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
            // Shapes redraw crisply at their new size, within the same undo step.
            crate::tools::shape::redraw_scaled(&mut project.doc);
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
                let on_handle = self.show_controls
                    && frame_for(&ctx.project.doc, &move_targets(&ctx.project.doc))
                        .is_some_and(|f| matches!(self.hit(&f, event.pos, ctx.zoom), Some(Drag::Scale(_) | Drag::Rotate)));
                if !event.modifiers.command && !on_handle {
                    if let Some(layer) = pixel_move_layer(ctx.project, event.pos) {
                        let selection = ctx.project.doc.selection.clone().unwrap();
                        self.pixels = Some(PixelMove { layer, duplicate: event.modifiers.alt, selection, offset: (0, 0), lifted: None, empty: false });
                        return;
                    }
                }
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
                if let Some(pixels) = &mut self.pixels {
                    let (mut dx, mut dy) =
                        ((event.pos.0 - event.press_origin.0).round() as i64, (event.pos.1 - event.press_origin.1).round() as i64);
                    if event.modifiers.shift {
                        if dx.abs() > dy.abs() {
                            dy = 0;
                        } else {
                            dx = 0;
                        }
                    }
                    pixels.drag(ctx.project, (dx, dy), ctx.cache);
                    return;
                }
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
            PointerPhase::Release => {
                if let Some(pixels) = self.pixels.take() {
                    pixels.finish(ctx.project);
                }
                self.finish(ctx.project);
            }
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

    fn commit(&mut self, ctx: &mut ToolCtx) {
        if let Some(pixels) = self.pixels.take() {
            pixels.finish(ctx.project);
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    fn project_with_selection() -> Project {
        let mut doc = Document::new(10, 10, Some([255, 0, 0, 255]));
        let mask = GrayImage::from_fn(10, 10, |x, y| Luma([if (2..4).contains(&x) && (2..4).contains(&y) { 255 } else { 0 }]));
        doc.selection = Selection::from_mask(mask);
        Project::new(doc, None, "Test".into())
    }

    fn drag_pixels(project: &mut Project, duplicate: bool, offset: (i64, i64)) {
        let cache = RenderCache::default();
        let layer = pixel_move_layer(project, (2.5, 2.5)).expect("inside the selection");
        let selection = project.doc.selection.clone().unwrap();
        let mut pm = PixelMove { layer, duplicate, selection, offset: (0, 0), lifted: None, empty: false };
        pm.drag(project, offset, &cache);
        assert_eq!(project.doc.layers.len(), 2, "a preview layer while dragging");
        pm.finish(project);
    }

    #[test]
    fn moves_selected_pixels() {
        let mut project = project_with_selection();
        drag_pixels(&mut project, false, (5, 0));
        assert_eq!(project.doc.layers.len(), 1);
        let image = project.doc.layers[0].image.as_ref().unwrap();
        assert_eq!(image.get_pixel(2, 2)[3], 0);
        assert_eq!(image.get_pixel(7, 2)[3], 255);
        assert_eq!(project.doc.selection.as_ref().unwrap().bounds, (7, 2, 9, 4));
        // One undo step puts everything back.
        project.undo();
        assert_eq!(project.doc.layers[0].image.as_ref().unwrap().get_pixel(2, 2)[3], 255);
        assert_eq!(project.doc.selection.as_ref().unwrap().bounds, (2, 2, 4, 4));
    }

    #[test]
    fn alt_drag_duplicates() {
        let mut project = project_with_selection();
        project.edit("Paint", |doc| {
            let image = Arc::make_mut(doc.layers[0].image.as_mut().unwrap());
            image.put_pixel(2, 2, image::Rgba([0, 0, 255, 255]));
        });
        drag_pixels(&mut project, true, (0, 3));
        let image = project.doc.layers[0].image.as_ref().unwrap();
        assert_eq!(*image.get_pixel(2, 2), image::Rgba([0, 0, 255, 255]));
        assert_eq!(*image.get_pixel(2, 5), image::Rgba([0, 0, 255, 255]));
    }

    #[test]
    fn pixels_moved_off_the_layer_grow_it() {
        let mut project = project_with_selection();
        drag_pixels(&mut project, false, (-5, 0));
        let layer = &project.doc.layers[0];
        assert_eq!(layer.transform.origin, [-3.0, 0.0]);
        assert_eq!(layer.image.as_ref().unwrap().dimensions(), (13, 10));
        assert_eq!(layer.image.as_ref().unwrap().get_pixel(0, 2)[3], 255);
    }
}
