//! Spot Healing Brush: paint over a blemish; on release it's filled from the surroundings. A
//! nearby patch whose border best matches the spot's border supplies texture, and the color
//! difference along the border is spread smoothly across the spot (a membrane, solved by
//! relaxation), so the patch blends in. Create Texture skips the patch and fills with the smooth
//! membrane plus grain matching the surroundings. A port of the macOS `HealPixels.c`.

use egui::{CursorIcon, Key, Modifiers, Painter};

use super::paint::{self, BrushSettings, Fields, Original, Paint, Stroke, StrokeDriver, StrokeStep};
use super::{PointerEvent, Tool, ToolCtx, ToolKind};
use crate::project::Project;
use crate::ui::canvas::ViewTransform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealMode {
    ContentAware,
    CreateTexture,
    ProximityMatch,
}

impl HealMode {
    const ALL: [HealMode; 3] = [HealMode::ContentAware, HealMode::CreateTexture, HealMode::ProximityMatch];
    fn name(self) -> &'static str {
        match self {
            HealMode::ContentAware => "Content-Aware",
            HealMode::CreateTexture => "Create Texture",
            HealMode::ProximityMatch => "Proximity Match",
        }
    }
}

pub struct SpotHealingTool {
    pub settings: BrushSettings,
    pub mode: HealMode,
    driver: StrokeDriver,
    seed: u32,
}

impl Default for SpotHealingTool {
    fn default() -> Self {
        SpotHealingTool {
            settings: BrushSettings { size: 30.0, hardness: 0.5, ..Default::default() },
            mode: HealMode::ContentAware,
            driver: StrokeDriver::default(),
            seed: 0x9e37_79b9,
        }
    }
}

/// While painting, the spot shows as a translucent dark stroke, as in Photoshop.
const PREVIEW: Paint<'static> = Paint::Color([20, 20, 20, 255]);
const PREVIEW_OPACITY: f32 = 0.45;

const OUTSIDE: u8 = 0;
const RING: u8 = 1;
const HOLE: u8 = 2;

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

fn unit(key: u32) -> f64 {
    (hash(key) >> 8) as f64 / 16_777_216.0
}

/// Mean squared difference between the ring around the spot and the ring around the patch at
/// `(dx, dy)`; infinite when the patch would overlap the spot or leave the image.
fn score(rgba: &[u8], role: &[u8], work: (i64, i64, i64, i64), dx: i64, dy: i64, w: i64, h: i64) -> f64 {
    let (wx0, wy0, ww, wh) = work;
    if dx.abs() < ww && dy.abs() < wh {
        return f64::INFINITY;
    }
    if wx0 + dx < 0 || wy0 + dy < 0 || wx0 + ww + dx > w || wy0 + wh + dy > h {
        return f64::INFINITY;
    }
    let mut sum = 0.0;
    let mut n = 0;
    for y in 0..wh {
        for x in 0..ww {
            if role[(y * ww + x) as usize] != RING {
                continue;
            }
            let t = (((wy0 + y) * w + wx0 + x) * 4) as usize;
            let s = (((wy0 + y + dy) * w + wx0 + x + dx) * 4) as usize;
            for c in 0..4 {
                let d = rgba[t + c] as f64 - rgba[s + c] as f64;
                sum += d * d;
            }
            n += 1;
        }
    }
    if n > 0 {
        sum / n as f64
    } else {
        f64::INFINITY
    }
}

