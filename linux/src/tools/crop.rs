//! Crop: a frame over the canvas (the selection's bounds to begin with, if any). Drag outside it
//! to draw a new one, inside to move it, a handle to resize it; Alt keeps its center fixed. The
//! ratio presets hold its proportions. Enter or a double-click crops (the frame may reach past
//! the canvas to extend it); Escape cancels. The outside is shaded.

use egui::{Color32, CursorIcon, Key, Modifiers, Painter, Rect, Stroke};

use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::doc::ops;
use crate::project::Project;
use crate::ui::canvas::ViewTransform;

/// `(x0, y0, x1, y1)` in whole document pixels.
type Frame = (f64, f64, f64, f64);

/// Handle positions in unit coordinates, clockwise from the top-left corner.
const HANDLES: [(f64, f64); 8] = [(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (1.0, 0.5), (1.0, 1.0), (0.5, 1.0), (0.0, 1.0), (0.0, 0.5)];

/// Canvases are limited to this many pixels, as in New Document.
const MAX_PIXELS: f64 = 100_000_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropRatio {
    Free,
    Original,
    Square,
    FourThree,
    ThreeFour,
    SixteenNine,
    NineSixteen,
}

impl CropRatio {
    const ALL: [CropRatio; 7] = [
        CropRatio::Free,
        CropRatio::Original,
        CropRatio::Square,
        CropRatio::FourThree,
        CropRatio::ThreeFour,
        CropRatio::SixteenNine,
        CropRatio::NineSixteen,
    ];

    fn name(self) -> &'static str {
        match self {
            CropRatio::Free => "Free",
            CropRatio::Original => "Original",
            CropRatio::Square => "1:1",
            CropRatio::FourThree => "4:3",
            CropRatio::ThreeFour => "3:4",
            CropRatio::SixteenNine => "16:9",
            CropRatio::NineSixteen => "9:16",
        }
    }

    /// Width / height, or `None` for Free.
    fn value(self, doc_w: u32, doc_h: u32) -> Option<f64> {
        match self {
            CropRatio::Free => None,
            CropRatio::Original => Some(doc_w as f64 / doc_h.max(1) as f64),
            CropRatio::Square => Some(1.0),
            CropRatio::FourThree => Some(4.0 / 3.0),
            CropRatio::ThreeFour => Some(3.0 / 4.0),
            CropRatio::SixteenNine => Some(16.0 / 9.0),
            CropRatio::NineSixteen => Some(9.0 / 16.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragMode {
    Create,
    Move,
    Resize(usize),
}

#[derive(Clone, Copy, Debug)]
struct CropDrag {
    start: (f64, f64),
    original: Frame,
    mode: DragMode,
}

pub struct CropTool {
    pub ratio: CropRatio,
    /// The frame once the user has drawn or adjusted one.
    rect: Option<Frame>,
    drag: Option<CropDrag>,
}

impl Default for CropTool {
    fn default() -> Self {
        CropTool { ratio: CropRatio::Free, rect: None, drag: None }
    }
}

/// Whole pixels, at least one wide and tall.
fn snapped(r: Frame) -> Frame {
    let (x0, x1) = (r.0.min(r.2).round(), r.0.max(r.2).round());
    let (y0, y1) = (r.1.min(r.3).round(), r.1.max(r.3).round());
    (x0, y0, x1.max(x0 + 1.0), y1.max(y0 + 1.0))
}

fn valid(r: Frame) -> bool {
    let (w, h) = (r.2 - r.0, r.3 - r.1);
    [r.0, r.1, r.2, r.3].iter().all(|v| v.is_finite() && v.abs() <= 1_000_000.0) && w >= 1.0 && h >= 1.0 && w * h <= MAX_PIXELS
}

/// A frame dragged from `start` to `end`, or grown out from `start` as its center (`symmetric`).
fn create(start: (f64, f64), end: (f64, f64), ratio: Option<f64>, symmetric: bool) -> Frame {
    let (mut dx, mut dy) = (end.0 - start.0, end.1 - start.1);
    if let Some(r) = ratio {
        let sign = |v: f64| if v < 0.0 { -1.0 } else { 1.0 };
        if dx.abs() > dy.abs() * r {
            dy = sign(dy) * dx.abs() / r;
        } else {
            dx = sign(dx) * dy.abs() * r;
        }
    }
    if symmetric {
        snapped((start.0 - dx.abs(), start.1 - dy.abs(), start.0 + dx.abs(), start.1 + dy.abs()))
    } else {
        snapped((start.0, start.1, start.0 + dx, start.1 + dy))
    }
}

/// `original` with handle `handle` dragged by `delta`. `symmetric` moves the opposite edges too;
/// `ratio` keeps the proportions (corners grow to contain the pointer, edges stay centered).
fn resize(original: Frame, handle: usize, delta: (f64, f64), ratio: Option<f64>, symmetric: bool) -> Frame {
    let (hu, hv) = HANDLES[handle];
    let (mut x0, mut y0, mut x1, mut y1) = original;
    if hu == 0.0 {
        x0 += delta.0;
        if symmetric {
            x1 -= delta.0;
        }
    } else if hu == 1.0 {
        x1 += delta.0;
        if symmetric {
            x0 -= delta.0;
        }
    }
    if hv == 0.0 {
        y0 += delta.1;
        if symmetric {
            y1 -= delta.1;
        }
    } else if hv == 1.0 {
        y1 += delta.1;
        if symmetric {
            y0 -= delta.1;
        }
    }
    let Some(r) = ratio else { return snapped((x0, y0, x1, y1)) };
    let (cx, cy) = ((original.0 + original.2) / 2.0, (original.1 + original.3) / 2.0);
    let (mut w, mut h) = ((x1 - x0).abs().max(1.0), (y1 - y0).abs().max(1.0));
    if hu != 0.5 && hv != 0.5 {
        if w / h > r {
            h = w / r;
        } else {
            w = h * r;
        }
        if symmetric {
            return snapped((cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0));
        }
        // Grow away from the fixed corner, toward wherever the dragged one went.
        let ax = if hu == 0.0 { original.2 } else { original.0 };
        let ay = if hv == 0.0 { original.3 } else { original.1 };
        let dragged_x = if hu == 0.0 { x0 } else { x1 };
        let dragged_y = if hv == 0.0 { y0 } else { y1 };
        let (nx0, nx1) = if dragged_x >= ax { (ax, ax + w) } else { (ax - w, ax) };
        let (ny0, ny1) = if dragged_y >= ay { (ay, ay + h) } else { (ay - h, ay) };
        snapped((nx0, ny0, nx1, ny1))
    } else if hu != 0.5 {
        h = w / r;
        snapped((x0.min(x1), cy - h / 2.0, x0.max(x1), cy + h / 2.0))
    } else {
        w = h * r;
        snapped((cx - w / 2.0, y0.min(y1), cx + w / 2.0, y0.max(y1)))
    }
}

fn handle_point(r: Frame, index: usize) -> (f64, f64) {
    let (u, v) = HANDLES[index];
    (r.0 + (r.2 - r.0) * u, r.1 + (r.3 - r.1) * v)
}

impl CropTool {
    /// The frame on screen: the one drawn, else the selection's bounds, else the canvas.
    fn visible(&self, project: &Project) -> Frame {
        if let Some(r) = self.rect {
            return r;
        }
        match &project.doc.selection {
            Some(s) => (s.bounds.0 as f64, s.bounds.1 as f64, s.bounds.2 as f64, s.bounds.3 as f64),
            None => (0.0, 0.0, project.doc.width as f64, project.doc.height as f64),
        }
    }

    fn hit(&self, frame: Frame, pos: (f64, f64), zoom: f32) -> DragMode {
        let radius = 8.0 / zoom as f64;
        for i in 0..8 {
            let (hx, hy) = handle_point(frame, i);
            if (hx - pos.0).abs() <= radius && (hy - pos.1).abs() <= radius {
                return DragMode::Resize(i);
            }
        }
        if pos.0 >= frame.0 && pos.0 <= frame.2 && pos.1 >= frame.1 && pos.1 <= frame.3 {
            DragMode::Move
        } else {
            DragMode::Create
        }
    }

    fn crop(&mut self, project: &mut Project, frame: Frame) {
        self.rect = None;
        self.drag = None;
        let full = (0.0, 0.0, project.doc.width as f64, project.doc.height as f64);
        if frame == full || !valid(frame) {
            return;
        }
        let (x, y) = (frame.0 as i64, frame.1 as i64);
        let (w, h) = ((frame.2 - frame.0) as u32, (frame.3 - frame.1) as u32);
        project.edit("Crop", |doc| ops::crop_canvas(doc, x, y, w, h));
        project.view.request_fit();
    }

    /// Reshapes the frame to a newly chosen ratio, keeping its width and vertical center.
    fn apply_ratio(&mut self, project: &Project) {
        let Some(r) = self.ratio.value(project.doc.width, project.doc.height) else { return };
        let frame = self.visible(project);
        let h = (frame.2 - frame.0) / r;
        let cy = (frame.1 + frame.3) / 2.0;
        let next = snapped((frame.0, cy - h / 2.0, frame.2, cy + h / 2.0));
        if valid(next) {
            self.rect = Some(next);
        }
    }
}

impl Tool for CropTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Crop
    }
    fn name(&self) -> &'static str {
        "Crop"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::CROP
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::C)
    }

    fn options_ui(&mut self, ui: &mut egui::Ui, ctx: &mut ToolCtx) {
        let before = self.ratio;
        egui::ComboBox::from_label("Ratio").selected_text(self.ratio.name()).show_ui(ui, |ui| {
            for ratio in CropRatio::ALL {
                ui.selectable_value(&mut self.ratio, ratio, ratio.name());
            }
        });
        if self.ratio != before {
            self.apply_ratio(ctx.project);
        }
        let frame = self.visible(ctx.project);
        ui.label(format!("{:.0} × {:.0} px", frame.2 - frame.0, frame.3 - frame.1));
        ui.separator();
        if ui.button("Cancel").on_hover_text("Esc").clicked() {
            self.rect = None;
            self.drag = None;
        }
        if ui.button("Crop").on_hover_text("Enter").clicked() {
            self.crop(ctx.project, frame);
        }
    }

    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        let ratio = self.ratio.value(ctx.project.doc.width, ctx.project.doc.height);
        match event.phase {
            PointerPhase::Press => {
                let frame = self.visible(ctx.project);
                let mode = self.hit(frame, event.pos, ctx.zoom);
                self.drag = Some(CropDrag { start: event.pos, original: frame, mode });
            }
            PointerPhase::Drag => {
                let Some(drag) = self.drag else { return };
                let delta = (event.pos.0 - drag.start.0, event.pos.1 - drag.start.1);
                let next = match drag.mode {
                    DragMode::Create => create(drag.start, event.pos, ratio, event.modifiers.alt),
                    DragMode::Move => {
                        let (dx, dy) = (delta.0.round(), delta.1.round());
                        let o = drag.original;
                        (o.0 + dx, o.1 + dy, o.2 + dx, o.3 + dy)
                    }
                    DragMode::Resize(handle) => resize(drag.original, handle, delta, ratio, event.modifiers.alt),
                };
                if valid(next) {
                    self.rect = Some(next);
                    *ctx.status = Some(format!("Crop {:.0} × {:.0} px", next.2 - next.0, next.3 - next.1));
                }
            }
            PointerPhase::Release => self.drag = None,
            PointerPhase::DoubleClick => {
                let frame = self.visible(ctx.project);
                if self.hit(frame, event.pos, ctx.zoom) != DragMode::Create {
                    self.crop(ctx.project, frame);
                }
            }
            PointerPhase::Hover => {}
        }
    }

    fn commit(&mut self, ctx: &mut ToolCtx) {
        if let Some(frame) = self.rect {
            self.crop(ctx.project, frame);
        }
    }

    fn cancel(&mut self, _ctx: &mut ToolCtx) {
        self.rect = None;
        self.drag = None;
    }

    fn overlay(&self, painter: &Painter, view: &ViewTransform, project: &Project, _hover: Option<(f64, f64)>) {
        let frame = self.visible(project);
        let r = view.doc_rect(frame.0, frame.1, frame.2, frame.3);
        let clip = painter.clip_rect();
        let shade = Color32::from_black_alpha(150);
        for outside in [
            Rect::from_min_max(clip.min, egui::pos2(clip.max.x, r.min.y)),
            Rect::from_min_max(egui::pos2(clip.min.x, r.max.y), clip.max),
            Rect::from_min_max(egui::pos2(clip.min.x, r.min.y), egui::pos2(r.min.x, r.max.y)),
            Rect::from_min_max(egui::pos2(r.max.x, r.min.y), egui::pos2(clip.max.x, r.max.y)),
        ] {
            if outside.is_positive() {
                painter.rect_filled(outside, 0.0, shade);
            }
        }
        // Rule of thirds while adjusting.
        if self.drag.is_some() {
            let thin = Stroke::new(1.0, Color32::from_white_alpha(110));
            for t in [1.0 / 3.0, 2.0 / 3.0] {
                let x = r.min.x + r.width() * t;
                let y = r.min.y + r.height() * t;
                painter.line_segment([egui::pos2(x, r.min.y), egui::pos2(x, r.max.y)], thin);
                painter.line_segment([egui::pos2(r.min.x, y), egui::pos2(r.max.x, y)], thin);
            }
        }
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Middle);
        for i in 0..8 {
            let (x, y) = handle_point(frame, i);
            let h = Rect::from_center_size(view.to_screen(x, y), egui::vec2(8.0, 8.0));
            painter.rect_filled(h, 0.0, Color32::WHITE);
            painter.rect_stroke(h, 0.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Inside);
        }
    }

    fn cursor(&self, project: &Project, hover: (f64, f64), _modifiers: Modifiers) -> CursorIcon {
        match self.hit(self.visible(project), hover, project.view.zoom) {
            DragMode::Resize(i) => match i % 4 {
                0 => CursorIcon::ResizeNwSe,
                1 => CursorIcon::ResizeVertical,
                2 => CursorIcon::ResizeNeSw,
                _ => CursorIcon::ResizeHorizontal,
            },
            DragMode::Move => CursorIcon::Move,
            DragMode::Create => CursorIcon::Crosshair,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_frames() {
        assert_eq!(create((10.0, 10.0), (4.2, 30.0), None, false), (4.0, 10.0, 10.0, 30.0));
        assert_eq!(create((10.0, 10.0), (14.0, 12.0), None, true), (6.0, 8.0, 14.0, 12.0));
        assert_eq!(create((0.0, 0.0), (10.0, 1.0), Some(2.0), false), (0.0, 0.0, 10.0, 5.0));
    }

    #[test]
    fn resize_frames() {
        let r = (0.0, 0.0, 100.0, 50.0);
        // Bottom-right corner, free.
        assert_eq!(resize(r, 4, (10.0, 10.0), None, false), (0.0, 0.0, 110.0, 60.0));
        // Left edge, symmetric.
        assert_eq!(resize(r, 7, (10.0, 0.0), None, true), (10.0, 0.0, 90.0, 50.0));
        // Corner at 2:1 grows to contain the pointer.
        assert_eq!(resize(r, 4, (20.0, 30.0), Some(2.0), false), (0.0, 0.0, 160.0, 80.0));
        // Top-left corner dragged past the fixed one flips over it.
        assert_eq!(resize(r, 0, (150.0, 0.0), None, false), (100.0, 0.0, 150.0, 50.0));
        // Right edge at 1:1 keeps the vertical center.
        assert_eq!(resize(r, 3, (-40.0, 0.0), Some(1.0), false), (0.0, -5.0, 60.0, 55.0));
    }

    #[test]
    fn crop_extends_the_canvas() {
        let mut doc = crate::doc::Document::new(10, 10, Some([255, 0, 0, 255]));
        ops::crop_canvas(&mut doc, -5, -5, 20, 20);
        assert_eq!((doc.width, doc.height), (20, 20));
        assert_eq!(doc.layers[0].transform.origin, [5.0, 5.0]);
    }
}
