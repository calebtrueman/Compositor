//! Shape tool (U): drags out a rectangle (rounded by a corner radius), an ellipse or a line in the
//! foreground color, each on a new layer. The layer keeps its style in `shape`, as the macOS app does,
//! so it can be drawn again cleanly when it's scaled or restyled; its PNG is an ordinary raster.

use std::sync::Arc;

use egui::{Color32, Key, Modifiers, Painter, Stroke};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::doc::{Document, Layer, LayerTransform};
use crate::project::Project;
use crate::ui::canvas::ViewTransform;

/// Pixels one shape layer may hold.
const MAX_PIXELS: u64 = 100_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    Line,
}

impl ShapeKind {
    pub const ALL: [ShapeKind; 3] = [ShapeKind::Rectangle, ShapeKind::Ellipse, ShapeKind::Line];

    /// As the manifest spells it.
    pub fn name(self) -> &'static str {
        match self {
            ShapeKind::Rectangle => "Rectangle",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Line => "Line",
        }
    }

    pub fn from_name(name: &str) -> Option<ShapeKind> {
        ShapeKind::ALL.into_iter().find(|k| k.name() == name)
    }
}

/// What a shape layer draws (the manifest's `shape` record), kept so it can be drawn again at a
/// new size. `kind` stays a string so a kind this build doesn't know survives a save.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShapeStyle {
    pub kind: String,
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    /// Document pixels, whatever size the shape is scaled to.
    pub corner_radius: f64,
    /// A line's thickness, and its ends as fractions of the layer's box (`[x, y]`, as Core
    /// Graphics writes a point). Missing on other shapes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<[f64; 2]>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for ShapeStyle {
    fn default() -> Self {
        ShapeStyle {
            kind: ShapeKind::Rectangle.name().to_string(),
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            corner_radius: 0.0,
            line_width: None,
            start: None,
            end: None,
            extra: Map::new(),
        }
    }
}

impl ShapeStyle {
    pub fn kind(&self) -> Option<ShapeKind> {
        ShapeKind::from_name(&self.kind)
    }
}