/// Solves for smooth values over HOLE pixels, fixed to the RING values around them. A coarser
/// copy is solved first and used as the starting point, so large spots settle in few passes.
fn solve(value: &mut [[f32; 4]], role: &[u8], w: usize, h: usize, depth: u32) {
    let mut iterations = 300;
    if w > 32 && h > 32 && depth < 16 {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut coarse = vec![[0f32; 4]; cw * ch];
        let mut coarse_role = vec![OUTSIDE; cw * ch];
        for y in 0..ch {
            for x in 0..cw {
                let (mut known, mut hole) = (0, 0);
                let (mut known_sum, mut hole_sum) = ([0f32; 4], [0f32; 4]);
                for j in 0..2 {
                    for i in 0..2 {
                        let (fx, fy) = (x * 2 + i, y * 2 + j);
                        if fx >= w || fy >= h {
                            continue;
                        }
                        let p = fy * w + fx;
                        let sum = match role[p] {
                            RING => {
                                known += 1;
                                &mut known_sum
                            }
                            HOLE => {
                                hole += 1;
                                &mut hole_sum
                            }
                            _ => continue,
                        };
                        for c in 0..4 {
                            sum[c] += value[p][c];
                        }
                    }
                }
                let q = y * cw + x;
                if known > 0 {
                    coarse_role[q] = RING;
                    coarse[q] = known_sum.map(|v| v / known as f32);
                } else if hole > 0 {
                    coarse_role[q] = HOLE;
                    coarse[q] = hole_sum.map(|v| v / hole as f32);
                }
            }
        }
        solve(&mut coarse, &coarse_role, cw, ch, depth + 1);
        for y in 0..h {
            for x in 0..w {
                let (p, q) = (y * w + x, (y / 2) * cw + x / 2);
                if role[p] == HOLE && coarse_role[q] == HOLE {
                    value[p] = coarse[q];
                }
            }
        }
        iterations = 40;
    }
    let omega = 1.8f32;
    for _ in 0..iterations {
        for y in 0..h {
            for x in 0..w {
                let p = y * w + x;
                if role[p] != HOLE {
                    continue;
                }
                let mut sum = [0f32; 4];
                let mut n = 0;
                let neighbors = [(x as i64 - 1, y as i64), (x as i64 + 1, y as i64), (x as i64, y as i64 - 1), (x as i64, y as i64 + 1)];
                for (nx, ny) in neighbors {
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let q = ny as usize * w + nx as usize;
                    if role[q] == OUTSIDE {
                        continue;
                    }
                    for c in 0..4 {
                        sum[c] += value[q][c];
                    }
                    n += 1;
                }
                if n == 0 {
                    continue;
                }
                for c in 0..4 {
                    value[p][c] += omega * (sum[c] / n as f32 - value[p][c]);
                }
            }
        }
    }
}

