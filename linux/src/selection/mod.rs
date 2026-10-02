//! Selections beyond All/Deselect/Inverse: the pieces the selection tools share (how modifier
//! keys combine a new shape with the current selection, rasterizing shapes, the Magic Wand) and
//! the Select menu's Reselect, Modify, Grow, Similar and Color Range.

pub mod color_range;
pub mod modify;
pub mod raster;
pub mod wand;

use egui::{Color32, Modifiers, Painter, Pos2, Stroke};
use image::GrayImage;

use crate::doc::selection::{Selection, SelectionOp};
use crate::project::Project;
use crate::ui::dialogs::Dialog;

/// How a selection tool's new shape combines with the current selection: Shift adds, Alt
/// subtracts, Shift+Alt intersects, as in Photoshop. With nothing selected the modifiers are
/// free to constrain the shape instead, and `base` (the options bar's mode) applies.
pub fn op_for(base: SelectionOp, modifiers: Modifiers, has_selection: bool) -> SelectionOp {
    if !has_selection {
        return base;
    }
    match (modifiers.shift, modifiers.alt) {
        (true, true) => SelectionOp::Intersect,
        (true, false) => SelectionOp::Add,
        (false, true) => SelectionOp::Subtract,
        _ => base,
    }
}

/// Combines a canvas-sized `shape` into the selection as one undo step named `name`.
pub fn apply(project: &mut Project, name: &str, shape: GrayImage, op: SelectionOp) {
    let result = Selection::combine(project.doc.selection.as_ref(), shape, op);
    if result.is_none() && project.doc.selection.is_none() {
        return;
    }
    project.edit(name, |doc| doc.selection = result);
}

/// A selection tool's finished shape: feathered by `feather` pixels when set, then combined.
pub fn apply_shape(project: &mut Project, name: &str, mut shape: GrayImage, feather: f64, op: SelectionOp) {
    if feather > 0.0 {
        shape = modify::feather(&shape, feather);
    }
    apply(project, name, shape, op);
}

/// Clears the selection as one undo step, if there is one.
pub fn deselect(project: &mut Project) {
    if project.doc.selection.is_some() {
        project.edit("Deselect", |doc| doc.selection = None);
    }
}

/// `selection` moved by whole pixels; whatever leaves the canvas is dropped.
pub fn translated(selection: &Selection, dx: i64, dy: i64) -> Option<Selection> {
    let (w, h) = selection.mask.dimensions();
    let (x0, y0, x1, y1) = selection.bounds;
    let mut out = GrayImage::new(w, h);
    let tx0 = (x0 as i64 + dx).max(0);
    let tx1 = (x1 as i64 + dx).min(w as i64);
    let ty0 = (y0 as i64 + dy).max(0);
    let ty1 = (y1 as i64 + dy).min(h as i64);
    if tx0 >= tx1 || ty0 >= ty1 {
        return None;
    }
    let src = selection.mask.as_raw();
    let dst: &mut [u8] = &mut out;
    let len = (tx1 - tx0) as usize;
    for ty in ty0..ty1 {
        let sy = ty - dy;
        let sx = tx0 - dx;
        let s = (sy as usize) * w as usize + sx as usize;
        let d = (ty as usize) * w as usize + tx0 as usize;
        dst[d..d + len].copy_from_slice(&src[s..s + len]);
    }
    Selection::from_mask(out)
}

/// A selection tool dragging the selection's outline (not its pixels), as Photoshop's do when
/// a drag starts inside the selection with no modifiers in New mode. The undo step starts with
/// the first real movement, so a plain click leaves no trace.
pub struct OutlineMove {
    original: Selection,
    offset: (i64, i64),
    started: bool,
}

