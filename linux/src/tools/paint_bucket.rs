//! Paint Bucket: fills the area of similar color around the click with the foreground color.
//! Tolerance is the largest per-channel difference (0…255) that still counts as similar;
//! Contiguous fills only the connected area; Anti-alias softens the fill's edge. It samples the
//! active layer (or its mask) or the merged image.

use egui::{CursorIcon, Key, Modifiers};
use image::{GrayImage, RgbaImage};

use super::paint::{self, Original, Paint, Surface};
use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::project::{EditTarget, Project};
use crate::render::{self, Region};

pub struct PaintBucketTool {
    pub tolerance: u8,
    pub contiguous: bool,
    pub anti_alias: bool,
    pub all_layers: bool,
    pub opacity: f32,
}

impl Default for PaintBucketTool {
    fn default() -> Self {
        PaintBucketTool { tolerance: 32, contiguous: true, anti_alias: true, all_layers: false, opacity: 1.0 }
    }
}

/// Whether two straight RGBA pixels are within `tolerance`, compared premultiplied so all
/// transparent pixels match each other.
#[inline]
fn similar(a: [u8; 4], b: [u8; 4], tolerance: u8) -> bool {
    let pa = |p: [u8; 4], c: usize| (p[c] as u32 * p[3] as u32 + 127) / 255;
    let t = tolerance as i32;
    (0..3).all(|c| (pa(a, c) as i32 - pa(b, c) as i32).abs() <= t) && (a[3] as i32 - b[3] as i32).abs() <= t
}

/// The fill's coverage (0 or 255) over a `w`×`h` image read through `pixel`, seeded at `seed`.
pub fn flood_fill(w: u32, h: u32, pixel: impl Fn(u32, u32) -> [u8; 4], seed: (u32, u32), tolerance: u8, contiguous: bool) -> GrayImage {
    let mut out = GrayImage::new(w, h);
    if seed.0 >= w || seed.1 >= h {
        return out;
    }
    let target = pixel(seed.0, seed.1);
    let matches = |x: u32, y: u32| similar(pixel(x, y), target, tolerance);
    if !contiguous {
        for (x, y, p) in out.enumerate_pixels_mut() {
            if matches(x, y) {
                p[0] = 255;
            }
        }
        return out;
    }
    // Scanline fill: fill a run, then queue the runs above and below it.
    let raw = out.as_mut();
    let idx = |x: u32, y: u32| (y * w + x) as usize;
    let mut stack = vec![seed];
    while let Some((x, y)) = stack.pop() {
        if raw[idx(x, y)] != 0 || !matches(x, y) {
            continue;
        }
        let mut left = x;
        while left > 0 && raw[idx(left - 1, y)] == 0 && matches(left - 1, y) {
            left -= 1;
        }
        let mut right = x;
        while right + 1 < w && raw[idx(right + 1, y)] == 0 && matches(right + 1, y) {
            right += 1;
        }
        for i in left..=right {
            raw[idx(i, y)] = 255;
        }
        for ny in [y.wrapping_sub(1), y + 1] {
            if ny >= h {
                continue;
            }
            let mut i = left;
            while i <= right {
                // One seed per run of fillable pixels.
                if raw[idx(i, ny)] == 0 && matches(i, ny) {
                    stack.push((i, ny));
                    while i <= right && raw[idx(i, ny)] == 0 && matches(i, ny) {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            }
        }
    }
    out
}

/// Softens a hard fill's edge: each pixel becomes the average of its 3×3 neighborhood.
pub fn anti_alias(mask: &GrayImage) -> GrayImage {
    let (w, h) = mask.dimensions();
    let mut out = mask.clone();
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0u32;
            let mut n = 0u32;
            let mut edge = false;
            let here = mask.get_pixel(x, y)[0];
            for j in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for i in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let v = mask.get_pixel(i, j)[0];
                    edge |= v != here;
                    sum += v as u32;
                    n += 1;
                }
            }
            if edge {
                out.put_pixel(x, y, image::Luma([((sum + n / 2) / n) as u8]));
            }
        }
    }
    out
}