/// Heals premultiplied `rgba` (`width`×`height`, tightly packed) where `coverage` (one byte per
/// pixel) is nonzero, blending by coverage × `opacity`.
pub fn spot_heal(rgba: &mut [u8], coverage: &[u8], width: usize, height: usize, opacity: f32, mode: HealMode, seed: u32) {
    let (w, h) = (width as i64, height as i64);
    let mut b = (w, h, 0i64, 0i64);
    for y in 0..h {
        for x in 0..w {
            if coverage[(y * w + x) as usize] != 0 {
                b = (b.0.min(x), b.1.min(y), b.2.max(x + 1), b.3.max(y + 1));
            }
        }
    }
    if b.2 <= b.0 {
        return;
    }
    let size = (b.2 - b.0).max(b.3 - b.1);
    let ring = (size / 8).clamp(2, 16);
    // Work box: the spot plus its ring, clipped to the image.
    let (wx0, wy0) = ((b.0 - ring).max(0), (b.1 - ring).max(0));
    let (wx1, wy1) = ((b.2 + ring).min(w), (b.3 + ring).min(h));
    let (ww, wh) = (wx1 - wx0, wy1 - wy0);
    let wn = (ww * wh) as usize;
    let mut role = vec![OUTSIDE; wn];
    for y in 0..wh {
        for x in 0..ww {
            if coverage[((wy0 + y) * w + wx0 + x) as usize] != 0 {
                role[(y * ww + x) as usize] = HOLE;
            }
        }
    }
    // The ring: pixels within `ring` of the spot (a square dilation, rows then columns).
    let mut near = vec![false; wn];
    let mut prefix = vec![0i64; ww.max(wh) as usize + 1];
    for y in 0..wh {
        for x in 0..ww {
            prefix[x as usize + 1] = prefix[x as usize] + (role[(y * ww + x) as usize] == HOLE) as i64;
        }
        for x in 0..ww {
            let (lo, hi) = ((x - ring).max(0), (x + ring + 1).min(ww));
            near[(y * ww + x) as usize] = prefix[hi as usize] - prefix[lo as usize] > 0;
        }
    }
    for x in 0..ww {
        for y in 0..wh {
            prefix[y as usize + 1] = prefix[y as usize] + near[(y * ww + x) as usize] as i64;
        }
        for y in 0..wh {
            let (lo, hi) = ((y - ring).max(0), (y + ring + 1).min(wh));
            let p = (y * ww + x) as usize;
            if role[p] == OUTSIDE && prefix[hi as usize] - prefix[lo as usize] > 0 {
                role[p] = RING;
            }
        }
    }
    let ring_count = role.iter().filter(|r| **r == RING).count();
    if ring_count == 0 {
        return;
    }
    let work = (wx0, wy0, ww, wh);

    // The source patch for Content-Aware and Proximity Match.
    let mut offset = None;
    if mode != HealMode::CreateTexture {
        let factors = [1.05, 1.35, 1.75, 2.25, 2.8];
        let count = if mode == HealMode::ProximityMatch { 2 } else { 5 };
        let mut best = f64::INFINITY;
        let mut found = (0, 0);
        for (f, factor) in factors.iter().take(count).enumerate() {
            for a in 0..24 {
                let angle = a as f64 * std::f64::consts::PI / 12.0;
                let dx = (angle.cos() * factor * ww as f64).round() as i64;
                let dy = (angle.sin() * factor * wh as f64).round() as i64;
                let mut s = score(rgba, &role, work, dx, dy, w, h);
                if !s.is_finite() {
                    continue;
                }
                // Nearer patches win ties.
                s *= if mode == HealMode::ProximityMatch { 1.0 + 0.6 * f as f64 } else { 1.0 + 0.1 * f as f64 };
                if s < best {
                    best = s;
                    found = (dx, dy);
                }
            }
        }
        if best.is_finite() {
            // Fine-tune the alignment so repeating texture lines up.
            let (cx, cy) = found;
            let mut refined = score(rgba, &role, work, cx, cy, w, h);
            for j in -3..=3 {
                for i in -3..=3 {
                    let s = score(rgba, &role, work, cx + i, cy + j, w, h);
                    if s < refined {
                        refined = s;
                        found = (cx + i, cy + j);
                    }
                }
            }
            offset = Some(found);
        }
    }

    // Membrane: the border difference between the original and the patch (or the original
    // itself for a smooth fill), spread across the spot.
    let index = |x: i64, y: i64| ((y * w + x) * 4) as usize;
    let mut value = vec![[0f32; 4]; wn];
    let mut mean = [0f64; 4];
    let mut detail = [0f64; 3];
    for y in 0..wh {
        for x in 0..ww {
            let p = (y * ww + x) as usize;
            if role[p] != RING {
                continue;
            }
            let (ix, iy) = (wx0 + x, wy0 + y);
            let t = index(ix, iy);
            for c in 0..4 {
                let s = offset.map_or(0.0, |(ox, oy)| rgba[index(ix + ox, iy + oy) + c] as f32);
                value[p][c] = rgba[t + c] as f32 - s;
                mean[c] += value[p][c] as f64;
            }
            if offset.is_none() {
                // Fine detail around the spot: each pixel against the average of its neighbors.
                for c in 0..3 {
                    let (mut around, mut n) = (0.0, 0);
                    for (nx, ny) in [(ix - 1, iy), (ix + 1, iy), (ix, iy - 1), (ix, iy + 1)] {
                        if nx < 0 || ny < 0 || nx >= w || ny >= h {
                            continue;
                        }
                        around += rgba[index(nx, ny) + c] as f64;
                        n += 1;
                    }
                    if n > 0 {
                        let d = rgba[t + c] as f64 - around / n as f64;
                        detail[c] += d * d;
                    }
                }
            }
        }
    }
    for m in &mut mean {
        *m /= ring_count as f64;
    }
    for (v, r) in value.iter_mut().zip(&role) {
        if *r == HOLE {
            *v = mean.map(|m| m as f32);
        }
    }
    solve(&mut value, &role, ww as usize, wh as usize, 0);
    for d in &mut detail {
        *d = (*d / ring_count as f64).sqrt() * 0.9;
    }

    for y in 0..wh {
        for x in 0..ww {
            let p = (y * ww + x) as usize;
            if role[p] != HOLE {
                continue;
            }
            let (ix, iy) = (wx0 + x, wy0 + y);
            let t = index(ix, iy);
            let amount = coverage[(iy * w + ix) as usize] as f64 / 255.0 * opacity as f64;
            let grain = if offset.is_none() {
                let key = hash(seed ^ hash((iy * w + ix) as u32));
                let (u1, u2) = (unit(key), unit(key ^ 0x68e3_1da4));
                (-2.0 * (1.0 - u1).ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
            } else {
                0.0
            };
            let mut out = [0f64; 4];
            for c in 0..4 {
                let s = offset.map_or(0.0, |(ox, oy)| rgba[index(ix + ox, iy + oy) + c] as f64);
                let healed = s + value[p][c] as f64 + if c < 3 { grain * detail[c] } else { 0.0 };
                out[c] = rgba[t + c] as f64 + (healed - rgba[t + c] as f64) * amount;
            }
            let alpha = out[3].clamp(0.0, 255.0).round();
            rgba[t + 3] = alpha as u8;
            for c in 0..3 {
                rgba[t + c] = out[c].clamp(0.0, alpha).round() as u8;
            }
        }
    }
}

impl SpotHealingTool {
    /// Replaces the preview with the healed pixels.
    fn heal(&mut self, ctx: &mut ToolCtx, stroke: &Stroke) {
        let Some(bounds) = stroke.bounds else { return };
        let surface = &stroke.surface;
        let Original::Rgba(original) = &surface.original else { return };
        // Room for the patch search, which looks up to about three spot-widths away.
        let reach = (((bounds.2 - bounds.0).max(bounds.3 - bounds.1) + 32) as f64 * 3.2) as u32;
        let x0 = bounds.0.saturating_sub(reach);
        let y0 = bounds.1.saturating_sub(reach);
        let x1 = (bounds.2 + reach).min(surface.width);
        let y1 = (bounds.3 + reach).min(surface.height);
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut rgba = vec![0u8; w * h * 4];
        let mut coverage = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x0 + x as u32, y0 + y as u32);
                let p = original.get_pixel(px, py).0;
                let a = p[3] as u32;
                let i = (y * w + x) * 4;
                for c in 0..3 {
                    rgba[i + c] = ((p[c] as u32 * a + 127) / 255) as u8;
                }
                rgba[i + 3] = p[3];
                let inside = px >= bounds.0 && px < bounds.2 && py >= bounds.1 && py < bounds.3;
                if inside {
                    let c = stroke.coverage(px, py) * surface.limit(px, py);
                    coverage[y * w + x] = (c * 255.0 + 0.5).min(255.0) as u8;
                }
            }
        }
        self.seed = self.seed.wrapping_add(0x6d2b_79f5);
        spot_heal(&mut rgba, &coverage, w, h, 1.0, self.mode, self.seed);
        let Some(paint::PixelsMut::Rgba(image)) = surface.pixels_mut(&mut ctx.project.doc) else { return };
        for y in bounds.1..bounds.3 {
            for x in bounds.0..bounds.2 {
                if stroke.coverage(x, y) <= 0.0 {
                    continue;
                }
                let i = (((y - y0) as usize) * w + (x - x0) as usize) * 4;
                let a = rgba[i + 3];
                let out = if a == 0 {
                    [0, 0, 0, 0]
                } else {
                    let un = |v: u8| ((v as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
                    [un(rgba[i]), un(rgba[i + 1]), un(rgba[i + 2]), a]
                };
                image.put_pixel(x, y, image::Rgba(out));
            }
        }
        ctx.project.invalidate(surface.doc_rect(bounds));
    }

    fn handle(&mut self, step: StrokeStep, ctx: &mut ToolCtx) {
        match step {
            StrokeStep::None => {}
            StrokeStep::Painted => {
                if let Some(stroke) = self.driver.stroke.as_mut() {
                    stroke.flush(ctx.project, PREVIEW_OPACITY, &PREVIEW);
                }
            }
            StrokeStep::Finished(mut stroke) => {
                stroke.flush(ctx.project, PREVIEW_OPACITY, &PREVIEW);
                self.heal(ctx, &stroke);
                ctx.project.finish_edit();
            }
        }
    }
}