impl OutlineMove {
    /// Whether a press at `pos` should move the outline.
    pub fn starts_at(project: &Project, pos: (f64, f64), modifiers: Modifiers, mode: SelectionOp) -> bool {
        mode == SelectionOp::Replace
            && !modifiers.shift
            && !modifiers.alt
            && !modifiers.command
            && project.doc.selection.as_ref().is_some_and(|s| s.coverage(pos.0.floor() as i64, pos.1.floor() as i64) >= 128)
    }

    pub fn new(project: &Project) -> Option<Self> {
        Some(OutlineMove { original: project.doc.selection.clone()?, offset: (0, 0), started: false })
    }

    /// Follows the pointer from `origin` to `pos` in whole pixels; Shift keeps to one axis.
    pub fn drag(&mut self, project: &mut Project, origin: (f64, f64), pos: (f64, f64), shift: bool) {
        let (mut dx, mut dy) = ((pos.0 - origin.0).round() as i64, (pos.1 - origin.1).round() as i64);
        if shift {
            if dx.abs() > dy.abs() {
                dy = 0;
            } else {
                dx = 0;
            }
        }
        if (dx, dy) == self.offset {
            return;
        }
        if !self.started {
            project.begin_edit("Move Selection");
            self.started = true;
        }
        self.offset = (dx, dy);
        project.doc.selection = translated(&self.original, dx, dy);
    }

    /// Whether the outline actually moved (otherwise the press was a click).
    pub fn finish(self, project: &mut Project) -> bool {
        if self.started {
            project.finish_edit();
        }
        self.started
    }
}

/// New / Add / Subtract / Intersect buttons for a selection tool's options bar.
pub fn mode_buttons(ui: &mut egui::Ui, op: &mut SelectionOp) {
    use crate::ui::icons;
    for (value, icon, hint) in [
        (SelectionOp::Replace, icons::SQUARE, "New selection"),
        (SelectionOp::Add, icons::UNITE_SQUARE, "Add to selection (Shift)"),
        (SelectionOp::Subtract, icons::SUBTRACT_SQUARE, "Subtract from selection (Alt)"),
        (SelectionOp::Intersect, icons::INTERSECT_SQUARE, "Intersect with selection (Shift+Alt)"),
    ] {
        if ui.selectable_label(*op == value, egui::RichText::new(icon).size(16.0)).on_hover_text(hint).clicked() {
            *op = value;
        }
    }
}

/// Draws a path as marching-ants style dashes (black over white), closed or open.
pub fn draw_dashed(painter: &Painter, points: &[Pos2], closed: bool) {
    if points.len() < 2 {
        return;
    }
    let mut path = points.to_vec();
    if closed {
        path.push(points[0]);
    }
    painter.add(egui::Shape::line(path.clone(), Stroke::new(1.0, Color32::WHITE)));
    painter.extend(egui::Shape::dashed_line(&path, Stroke::new(1.0, Color32::BLACK), 4.0, 4.0));
}

/// The Select menu's commands after Load Layer as Selection.
pub fn select_menu_items(ui: &mut egui::Ui, project: &mut Project, dialog: &mut Option<Box<dyn Dialog>>) {
    let has_selection = project.doc.selection.is_some();
    let can_reselect = !has_selection
        && project.last_selection.as_ref().is_some_and(|s| s.mask.dimensions() == (project.doc.width, project.doc.height));
    if menu_item(ui, can_reselect, "Reselect", "Ctrl+Shift+D") {
        let selection = project.last_selection.clone();
        project.edit("Reselect", |doc| doc.selection = selection);
    }
    ui.separator();
    if menu_item(ui, true, "Color Range…", "") {
        *dialog = Some(Box::new(color_range::ColorRangeDialog::new(project)));
    }
    ui.menu_button("Modify", |ui| {
        for op in modify::ModifyOp::ALL {
            if menu_item(ui, has_selection, &format!("{}…", op.name()), "") {
                *dialog = Some(Box::new(modify::ModifyDialog::new(op)));
            }
        }
    });
    if menu_item(ui, has_selection, "Grow", "") {
        grow_or_similar(project, true);
    }
    if menu_item(ui, has_selection, "Similar", "") {
        grow_or_similar(project, false);
    }
}