impl PaintBucketTool {
    fn fill(&self, ctx: &mut ToolCtx, pos: (f64, f64)) {
        let doc = &ctx.project.doc;
        if pos.0 < 0.0 || pos.1 < 0.0 || pos.0 >= doc.width as f64 || pos.1 >= doc.height as f64 {
            return;
        }
        // The merged image, rendered in bands to keep memory down.
        let merged = self.all_layers.then(|| {
            let mut image = RgbaImage::new(doc.width, doc.height);
            let band = 512u32;
            for y in (0..doc.height).step_by(band as usize) {
                let rows = band.min(doc.height - y);
                let region = Region { x: 0, y: y as i64, width: doc.width as usize, height: rows as usize, scale: 1.0 };
                let part = render::composite(doc, region, ctx.cache).to_rgba8();
                image::imageops::replace(&mut image, &part, 0, y as i64);
            }
            image
        });
        if !self.all_layers {
            let id = doc.active;
            let off_layer = id.and_then(|id| doc.layer(id)).is_some_and(|l| {
                let target_mask = ctx.project.target == EditTarget::Mask && l.mask.is_some();
                !target_mask && l.image.is_some() && super::layer_pixel_at(ctx.project, l.id, pos).is_none()
            });
            if off_layer {
                return;
            }
        }
        let surface = match Surface::begin(ctx.project, "Paint Bucket", true) {
            Ok(surface) => surface,
            Err(message) => {
                *ctx.status = Some(message);
                return;
            }
        };
        let coverage: Box<dyn Fn(u32, u32) -> f32 + Sync> = match merged {
            Some(merged) => {
                let seed = (pos.0 as u32, pos.1 as u32);
                let mut fill = flood_fill(merged.width(), merged.height(), |x, y| merged.get_pixel(x, y).0, seed, self.tolerance, self.contiguous);
                if self.anti_alias {
                    fill = anti_alias(&fill);
                }
                let to_doc = surface.to_doc;
                Box::new(move |x, y| {
                    let (dx, dy) = to_doc.apply(x as f64 + 0.5, y as f64 + 0.5);
                    if dx < 0.0 || dy < 0.0 || dx >= fill.width() as f64 || dy >= fill.height() as f64 {
                        0.0
                    } else {
                        fill.get_pixel(dx as u32, dy as u32)[0] as f32 / 255.0
                    }
                })
            }
            None => {
                let (sx, sy) = surface.to_pixel.apply(pos.0, pos.1);
                let seed = (sx.max(0.0) as u32, sy.max(0.0) as u32);
                let mut fill = match &surface.original {
                    Original::Rgba(image) => flood_fill(image.width(), image.height(), |x, y| image.get_pixel(x, y).0, seed, self.tolerance, self.contiguous),
                    Original::Gray(gray) => flood_fill(
                        gray.width(),
                        gray.height(),
                        |x, y| {
                            let g = gray.get_pixel(x, y)[0];
                            [g, g, g, 255]
                        },
                        seed,
                        self.tolerance,
                        self.contiguous,
                    ),
                };
                if self.anti_alias {
                    fill = anti_alias(&fill);
                }
                Box::new(move |x, y| fill.get_pixel(x, y)[0] as f32 / 255.0)
            }
        };
        let color = paint::paint_color(ctx, surface.mask);
        let opacity = self.opacity;
        let strength = |x: u32, y: u32| {
            let c = coverage(x, y);
            if c <= 0.0 {
                0.0
            } else {
                c * opacity * surface.limit(x, y)
            }
        };
        surface.apply(&mut ctx.project.doc, (0, surface.height), &[(0, surface.width)], &Paint::Color(color), &strength);
        ctx.project.finish_edit();
        ctx.project.invalidate_all();
    }
}

