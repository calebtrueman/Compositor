//! Gradient tool: drag to lay a linear or radial gradient from the foreground to the background
//! (or to transparent) over the active layer or its mask, inside the selection. Shift
//! constrains the line to 45° steps.

use egui::{Color32, CursorIcon, Key, Modifiers, Painter, Stroke};

use super::paint::{self, Surface};
use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::project::Project;
use crate::ui::canvas::ViewTransform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GradientShape {
    Linear,
    Radial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GradientStyle {
    ForegroundToBackground,
    ForegroundToTransparent,
}

pub struct GradientTool {
    pub shape: GradientShape,
    pub style: GradientStyle,
    pub reverse: bool,
    pub dither: bool,
    pub opacity: f32,
    /// The line being dragged, start and end in document pixels.
    line: Option<((f64, f64), (f64, f64))>,
}

impl Default for GradientTool {
    fn default() -> Self {
        GradientTool {
            shape: GradientShape::Linear,
            style: GradientStyle::ForegroundToBackground,
            reverse: false,
            dither: true,
            opacity: 1.0,
            line: None,
        }
    }
}

/// Snaps `end` so the line from `start` runs at a multiple of 45°.
pub fn constrain(start: (f64, f64), end: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (end.0 - start.0, end.1 - start.1);
    let length = dx.hypot(dy);
    let step = std::f64::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    (start.0 + length * angle.cos(), start.1 + length * angle.sin())
}

/// Position 0…1 along the gradient at document point `p`.
#[inline]
pub fn position(shape: GradientShape, start: (f64, f64), end: (f64, f64), p: (f64, f64)) -> f64 {
    let (vx, vy) = (end.0 - start.0, end.1 - start.1);
    let (px, py) = (p.0 - start.0, p.1 - start.1);
    let t = match shape {
        GradientShape::Linear => (px * vx + py * vy) / (vx * vx + vy * vy).max(1e-12),
        GradientShape::Radial => px.hypot(py) / vx.hypot(vy).max(1e-12),
    };
    t.clamp(0.0, 1.0)
}

/// A small, fixed noise per pixel (±½ level) that hides banding.
#[inline]
fn dither_noise(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(0x9e37_79b1) ^ y.wrapping_mul(0x85eb_ca77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65535.0 - 0.5
}

impl GradientTool {
    /// The colors at either end, straight RGBA.
    fn ends(&self, ctx: &ToolCtx) -> ([f32; 4], [f32; 4]) {
        let f = ctx.colors.foreground.map(|v| v as f32);
        let to = match self.style {
            GradientStyle::ForegroundToBackground => ctx.colors.background.map(|v| v as f32),
            GradientStyle::ForegroundToTransparent => [f[0], f[1], f[2], 0.0],
        };
        if self.reverse {
            (to, f)
        } else {
            (f, to)
        }
    }

    fn apply(&self, ctx: &mut ToolCtx, start: (f64, f64), end: (f64, f64)) {
        let surface = match Surface::begin(ctx.project, "Gradient", true) {
            Ok(surface) => surface,
            Err(message) => {
                *ctx.status = Some(message);
                return;
            }
        };
        let (a, b) = self.ends(ctx);
        let shape = self.shape;
        let opacity = self.opacity;
        let dither = self.dither;
        // Interpolated premultiplied, so fading to transparent keeps the color.
        let color_at = |x: u32, y: u32| -> [u8; 4] {
            let (dx, dy) = surface.to_doc.apply(x as f64 + 0.5, y as f64 + 0.5);
            let t = position(shape, start, end, (dx, dy)) as f32;
            let alpha = a[3] + (b[3] - a[3]) * t;
            let noise = if dither { dither_noise(x, y) } else { 0.0 };
            let mut out = [0u8; 4];
            for c in 0..3 {
                let p = a[c] * a[3] + (b[c] * b[3] - a[c] * a[3]) * t;
                let v = if alpha > 0.0 { p / alpha } else { a[c] };
                out[c] = (v + noise + 0.5).clamp(0.0, 255.0) as u8;
            }
            out[3] = (alpha + noise + 0.5).clamp(0.0, 255.0) as u8;
            out
        };
        let mask = surface.mask;
        let rows = (0, surface.height);
        let spans = [(0, surface.width)];
        let strength = |x: u32, y: u32| opacity * surface.limit(x, y);
        let paint = paint::Paint::Over(&color_at);
        if mask {
            // A mask takes the gray of each color, laid over by its alpha.
            let gray = |x: u32, y: u32| {
                let c = color_at(x, y);
                let g = paint::gray_of(c);
                [g, g, g, c[3]]
            };
            surface.apply(&mut ctx.project.doc, rows, &spans, &paint::Paint::Over(&gray), &strength);
        } else {
            surface.apply(&mut ctx.project.doc, rows, &spans, &paint, &strength);
        }
        ctx.project.finish_edit();
        ctx.project.invalidate_all();
    }
}

impl Tool for GradientTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Gradient
    }
    fn name(&self) -> &'static str {
        "Gradient"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::GRADIENT
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::G)
    }
    fn options_ui(&mut self, ui: &mut egui::Ui, ctx: &mut ToolCtx) {
        // A preview of the colors as they'll run.
        let (a, b) = self.ends(ctx);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(80.0, 18.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let steps = 40;
        for i in 0..steps {
            let t = i as f32 / (steps - 1) as f32;
            let c: Vec<f32> = (0..4).map(|k| a[k] + (b[k] - a[k]) * t).collect();
            let x0 = rect.left() + rect.width() * i as f32 / steps as f32;
            let r = egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x0 + rect.width() / steps as f32 + 0.5, rect.bottom()));
            let check = if i % 4 < 2 { 200 } else { 150 };
            painter.rect_filled(r, 0.0, Color32::from_gray(check));
            painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(c[0] as u8, c[1] as u8, c[2] as u8, c[3] as u8));
        }
        painter.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_gray(90)), egui::StrokeKind::Inside);
        egui::ComboBox::from_id_salt("gradient-style")
            .selected_text(match self.style {
                GradientStyle::ForegroundToBackground => "Foreground to Background",
                GradientStyle::ForegroundToTransparent => "Foreground to Transparent",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.style, GradientStyle::ForegroundToBackground, "Foreground to Background");
                ui.selectable_value(&mut self.style, GradientStyle::ForegroundToTransparent, "Foreground to Transparent");
            });
        ui.separator();
        ui.selectable_value(&mut self.shape, GradientShape::Linear, "Linear");
        ui.selectable_value(&mut self.shape, GradientShape::Radial, "Radial");
        ui.separator();
        ui.label("Opacity");
        let mut p = self.opacity * 100.0;
        if ui.add(egui::DragValue::new(&mut p).range(1.0..=100.0).speed(0.5).suffix("%").max_decimals(0)).changed() {
            self.opacity = p / 100.0;
        }
        ui.checkbox(&mut self.reverse, "Reverse");
        ui.checkbox(&mut self.dither, "Dither");
    }
    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        match event.phase {
            PointerPhase::Press => self.line = Some((event.pos, event.pos)),
            PointerPhase::Drag => {
                if let Some((start, _)) = self.line {
                    let end = if event.modifiers.shift { constrain(start, event.pos) } else { event.pos };
                    self.line = Some((start, end));
                }
            }
            PointerPhase::Release => {
                if let Some((start, _)) = self.line.take() {
                    let end = if event.modifiers.shift { constrain(start, event.pos) } else { event.pos };
                    if (end.0 - start.0).hypot(end.1 - start.1) >= 0.5 {
                        self.apply(ctx, start, end);
                    }
                }
            }
            _ => {}
        }
    }
    fn overlay(&self, painter: &Painter, view: &ViewTransform, _project: &Project, _hover: Option<(f64, f64)>) {
        let Some((start, end)) = self.line else { return };
        let a = view.to_screen(start.0, start.1);
        let b = view.to_screen(end.0, end.1);
        painter.line_segment([a, b], Stroke::new(3.0, Color32::from_black_alpha(140)));
        painter.line_segment([a, b], Stroke::new(1.0, Color32::WHITE));
        for p in [a, b] {
            painter.circle_filled(p, 3.5, Color32::WHITE);
            painter.circle_stroke(p, 3.5, Stroke::new(1.0, Color32::from_black_alpha(160)));
        }
        if self.shape == GradientShape::Radial {
            painter.circle_stroke(a, a.distance(b), Stroke::new(1.0, Color32::from_white_alpha(120)));
        }
    }
    fn cursor(&self, _project: &Project, _hover: (f64, f64), _modifiers: Modifiers) -> CursorIcon {
        CursorIcon::Crosshair
    }
    fn cancel(&mut self, _ctx: &mut ToolCtx) {
        self.line = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::RenderCache;
    use crate::tools::Colors;

    #[test]
    fn positions_and_constraint() {
        let s = (10.0, 10.0);
        assert_eq!(position(GradientShape::Linear, s, (20.0, 10.0), (15.0, 99.0)), 0.5);
        assert_eq!(position(GradientShape::Linear, s, (20.0, 10.0), (0.0, 0.0)), 0.0);
        assert_eq!(position(GradientShape::Radial, s, (20.0, 10.0), (10.0, 30.0)), 1.0);
        let e = constrain(s, (30.0, 12.0));
        assert!((e.1 - 10.0).abs() < 1e-9);
        let e = constrain(s, (20.0, 21.0));
        assert!((e.0 - 10.0 - (e.1 - 10.0)).abs() < 1e-9);
    }

    #[test]
    fn lays_foreground_to_background() {
        let mut project = crate::tools::paint::tests::project(100, 10, [0, 255, 0, 255]);
        let mut colors = Colors::default();
        let cache = RenderCache::default();
        let mut status = None;
        let mut ctx = ToolCtx { project: &mut project, colors: &mut colors, cache: &cache, zoom: 1.0, status: &mut status };
        let tool = GradientTool { dither: false, ..Default::default() };
        tool.apply(&mut ctx, (0.0, 5.0), (100.0, 5.0));
        let image = project.doc.layers[0].image.as_ref().unwrap();
        assert_eq!(image.get_pixel(0, 5).0, [1, 1, 1, 255]);
        assert_eq!(image.get_pixel(99, 5).0, [254, 254, 254, 255]);
        assert!((image.get_pixel(50, 5)[0] as i32 - 129).abs() <= 1);
        assert_eq!(project.history.undo_name(), Some("Gradient"));
    }
}