fn menu_item(ui: &mut egui::Ui, enabled: bool, label: &str, shortcut: &str) -> bool {
    let mut button = egui::Button::new(label);
    if !shortcut.is_empty() {
        button = button.shortcut_text(shortcut);
    }
    let clicked = ui.add_enabled(enabled, button).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// Select > Grow (adjacent pixels) or Similar (anywhere), with the Magic Wand's settings.
pub fn grow_or_similar(project: &mut Project, contiguous: bool) {
    let Some(selection) = project.doc.selection.clone() else { return };
    let settings = wand::settings();
    let cache = crate::render::RenderCache::default();
    let Some(sample) = wand::sample_image(&project.doc, settings.sample_all_layers, &cache) else { return };
    let mut shape = wand::extend_similar(&sample, &selection.mask, settings.tolerance as i32, contiguous);
    if settings.anti_alias {
        modify::soften_edges(&mut shape);
    }
    let name = if contiguous { "Grow" } else { "Similar" };
    apply(project, name, shape, SelectionOp::Add);
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    fn rect(w: u32, h: u32, r: (u32, u32, u32, u32)) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| Luma([if x >= r.0 && x < r.2 && y >= r.1 && y < r.3 { 255 } else { 0 }]))
    }

    #[test]
    fn combine_ops() {
        let a = Selection::from_mask(rect(10, 10, (0, 0, 6, 6)));
        let b = rect(10, 10, (4, 4, 10, 10));
        let add = Selection::combine(a.as_ref(), b.clone(), SelectionOp::Add).unwrap();
        assert_eq!(add.bounds, (0, 0, 10, 10));
        let sub = Selection::combine(a.as_ref(), b.clone(), SelectionOp::Subtract).unwrap();
        assert_eq!(sub.bounds, (0, 0, 6, 6));
        assert_eq!(sub.coverage(5, 5), 0);
        assert_eq!(sub.coverage(3, 3), 255);
        let both = Selection::combine(a.as_ref(), b.clone(), SelectionOp::Intersect).unwrap();
        assert_eq!(both.bounds, (4, 4, 6, 6));
        let replaced = Selection::combine(a.as_ref(), b.clone(), SelectionOp::Replace).unwrap();
        assert_eq!(replaced.bounds, (4, 4, 10, 10));
        // Nothing selected: subtracting and intersecting give nothing.
        assert!(Selection::combine(None, b.clone(), SelectionOp::Subtract).is_none());
        assert!(Selection::combine(None, b, SelectionOp::Intersect).is_none());
    }

    #[test]
    fn modifiers_pick_ops() {
        let shift = Modifiers::SHIFT;
        let alt = Modifiers::ALT;
        assert_eq!(op_for(SelectionOp::Replace, shift, true), SelectionOp::Add);
        assert_eq!(op_for(SelectionOp::Replace, alt, true), SelectionOp::Subtract);
        assert_eq!(op_for(SelectionOp::Replace, shift | alt, true), SelectionOp::Intersect);
        assert_eq!(op_for(SelectionOp::Subtract, Modifiers::NONE, true), SelectionOp::Subtract);
        // Without a selection Shift and Alt shape the marquee instead.
        assert_eq!(op_for(SelectionOp::Replace, shift, false), SelectionOp::Replace);
    }

    #[test]
    fn translate_clips_to_canvas() {
        let a = Selection::from_mask(rect(10, 10, (2, 2, 5, 5))).unwrap();
        let moved = translated(&a, 3, -1).unwrap();
        assert_eq!(moved.bounds, (5, 1, 8, 4));
        let off = translated(&a, 7, 0).unwrap();
        assert_eq!(off.bounds, (9, 2, 10, 5));
        assert!(translated(&a, 20, 0).is_none());
    }
}