impl Tool for PaintBucketTool {
    fn kind(&self) -> ToolKind {
        ToolKind::PaintBucket
    }
    fn name(&self) -> &'static str {
        "Paint Bucket"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::PAINT_BUCKET
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::G)
    }
    fn options_ui(&mut self, ui: &mut egui::Ui, _ctx: &mut ToolCtx) {
        ui.label("Opacity");
        let mut p = self.opacity * 100.0;
        if ui.add(egui::DragValue::new(&mut p).range(1.0..=100.0).speed(0.5).suffix("%").max_decimals(0)).changed() {
            self.opacity = p / 100.0;
        }
        ui.label("Tolerance");
        ui.add(egui::DragValue::new(&mut self.tolerance).range(0..=255).speed(0.5));
        ui.checkbox(&mut self.anti_alias, "Anti-alias");
        ui.checkbox(&mut self.contiguous, "Contiguous");
        ui.checkbox(&mut self.all_layers, "All Layers").on_hover_text("Find the area in the merged image instead of this layer");
    }
    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        if event.phase == PointerPhase::Press {
            self.fill(ctx, event.pos);
        }
    }
    fn cursor(&self, _project: &Project, _hover: (f64, f64), _modifiers: Modifiers) -> CursorIcon {
        CursorIcon::Crosshair
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::RenderCache;
    use crate::tools::Colors;

    /// Two white areas split by a black column, with a near-white pixel on the left.
    fn split(x: u32, y: u32) -> [u8; 4] {
        match (x, y) {
            (5, _) => [0, 0, 0, 255],
            (1, 1) => [240, 240, 240, 255],
            _ => [255, 255, 255, 255],
        }
    }

    #[test]
    fn flood_fill_respects_contiguity_and_tolerance() {
        let fill = flood_fill(10, 4, split, (0, 0), 32, true);
        assert_eq!(fill.get_pixel(4, 3)[0], 255);
        assert_eq!(fill.get_pixel(1, 1)[0], 255, "within tolerance");
        assert_eq!(fill.get_pixel(5, 0)[0], 0, "the wall");
        assert_eq!(fill.get_pixel(7, 2)[0], 0, "beyond the wall");
        let strict = flood_fill(10, 4, split, (0, 0), 0, true);
        assert_eq!(strict.get_pixel(1, 1)[0], 0);
        let everywhere = flood_fill(10, 4, split, (0, 0), 0, false);
        assert_eq!(everywhere.get_pixel(7, 2)[0], 255);
        assert_eq!(everywhere.get_pixel(5, 2)[0], 0);
    }

    #[test]
    fn flood_fill_follows_winding_paths() {
        // A spiral-ish maze: walls on alternate rows with gaps at alternating ends.
        let pixel = |x: u32, y: u32| {
            let wall = y % 2 == 1 && if (y / 2) % 2 == 0 { x < 19 } else { x > 0 };
            if wall { [0, 0, 0, 255] } else { [255, 255, 255, 255] }
        };
        let fill = flood_fill(20, 21, pixel, (0, 0), 0, true);
        assert_eq!(fill.get_pixel(10, 20)[0], 255);
        assert_eq!(fill.get_pixel(10, 1)[0], 0);
    }

    #[test]
    fn anti_alias_softens_only_edges() {
        let fill = flood_fill(10, 4, split, (0, 0), 32, true);
        let soft = anti_alias(&fill);
        assert_eq!(soft.get_pixel(1, 2)[0], 255);
        let edge = soft.get_pixel(4, 2)[0];
        assert!(edge > 100 && edge < 255);
        assert!(soft.get_pixel(5, 2)[0] > 0);
    }

    #[test]
    fn fills_the_layer_with_the_foreground() {
        let mut project = crate::tools::paint::tests::project(10, 4, [255, 255, 255, 255]);
        {
            let image = std::sync::Arc::make_mut(project.doc.layers[0].image.as_mut().unwrap());
            for y in 0..4 {
                image.put_pixel(5, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let cache = RenderCache::default();
        for all_layers in [false, true] {
            let mut p = Project::new(project.doc.clone(), None, "Test".into());
            let mut colors = Colors { foreground: [255, 0, 0, 255], ..Default::default() };
            let mut status = None;
            let mut ctx = ToolCtx { project: &mut p, colors: &mut colors, cache: &cache, zoom: 1.0, status: &mut status };
            let tool = PaintBucketTool { anti_alias: false, all_layers, ..Default::default() };
            tool.fill(&mut ctx, (1.5, 1.5));
            let image = p.doc.layers[0].image.as_ref().unwrap();
            assert_eq!(image.get_pixel(2, 2).0, [255, 0, 0, 255]);
            assert_eq!(image.get_pixel(8, 2).0, [255, 255, 255, 255]);
        }
    }
}