impl Tool for SpotHealingTool {
    fn kind(&self) -> ToolKind {
        ToolKind::SpotHealing
    }
    fn name(&self) -> &'static str {
        "Spot Healing Brush"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::BANDAIDS
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::J)
    }
    fn options_ui(&mut self, ui: &mut egui::Ui, _ctx: &mut ToolCtx) {
        paint::options_ui(ui, &mut self.settings, Fields { hardness: true, opacity: None, flow: false, smoothing: false });
        ui.separator();
        egui::ComboBox::from_label("Type").selected_text(self.mode.name()).show_ui(ui, |ui| {
            for mode in HealMode::ALL {
                ui.selectable_value(&mut self.mode, mode, mode.name());
            }
        });
    }
    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        let step = self.driver.pointer(event, ctx, &self.settings, "Spot Healing Brush", false);
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

    /// A smooth horizontal ramp with a dark blot in the middle.
    fn blotted(w: usize, h: usize) -> (Vec<u8>, Vec<u8>) {
        let mut rgba = vec![0u8; w * h * 4];
        let mut coverage = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let v = (60 + x * 120 / w) as u8;
                let i = (y * w + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[v, v, v, 255]);
                let (dx, dy) = (x as f64 - w as f64 / 2.0, y as f64 - h as f64 / 2.0);
                if dx.hypot(dy) < 6.0 {
                    rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
                    coverage[y * w + x] = 255;
                } else if dx.hypot(dy) < 8.0 {
                    coverage[y * w + x] = 255;
                }
            }
        }
        (rgba, coverage)
    }

    #[test]
    fn heals_a_blot_from_its_surroundings() {
        let (w, h) = (96, 96);
        for mode in HealMode::ALL {
            let (mut rgba, coverage) = blotted(w, h);
            spot_heal(&mut rgba, &coverage, w, h, 1.0, mode, 7);
            let center = (h / 2 * w + w / 2) * 4;
            let expected = (60 + (w / 2) * 120 / w) as i32;
            assert!((rgba[center] as i32 - expected).abs() < 25, "{mode:?}: {} vs {expected}", rgba[center]);
            assert_eq!(rgba[center + 3], 255);
            // Untouched outside the spot.
            assert_eq!(rgba[0], 60);
        }
    }
}
