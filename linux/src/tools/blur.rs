//! Blur tool: softens the pixels (or the mask) under the brush. The softened copy is made from
//! the pixels as the stroke began, a tile at a time as the brush reaches it, so going over an
//! area again in a new stroke softens it further, as in Photoshop.

use std::collections::HashMap;

use egui::{CursorIcon, Key, Modifiers, Painter};
use rayon::prelude::*;

use super::paint::{self, BrushSettings, Fields, Paint, Stroke, StrokeDriver, StrokeStep, Surface, TILE};
use super::{PointerEvent, Tool, ToolCtx, ToolKind};
use crate::project::Project;
use crate::ui::canvas::ViewTransform;

pub struct BlurTool {
    /// `opacity` is the Strength.
    pub settings: BrushSettings,
    /// How far it softens, in document pixels.
    pub radius: f32,
    driver: StrokeDriver,
    /// Softened pixels per stroke tile.
    blurred: HashMap<(u32, u32), Vec<[u8; 4]>>,
}

impl Default for BlurTool {
    fn default() -> Self {
        BlurTool {
            settings: BrushSettings { size: 60.0, hardness: 0.0, opacity: 0.5, ..Default::default() },
            radius: 5.0,
            driver: StrokeDriver::default(),
            blurred: HashMap::new(),
        }
    }
}

/// A normalized Gaussian kernel of standard deviation `sigma` (radius 3σ).
pub fn kernel(sigma: f32) -> Vec<f32> {
    let reach = (sigma * 3.0).ceil().max(1.0) as i32;
    let mut k: Vec<f32> = (-reach..=reach).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k
}

/// One tile of the surface's original pixels blurred by `kernel`, straight RGBA. Edges clamp.
pub fn blur_tile(surface: &Surface, key: (u32, u32), kernel: &[f32]) -> Vec<[u8; 4]> {
    let reach = (kernel.len() / 2) as i64;
    let (w, h) = (surface.width as i64, surface.height as i64);
    let (x0, y0) = ((key.0 * TILE) as i64, (key.1 * TILE) as i64);
    let (x1, y1) = ((x0 + TILE as i64).min(w), (y0 + TILE as i64).min(h));
    let (tw, th) = ((x1 - x0) as usize, (y1 - y0) as usize);
    // Premultiplied source rows covering the tile plus the kernel's reach, clamped at the edges.
    let sx0 = x0 - reach;
    let sw = tw + 2 * reach as usize;
    let sy0 = y0 - reach;
    let sh = th + 2 * reach as usize;
    let mut source = vec![[0f32; 4]; sw * sh];
    for j in 0..sh {
        let y = (sy0 + j as i64).clamp(0, h - 1) as u32;
        for i in 0..sw {
            let x = (sx0 + i as i64).clamp(0, w - 1) as u32;
            let p = surface.original_at(x, y);
            let a = p[3] as f32 / 255.0;
            source[j * sw + i] = [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, a];
        }
    }
    // Horizontal pass over every source row, then vertical into the tile.
    let mut across = vec![[0f32; 4]; tw * sh];
    for j in 0..sh {
        for i in 0..tw {
            let mut sum = [0f32; 4];
            for (k, weight) in kernel.iter().enumerate() {
                let p = source[j * sw + i + k];
                for c in 0..4 {
                    sum[c] += p[c] * weight;
                }
            }
            across[j * tw + i] = sum;
        }
    }
    let mut out = vec![[0u8; 4]; (TILE * TILE) as usize];
    for j in 0..th {
        for i in 0..tw {
            let mut sum = [0f32; 4];
            for (k, weight) in kernel.iter().enumerate() {
                let p = across[(j + k) * tw + i];
                for c in 0..4 {
                    sum[c] += p[c] * weight;
                }
            }
            let a = sum[3];
            out[j * TILE as usize + i] = if a <= 1e-6 {
                [0, 0, 0, 0]
            } else {
                let to = |v: f32| (v / a + 0.5).clamp(0.0, 255.0) as u8;
                [to(sum[0]), to(sum[1]), to(sum[2]), (a * 255.0 + 0.5).clamp(0.0, 255.0) as u8]
            };
        }
    }
    out
}