/// The shape filling a `width`×`height` box, anti-aliased. A rectangle's corners round by
/// `cornerRadius`, at most half its shorter side; a line runs between its ends with round caps.
/// `None` for a kind this build can't draw.
pub fn rasterize(style: &ShapeStyle, width: u32, height: u32) -> Option<RgbaImage> {
    use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, Rect, Transform};
    let kind = style.kind()?;
    let mut pixmap = Pixmap::new(width.max(1), height.max(1))?;
    let (w, h) = (pixmap.width() as f32, pixmap.height() as f32);
    let channel = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut paint = Paint::default();
    paint.set_color_rgba8(channel(style.red), channel(style.green), channel(style.blue), 255);
    paint.anti_alias = true;
    let bounds = Rect::from_xywh(0.0, 0.0, w, h)?;
    match kind {
        ShapeKind::Rectangle => {
            let radius = (style.corner_radius.max(0.0) as f32).min(w / 2.0).min(h / 2.0);
            let path = if radius > 0.0 { rounded_rect(w, h, radius)? } else { PathBuilder::from_rect(bounds) };
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
        ShapeKind::Ellipse => {
            let path = PathBuilder::from_oval(bounds)?;
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
        ShapeKind::Line => {
            let thickness = style.line_width.unwrap_or(0.0).max(1.0) as f32;
            // Lines from older projects (no ends stored) run corner to corner, inset by half their thickness.
            let (ix, iy) = (thickness.min(w) / 2.0, thickness.min(h) / 2.0);
            let from = style.start.map_or((ix, iy), |p| (p[0] as f32 * w, p[1] as f32 * h));
            let to = style.end.map_or((w - ix, h - iy), |p| (p[0] as f32 * w, p[1] as f32 * h));
            let mut builder = PathBuilder::new();
            builder.move_to(from.0, from.1);
            builder.line_to(to.0, to.1);
            // A zero-length line is still a round dot.
            if from == to {
                builder.line_to(to.0 + 0.001, to.1);
            }
            let path = builder.finish()?;
            let stroke = tiny_skia::Stroke { width: thickness, line_cap: LineCap::Round, ..Default::default() };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
    let mut image = RgbaImage::new(pixmap.width(), pixmap.height());
    for (out, p) in image.pixels_mut().zip(pixmap.pixels()) {
        let c = p.demultiply();
        *out = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(image)
}

/// A `w`×`h` rectangle with circular corners of `r`.
fn rounded_rect(w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    // Control-point distance for a quarter circle drawn as a cubic.
    let k = r * 0.552_284_8;
    let mut b = tiny_skia::PathBuilder::new();
    b.move_to(r, 0.0);
    b.line_to(w - r, 0.0);
    b.cubic_to(w - r + k, 0.0, w, r - k, w, r);
    b.line_to(w, h - r);
    b.cubic_to(w, h - r + k, w - r + k, h, w - r, h);
    b.line_to(r, h);
    b.cubic_to(r - k, h, 0.0, h - r + k, 0.0, h - r);
    b.line_to(0.0, r);
    b.cubic_to(0.0, r - k, r - k, 0.0, r, 0.0);
    b.close();
    b.finish()
}

/// The pixel size a shape layer should have for its transform.
fn target_size(layer: &Layer) -> Option<(u32, u32)> {
    let t = &layer.transform;
    if !t.is_valid() {
        return None;
    }
    let (w, h) = (t.size[0].round().max(1.0) as u32, t.size[1].round().max(1.0) as u32);
    (w as u64 * h as u64 <= MAX_PIXELS).then_some((w, h))
}

/// Draws a shape layer again at its pixel size (`size`, or its transform's), keeping its style.
pub fn redraw(layer: &mut Layer, size: Option<(u32, u32)>) -> bool {
    let Some(style) = layer.shape.clone() else { return false };
    let Some((w, h)) = size.or_else(|| target_size(layer)) else { return false };
    let Some(image) = rasterize(&style, w, h) else { return false };
    layer.image = Some(Arc::new(image));
    true
}

/// Shape layers whose transform was scaled away from their pixel size are drawn again at that
/// size, so corners keep their radius and edges stay crisp. Returns whether any changed.
pub fn redraw_scaled(doc: &mut Document) -> bool {
    let mut changed = false;
    for layer in &mut doc.layers {
        if layer.shape.as_ref().and_then(|s| s.kind()).is_none() {
            continue;
        }
        let Some(target) = target_size(layer) else { continue };
        if layer.image.as_ref().is_some_and(|i| i.dimensions() != target) {
            changed |= redraw(layer, Some(target));
        }
    }
    changed
}

/// The Shape section of the Properties panel: the style of the active shape layer.
pub fn properties_ui(ui: &mut egui::Ui, project: &mut Project) {
    // A scale that has finished (a drag released, a typed size) redraws as part of that edit.
    if project.pending_edit.is_none() && !ui.input(|i| i.pointer.any_down()) && redraw_scaled(&mut project.doc) {
        project.invalidate_all();
    }
    let Some(layer) = project.doc.active_layer() else { return };
    let Some(original) = layer.shape.clone() else { return };
    let Some(kind) = original.kind() else {
        ui.weak(format!("{} shape", original.kind));
        return;
    };
    let mut style = original.clone();
    ui.strong(format!("Shape: {}", kind.name()));
    egui::Grid::new("shape").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("Color");
        let mut rgb = [style.red as f32, style.green as f32, style.blue as f32];
        if egui::color_picker::color_edit_button_rgb(ui, &mut rgb).changed() {
            [style.red, style.green, style.blue] = rgb.map(|c| c.clamp(0.0, 1.0) as f64);
        }
        ui.end_row();
        match kind {
            ShapeKind::Rectangle => {
                ui.label("Radius");
                ui.add(egui::DragValue::new(&mut style.corner_radius).speed(0.5).range(0.0..=5000.0).suffix(" px"));
                ui.end_row();
            }
            ShapeKind::Line => {
                ui.label("Width");
                let mut width = style.line_width.unwrap_or(1.0);
                if ui.add(egui::DragValue::new(&mut width).speed(0.5).range(1.0..=5000.0).suffix(" px")).changed() {
                    style.line_width = Some(width);
                }
                ui.end_row();
            }
            ShapeKind::Ellipse => {}
        }
    });
    if style != original {
        crate::ui::properties::change(project, |doc| {
            if let Some(l) = doc.active_layer_mut() {
                let size = l.image.as_ref().map(|i| i.dimensions());
                l.shape = Some(style);
                redraw(l, size);
            }
        });
    }
}

/// A shape being dragged out, in document pixels.
#[derive(Clone, Copy, Debug)]
struct Draft {
    anchor: (f64, f64),
    point: (f64, f64),
    modifiers: Modifiers,
}

/// What a draft makes: the layer's box, and for a line its two ends.
struct Geometry {
    rect: (f64, f64, f64, f64),
    line: Option<((f64, f64), (f64, f64))>,
}

pub struct ShapeTool {
    pub kind: ShapeKind,
    /// Document pixels; rectangles only.
    pub corner_radius: f64,
    pub line_width: f64,
    draft: Option<Draft>,
}

impl Default for ShapeTool {
    fn default() -> Self {
        ShapeTool { kind: ShapeKind::Rectangle, corner_radius: 0.0, line_width: 4.0, draft: None }
    }
}

impl ShapeTool {
    /// Shift makes a square or circle (or snaps a line to 45°); Alt grows the shape from its center.
    fn geometry(&self, draft: &Draft) -> Geometry {
        let anchor = draft.anchor;
        let (square, from_center) = (draft.modifiers.shift, draft.modifiers.alt);
        if self.kind == ShapeKind::Line {
            let mut end = draft.point;
            if square {
                let (dx, dy) = (end.0 - anchor.0, end.1 - anchor.1);
                let step = std::f64::consts::FRAC_PI_4;
                let angle = (dy.atan2(dx) / step).round() * step;
                let length = dx.hypot(dy);
                end = (anchor.0 + angle.cos() * length, anchor.1 + angle.sin() * length);
            }
            let start = if from_center { (2.0 * anchor.0 - end.0, 2.0 * anchor.1 - end.1) } else { anchor };
            // The layer is the ends' box with room for the line's thickness and round caps.
            let half = self.line_width.max(1.0) / 2.0;
            let (x0, y0) = (start.0.min(end.0) - half, start.1.min(end.1) - half);
            let (x1, y1) = (start.0.max(end.0) + half, start.1.max(end.1) + half);
            return Geometry { rect: (x0, y0, x1 - x0, y1 - y0), line: Some((start, end)) };
        }
        let mut dx = draft.point.0.round() - anchor.0;
        let mut dy = draft.point.1.round() - anchor.1;
        if square {
            let side = dx.abs().max(dy.abs());
            dx = side.copysign(dx);
            dy = side.copysign(dy);
        }
        let rect = if from_center {
            (anchor.0 - dx.abs(), anchor.1 - dy.abs(), dx.abs() * 2.0, dy.abs() * 2.0)
        } else {
            (anchor.0.min(anchor.0 + dx), anchor.1.min(anchor.1 + dy), dx.abs(), dy.abs())
        };
        Geometry { rect, line: None }
    }

    /// Draws the dragged shape in the foreground color on a new layer above the active one, in
    /// one undo step. A click without a drag makes nothing.
    fn finish(&mut self, ctx: &mut ToolCtx) {
        let Some(draft) = self.draft.take() else { return };
        ctx.project.invalidate_all();
        let geometry = self.geometry(&draft);
        let (x, y, w, h) = geometry.rect;
        if !(w >= 1.0 && h >= 1.0) || (self.kind == ShapeKind::Line && geometry.line.is_some_and(|(a, b)| a == b)) {
            return;
        }
        let (pw, ph) = (w.round().max(1.0) as u32, h.round().max(1.0) as u32);
        if pw as u64 * ph as u64 > MAX_PIXELS {
            *ctx.status = Some("That shape is too large. A shape can cover up to 100 megapixels.".to_string());
            return;
        }
        let fg = ctx.colors.foreground;
        let unit = |p: (f64, f64)| [(p.0 - x) / w, (p.1 - y) / h];
        let style = ShapeStyle {
            kind: self.kind.name().to_string(),
            red: fg[0] as f64 / 255.0,
            green: fg[1] as f64 / 255.0,
            blue: fg[2] as f64 / 255.0,
            corner_radius: if self.kind == ShapeKind::Rectangle { self.corner_radius } else { 0.0 },
            line_width: geometry.line.map(|_| self.line_width.max(1.0)),
            start: geometry.line.map(|(a, _)| unit(a)),
            end: geometry.line.map(|(_, b)| unit(b)),
            extra: Map::new(),
        };
        let Some(image) = rasterize(&style, pw, ph) else { return };
        let name = next_name(&ctx.project.doc, self.kind);
        let mut layer = Layer::new_pixel(name, image, LayerTransform::rect(x, y, pw as f64, ph as f64));
        layer.shape = Some(style);
        ctx.project.edit(self.kind.name(), |doc| {
            doc.insert_above_active(layer);
        });
    }
}

/// "Rectangle 1", "Ellipse 2", … skipping names already in the document.
fn next_name(doc: &Document, kind: ShapeKind) -> String {
    (1..)
        .map(|n| format!("{} {n}", kind.name()))
        .find(|name| !doc.layers.iter().any(|l| &l.name == name))
        .unwrap()
}

impl Tool for ShapeTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Shape
    }
    fn name(&self) -> &'static str {
        "Shape"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::SHAPES
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::U)
    }
    fn options_ui(&mut self, ui: &mut egui::Ui, _ctx: &mut ToolCtx) {
        for kind in ShapeKind::ALL {
            ui.selectable_value(&mut self.kind, kind, kind.name());
        }
        ui.separator();
        match self.kind {
            ShapeKind::Rectangle => {
                ui.label("Radius");
                ui.add(egui::DragValue::new(&mut self.corner_radius).speed(0.5).range(0.0..=5000.0).suffix(" px"));
            }
            ShapeKind::Line => {
                ui.label("Width");
                ui.add(egui::DragValue::new(&mut self.line_width).speed(0.5).range(1.0..=5000.0).suffix(" px"));
            }
            ShapeKind::Ellipse => {}
        }
    }
    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        match event.phase {
            PointerPhase::Press => {
                let anchor = (event.pos.0.round(), event.pos.1.round());
                self.draft = Some(Draft { anchor, point: anchor, modifiers: event.modifiers });
            }
            PointerPhase::Drag => {
                if let Some(draft) = &mut self.draft {
                    draft.point = event.pos;
                    draft.modifiers = event.modifiers;
                }
            }
            PointerPhase::Release => {
                if let Some(draft) = &mut self.draft {
                    draft.point = event.pos;
                    draft.modifiers = event.modifiers;
                }
                self.finish(ctx);
            }
            _ => {}
        }
    }
    /// Tab or Shift-U steps through Rectangle, Ellipse and Line.
    fn key(&mut self, key: Key, modifiers: Modifiers, _ctx: &mut ToolCtx) -> bool {
        if (key == Key::Tab && modifiers.is_none()) || (key == Key::U && modifiers == Modifiers::SHIFT) {
            self.draft = None;
            let index = ShapeKind::ALL.iter().position(|k| *k == self.kind).unwrap_or(0);
            self.kind = ShapeKind::ALL[(index + 1) % ShapeKind::ALL.len()];
            return true;
        }
        false
    }
    fn overlay(&self, painter: &Painter, view: &ViewTransform, _project: &Project, _hover: Option<(f64, f64)>) {
        let Some(draft) = &self.draft else { return };
        let geometry = self.geometry(draft);
        let fill = Color32::from_white_alpha(40);
        let outline = Stroke::new(1.0, Color32::from_rgb(0, 140, 255));
        if let Some((a, b)) = geometry.line {
            let width = (self.line_width.max(1.0) as f32 * view.zoom).max(1.0);
            painter.line_segment([view.to_screen(a.0, a.1), view.to_screen(b.0, b.1)], Stroke::new(width, fill));
            painter.line_segment([view.to_screen(a.0, a.1), view.to_screen(b.0, b.1)], outline);
            return;
        }
        let (x, y, w, h) = geometry.rect;
        let rect = view.doc_rect(x, y, x + w, y + h);
        match self.kind {
            ShapeKind::Ellipse => {
                painter.add(egui::Shape::ellipse_filled(rect.center(), rect.size() / 2.0, fill));
                painter.add(egui::Shape::ellipse_stroke(rect.center(), rect.size() / 2.0, outline));
            }
            _ => {
                let radius = (self.corner_radius as f32 * view.zoom).min(rect.width() / 2.0).min(rect.height() / 2.0);
                let corner = egui::CornerRadius::same(radius.clamp(0.0, 255.0) as u8);
                painter.rect_filled(rect, corner, fill);
                painter.rect_stroke(rect, corner, outline, egui::StrokeKind::Middle);
            }
        }
    }
    fn commit(&mut self, ctx: &mut ToolCtx) {
        self.finish(ctx);
    }
    fn cancel(&mut self, ctx: &mut ToolCtx) {
        if self.draft.take().is_some() {
            ctx.project.invalidate_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(kind: ShapeKind) -> ShapeStyle {
        ShapeStyle { kind: kind.name().to_string(), red: 1.0, ..ShapeStyle::default() }
    }

    #[test]
    fn rectangle_fills_its_box() {
        let image = rasterize(&style(ShapeKind::Rectangle), 20, 10).unwrap();
        assert!(image.pixels().all(|p| p.0 == [255, 0, 0, 255]));
    }

    #[test]
    fn rounded_corners_and_ellipses_leave_corners_clear() {
        let rounded = rasterize(&ShapeStyle { corner_radius: 8.0, ..style(ShapeKind::Rectangle) }, 40, 30).unwrap();
        assert_eq!(rounded.get_pixel(0, 0)[3], 0);
        assert_eq!(rounded.get_pixel(20, 0)[3], 255);
        assert_eq!(rounded.get_pixel(20, 15)[3], 255);
        let ellipse = rasterize(&style(ShapeKind::Ellipse), 40, 20).unwrap();
        assert_eq!(ellipse.get_pixel(20, 10).0, [255, 0, 0, 255]);
        assert_eq!(ellipse.get_pixel(1, 1)[3], 0);
        assert_eq!(ellipse.get_pixel(38, 18)[3], 0);
        // Anti-aliased: some edge pixels are partly covered.
        assert!(ellipse.pixels().any(|p| p[3] > 0 && p[3] < 255));
    }

    #[test]
    fn line_runs_between_its_ends() {
        let line = ShapeStyle {
            line_width: Some(4.0),
            start: Some([0.1, 0.5]),
            end: Some([0.9, 0.5]),
            ..style(ShapeKind::Line)
        };
        let image = rasterize(&line, 50, 10).unwrap();
        assert_eq!(image.get_pixel(25, 5)[3], 255);
        assert_eq!(image.get_pixel(25, 0)[3], 0);
        assert_eq!(image.get_pixel(0, 5)[3], 0);
    }

    #[test]
    fn style_round_trips_macos_fields() {
        let json = r#"{"kind":"Line","red":0.5,"green":0,"blue":1,"cornerRadius":0,"lineWidth":6,"start":[0,0.25],"end":[1,0.75],"later":true}"#;
        let style: ShapeStyle = serde_json::from_str(json).unwrap();
        assert_eq!(style.kind(), Some(ShapeKind::Line));
        assert_eq!(style.start, Some([0.0, 0.25]));
        let written = serde_json::to_value(&style).unwrap();
        assert_eq!(written["lineWidth"], 6.0);
        assert_eq!(written["later"], true);
        assert_eq!(serde_json::from_value::<ShapeStyle>(written).unwrap(), style);
        let rect = serde_json::to_value(ShapeStyle::default()).unwrap();
        assert!(rect.get("lineWidth").is_none() && rect.get("start").is_none());
        assert_eq!(rect["cornerRadius"], 0.0);
    }

    #[test]
    fn scaled_shapes_redraw_at_their_new_size() {
        let mut doc = Document::new(100, 100, None);
        let image = rasterize(&style(ShapeKind::Ellipse), 10, 10).unwrap();
        let mut layer = Layer::new_pixel("Ellipse 1", image, LayerTransform::rect(0.0, 0.0, 40.0, 20.0));
        layer.shape = Some(style(ShapeKind::Ellipse));
        doc.layers.push(layer);
        assert!(redraw_scaled(&mut doc));
        assert_eq!(doc.layers.last().unwrap().image.as_ref().unwrap().dimensions(), (40, 20));
        assert!(!redraw_scaled(&mut doc));
    }

    #[test]
    fn shift_squares_and_alt_centers() {
        let tool = ShapeTool::default();
        let draft = Draft { anchor: (10.0, 10.0), point: (20.0, 14.0), modifiers: Modifiers::SHIFT };
        assert_eq!(tool.geometry(&draft).rect, (10.0, 10.0, 10.0, 10.0));
        let draft = Draft { anchor: (10.0, 10.0), point: (20.0, 14.0), modifiers: Modifiers::ALT };
        assert_eq!(tool.geometry(&draft).rect, (0.0, 6.0, 20.0, 8.0));
    }
}