impl BlurTool {
    fn flush(&mut self, ctx: &mut ToolCtx, stroke: &mut Stroke) {
        let bands = stroke.take_dirty();
        if bands.is_empty() {
            return;
        }
        // The radius is on the canvas; carry it into the layer's pixels.
        let m = stroke.surface.to_doc;
        let per_pixel = (m.a * m.d - m.b * m.c).abs().sqrt().max(1e-6);
        let sigma = (self.radius as f64 / per_pixel).clamp(0.3, 200.0) as f32;
        let kernel = kernel(sigma);
        let missing: Vec<(u32, u32)> = bands
            .iter()
            .flat_map(|b| {
                let ty = b.rows.0 / TILE;
                b.spans.iter().flat_map(move |&(x0, x1)| (x0 / TILE..x1.div_ceil(TILE)).map(move |tx| (tx, ty)))
            })
            .filter(|key| !self.blurred.contains_key(key))
            .collect();
        let surface = &stroke.surface;
        let made: Vec<_> = missing.into_par_iter().map(|key| (key, blur_tile(surface, key, &kernel))).collect();
        self.blurred.extend(made);
        let blurred = &self.blurred;
        let source = |x: u32, y: u32| {
            blurred.get(&(x / TILE, y / TILE)).map_or([0; 4], |tile| tile[((y % TILE) * TILE + x % TILE) as usize])
        };
        stroke.composite(ctx.project, &bands, self.settings.opacity, &Paint::Mix(&source));
    }

    fn handle(&mut self, step: StrokeStep, ctx: &mut ToolCtx) {
        match step {
            StrokeStep::None => {}
            StrokeStep::Painted => {
                let Some(mut stroke) = self.driver.stroke.take() else { return };
                self.flush(ctx, &mut stroke);
                self.driver.stroke = Some(stroke);
            }
            StrokeStep::Finished(mut stroke) => {
                self.flush(ctx, &mut stroke);
                ctx.project.finish_edit();
                self.blurred.clear();
            }
        }
    }
}

impl Tool for BlurTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Blur
    }
    fn name(&self) -> &'static str {
        "Blur"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::DROP
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::R)
    }
    fn options_ui(&mut self, ui: &mut egui::Ui, _ctx: &mut ToolCtx) {
        paint::options_ui(ui, &mut self.settings, Fields { hardness: true, opacity: Some("Strength"), flow: false, smoothing: false });
        ui.label("Radius");
        ui.add(egui::DragValue::new(&mut self.radius).range(0.5..=50.0).speed(0.1).suffix(" px").max_decimals(1))
            .on_hover_text("How far the blur spreads, in canvas pixels");
    }
    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        let step = self.driver.pointer(event, ctx, &self.settings, "Blur", true);
        self.handle(step, ctx);
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
        let step = self.driver.end();
        self.handle(step, ctx);
    }
    fn cancel(&mut self, ctx: &mut ToolCtx) {
        self.commit(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_is_normalized() {
        let k = kernel(2.0);
        assert_eq!(k.len(), 13);
        assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(k[6] > k[5] && k[5] > k[0]);
    }

    #[test]
    fn blur_softens_an_edge_and_keeps_flat_areas() {
        let mut project = crate::tools::paint::tests::project(128, 64, [255, 255, 255, 255]);
        {
            let image = std::sync::Arc::make_mut(project.doc.layers[0].image.as_mut().unwrap());
            for y in 0..64 {
                for x in 64..128 {
                    image.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                }
            }
        }
        let surface = Surface::begin(&mut project, "Blur", true).unwrap();
        let tile = blur_tile(&surface, (0, 0), &kernel(3.0));
        assert_eq!(tile[10 * 64 + 10], [255, 255, 255, 255]);
        let edge = tile[10 * 64 + 63];
        assert!(edge[0] > 60 && edge[0] < 200, "{edge:?}");
    }
}
