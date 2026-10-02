//! Select > Modify: Expand and Contract (exact Euclidean distance transforms, so corners round
//! as in Photoshop), Feather (a Gaussian from three box blurs), Border and Smooth. Each works on
//! the selection's bounds plus the margin it can reach, with rows and columns in parallel.

use std::sync::Mutex;

use image::GrayImage;
use rayon::prelude::*;

use crate::doc::Selection;
use crate::ui::dialogs::{ok_cancel, Dialog, DialogCtx, DialogState};

/// Stands for "no feature pixel in this row or column" in the distance transform.
const FAR: f64 = 1e12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifyOp {
    Expand,
    Contract,
    Feather,
    Border,
    Smooth,
}

impl ModifyOp {
    pub const ALL: [ModifyOp; 5] = [ModifyOp::Expand, ModifyOp::Contract, ModifyOp::Feather, ModifyOp::Border, ModifyOp::Smooth];

    pub fn name(self) -> &'static str {
        match self {
            ModifyOp::Expand => "Expand",
            ModifyOp::Contract => "Contract",
            ModifyOp::Feather => "Feather",
            ModifyOp::Border => "Border",
            ModifyOp::Smooth => "Smooth",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ModifyOp::Expand => "Expand By",
            ModifyOp::Contract => "Contract By",
            ModifyOp::Feather => "Feather Radius",
            ModifyOp::Border => "Width",
            ModifyOp::Smooth => "Sample Radius",
        }
    }

    fn range(self) -> std::ops::RangeInclusive<f64> {
        match self {
            ModifyOp::Feather => 0.1..=1000.0,
            ModifyOp::Border => 1.0..=200.0,
            ModifyOp::Smooth => 1.0..=500.0,
            _ => 1.0..=500.0,
        }
    }

    /// The selection's coverage after the operation, canvas-sized.
    pub fn apply(self, mask: &GrayImage, amount: f64) -> GrayImage {
        match self {
            ModifyOp::Expand => expand(mask, amount),
            ModifyOp::Contract => contract(mask, amount),
            ModifyOp::Feather => feather(mask, amount),
            ModifyOp::Border => border(mask, amount),
            ModifyOp::Smooth => smooth(mask, amount.round().max(1.0) as usize),
        }
    }
}

/// The last amount used for each operation, so the dialogs reopen with it.
static LAST_AMOUNTS: Mutex<[f64; 5]> = Mutex::new([5.0, 5.0, 5.0, 10.0, 5.0]);

// ---------------------------------------------------------------------------------------------
// Regions

/// A rectangle `(x0, y0, x1, y1)` of document pixels; it may reach past the canvas.
type Area = (i64, i64, i64, i64);

fn grown(bounds: (u32, u32, u32, u32), by: i64) -> Area {
    (bounds.0 as i64 - by, bounds.1 as i64 - by, bounds.2 as i64 + by, bounds.3 as i64 + by)
}

fn clipped(area: Area, w: u32, h: u32) -> Area {
    (area.0.max(0), area.1.max(0), area.2.min(w as i64), area.3.min(h as i64))
}

/// The mask's values over `area`, with 0 beyond the canvas.
fn read_area(mask: &GrayImage, area: Area) -> Vec<u8> {
    let (w, h) = mask.dimensions();
    let aw = (area.2 - area.0) as usize;
    let mut out = vec![0u8; aw * (area.3 - area.1) as usize];
    out.par_chunks_mut(aw).enumerate().for_each(|(j, row)| {
        let y = area.1 + j as i64;
        if y < 0 || y >= h as i64 {
            return;
        }
        let src = &mask.as_raw()[(y as usize) * w as usize..(y as usize + 1) * w as usize];
        for (i, v) in row.iter_mut().enumerate() {
            let x = area.0 + i as i64;
            if x >= 0 && x < w as i64 {
                *v = src[x as usize];
            }
        }
    });
    out
}

/// A canvas-sized copy of `mask` with `area` (inside the canvas) replaced by `values` through `f`.
fn write_area(mask: &GrayImage, area: Area, values: &[f32], f: impl Fn(u8, f32) -> u8 + Sync) -> GrayImage {
    let (w, _) = mask.dimensions();
    let mut out = mask.clone();
    let aw = (area.2 - area.0) as usize;
    let rows = (area.3 - area.1) as usize;
    let raw: &mut [u8] = &mut out;
    raw.par_chunks_mut(w as usize).skip(area.1 as usize).take(rows).enumerate().for_each(|(j, row)| {
        let src = &values[j * aw..(j + 1) * aw];
        for (i, v) in src.iter().enumerate() {
            let p = &mut row[area.0 as usize + i];
            *p = f(*p, *v);
        }
    });
    out
}

// ---------------------------------------------------------------------------------------------
// Distance transform (Felzenszwalb & Huttenlocher)

/// Squared distance from each sample to the nearest parabola base: `out[q] = min_p (q-p)² + f[p]`.
fn distance_1d(f: &[f64], out: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let fq = f[q] + (q * q) as f64;
        loop {
            let p = v[k];
            let s = (fq - (f[p] + (p * p) as f64)) / (2.0 * (q - p) as f64);
            if s <= z[k] {
                // z[0] is -∞, so this never runs past the first parabola.
                k -= 1;
                continue;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f64::INFINITY;
            break;
        }
    }
    k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        let d = q as f64 - p as f64;
        *o = d * d + f[p];
    }
}

/// Squared Euclidean distance from every pixel of a `w`×`h` grid to the nearest pixel where
/// `feature` is true (`FAR` or more when there is none).
fn distance_sq(feature: &[bool], w: usize, h: usize) -> Vec<f32> {
    // Columns first, stored transposed so each column is contiguous.
    let mut columns = vec![0f32; w * h];
    columns.par_chunks_mut(h).enumerate().for_each_init(
        || (vec![0f64; h], vec![0f64; h], vec![0usize; h], vec![0f64; h + 1]),
        |(f, out, v, z), (x, column)| {
            for y in 0..h {
                f[y] = if feature[y * w + x] { 0.0 } else { FAR };
            }
            distance_1d(f, out, v, z);
            for y in 0..h {
                column[y] = out[y] as f32;
            }
        },
    );
    let mut result = vec![0f32; w * h];
    result.par_chunks_mut(w).enumerate().for_each_init(
        || (vec![0f64; w], vec![0f64; w], vec![0usize; w], vec![0f64; w + 1]),
        |(f, out, v, z), (y, row)| {
            for x in 0..w {
                f[x] = columns[x * h + y] as f64;
            }
            distance_1d(f, out, v, z);
            for x in 0..w {
                row[x] = out[x] as f32;
            }
        },
    );
    result
}

// ---------------------------------------------------------------------------------------------
// Operations

/// Grows the selection by `radius` pixels with round corners; the new edge is anti-aliased.
pub fn expand(mask: &GrayImage, radius: f64) -> GrayImage {
    let Some(bounds) = crate::doc::selection::tight_bounds(mask) else { return mask.clone() };
    let (w, h) = mask.dimensions();
    let area = clipped(grown(bounds, radius.ceil() as i64 + 1), w, h);
    let (aw, ah) = ((area.2 - area.0) as usize, (area.3 - area.1) as usize);
    let values = read_area(mask, area);
    let feature: Vec<bool> = values.par_iter().map(|v| *v >= 128).collect();
    let d2 = distance_sq(&feature, aw, ah);
    let coverage: Vec<f32> = d2.par_iter().map(|d| (radius as f32 + 1.0 - d.sqrt()).clamp(0.0, 1.0)).collect();
    write_area(mask, area, &coverage, |old, c| old.max((c * 255.0).round() as u8))
}

/// Shrinks the selection by `radius` pixels, the canvas edge counting as unselected.
pub fn contract(mask: &GrayImage, radius: f64) -> GrayImage {
    let Some(bounds) = crate::doc::selection::tight_bounds(mask) else { return mask.clone() };
    let (w, h) = mask.dimensions();
    // One pixel past the bounds (possibly off the canvas) is unselected, so every inside pixel
    // finds its distance to the outside within the area.
    let area = grown(bounds, 1);
    let (aw, ah) = ((area.2 - area.0) as usize, (area.3 - area.1) as usize);
    let values = read_area(mask, area);
    let feature: Vec<bool> = values.par_iter().map(|v| *v < 128).collect();
    let d2 = distance_sq(&feature, aw, ah);
    // Keep the canvas part of the area.
    let inner = clipped(area, w, h);
    let (iw, ih) = ((inner.2 - inner.0) as usize, (inner.3 - inner.1) as usize);
    let (ox, oy) = ((inner.0 - area.0) as usize, (inner.1 - area.1) as usize);
    let mut coverage = vec![0f32; iw * ih];
    coverage.par_chunks_mut(iw).enumerate().for_each(|(j, row)| {
        let src = &d2[(j + oy) * aw + ox..(j + oy) * aw + ox + iw];
        for (c, d) in row.iter_mut().zip(src) {
            *c = (d.sqrt() - radius as f32).clamp(0.0, 1.0);
        }
    });
    write_area(mask, inner, &coverage, |old, c| old.min((c * 255.0).round() as u8))
}

/// Softens the edge with a Gaussian of standard deviation `radius / 2`, as the macOS app does.
pub fn feather(mask: &GrayImage, radius: f64) -> GrayImage {
    let Some(bounds) = crate::doc::selection::tight_bounds(mask) else { return mask.clone() };
    let sigma = radius / 2.0;
    let (w, h) = mask.dimensions();
    // Clipped to the canvas, so the edge pixels repeat outward: Select All stays whole.
    let area = clipped(grown(bounds, (sigma * 3.0).ceil() as i64 + 2), w, h);
    let (aw, ah) = ((area.2 - area.0) as usize, (area.3 - area.1) as usize);
    let mut values: Vec<f32> = read_area(mask, area).par_iter().map(|v| *v as f32).collect();
    gaussian_blur(&mut values, aw, ah, sigma);
    write_area(mask, area, &values, |_, v| v.round().clamp(0.0, 255.0) as u8)
}

/// A band `width` pixels wide straddling the selection's edge.
pub fn border(mask: &GrayImage, width: f64) -> GrayImage {
    let outer = expand(mask, width / 2.0);
    let inner = contract(mask, width / 2.0);
    let mut out = outer;
    let raw: &mut [u8] = &mut out;
    raw.par_iter_mut().zip(inner.as_raw().par_iter()).for_each(|(o, i)| {
        *o = ((*o as u32 * (255 - *i as u32) + 127) / 255) as u8;
    });
    out
}

/// Each pixel takes the majority of the square around it: specks vanish and corners round.
pub fn smooth(mask: &GrayImage, radius: usize) -> GrayImage {
    let Some(bounds) = crate::doc::selection::tight_bounds(mask) else { return mask.clone() };
    let (w, h) = mask.dimensions();
    let area = clipped(grown(bounds, radius as i64 + 1), w, h);
    let (aw, ah) = ((area.2 - area.0) as usize, (area.3 - area.1) as usize);
    let mut values: Vec<f32> = read_area(mask, area).par_iter().map(|v| *v as f32 / 255.0).collect();
    box_blur(&mut values, aw, ah, radius);
    // A steep ramp around one half keeps a pixel's worth of anti-aliasing.
    write_area(mask, area, &values, |_, v| (((v - 0.5) * 4.0 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// A light [1 2 1] blur over the selection's edge: the Magic Wand's anti-aliasing.
pub fn soften_edges(mask: &mut GrayImage) {
    let Some(bounds) = crate::doc::selection::tight_bounds(mask) else { return };
    let (w, h) = mask.dimensions();
    let area = clipped(grown(bounds, 1), w, h);
    let (aw, ah) = ((area.2 - area.0) as usize, (area.3 - area.1) as usize);
    let values = read_area(mask, area);
    let mut horizontal = vec![0u16; aw * ah];
    horizontal.par_chunks_mut(aw).enumerate().for_each(|(j, row)| {
        let src = &values[j * aw..(j + 1) * aw];
        for i in 0..aw {
            let l = src[i.saturating_sub(1)] as u16;
            let r = src[(i + 1).min(aw - 1)] as u16;
            row[i] = l + 2 * src[i] as u16 + r;
        }
    });
    let mut out = vec![0f32; aw * ah];
    out.par_chunks_mut(aw).enumerate().for_each(|(j, row)| {
        let up = &horizontal[j.saturating_sub(1) * aw..][..aw];
        let mid = &horizontal[j * aw..][..aw];
        let down = &horizontal[(j + 1).min(ah - 1) * aw..][..aw];
        for i in 0..aw {
            row[i] = (up[i] + 2 * mid[i] + down[i]) as f32 / 16.0;
        }
    });
    *mask = write_area(mask, area, &out, |_, v| v.round().clamp(0.0, 255.0) as u8);
}

// ---------------------------------------------------------------------------------------------
// Blurs

/// Box widths whose three passes approximate a Gaussian of `sigma`.
fn gaussian_boxes(sigma: f64) -> [usize; 3] {
    let n = 3.0;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let m = ((12.0 * sigma * sigma - n * (wl * wl) as f64 - 4.0 * n * wl as f64 - 3.0 * n) / (-4.0 * wl as f64 - 4.0)).round() as i64;
    let mut radii = [0usize; 3];
    for (i, r) in radii.iter_mut().enumerate() {
        let width = if (i as i64) < m { wl } else { wu };
        *r = ((width - 1) / 2) as usize;
    }
    radii
}

fn gaussian_blur(values: &mut [f32], w: usize, h: usize, sigma: f64) {
    if sigma <= 0.0 {
        return;
    }
    for radius in gaussian_boxes(sigma) {
        box_blur(values, w, h, radius);
    }
}

/// A (2r+1)² box blur, separable, repeating the edge pixels outward.
fn box_blur(values: &mut [f32], w: usize, h: usize, radius: usize) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    blur_rows(values, w, radius);
    let mut t = transpose(values, w, h);
    blur_rows(&mut t, h, radius);
    let back = transpose(&t, h, w);
    values.copy_from_slice(&back);
}

fn blur_rows(values: &mut [f32], w: usize, radius: usize) {
    values.par_chunks_mut(w).for_each_init(
        || vec![0f64; w + 1],
        |prefix, row| {
            for i in 0..w {
                prefix[i + 1] = prefix[i] + row[i] as f64;
            }
            let (first, last) = (row[0] as f64, row[w - 1] as f64);
            // Sum of the row extended by repeating its ends, over indices ..= i.
            let sum_to = |i: i64| -> f64 {
                if i < 0 {
                    (i + 1) as f64 * first
                } else if i >= w as i64 {
                    prefix[w] + (i - w as i64 + 1) as f64 * last
                } else {
                    prefix[i as usize + 1]
                }
            };
            let r = radius as i64;
            let scale = 1.0 / (2 * radius + 1) as f64;
            for (i, v) in row.iter_mut().enumerate() {
                let i = i as i64;
                *v = ((sum_to(i + r) - sum_to(i - r - 1)) * scale) as f32;
            }
        },
    );
}

fn transpose(values: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(h).enumerate().for_each(|(x, column)| {
        for (y, v) in column.iter_mut().enumerate() {
            *v = values[y * w + x];
        }
    });
    out
}

// ---------------------------------------------------------------------------------------------

/// Select > Modify > Expand…, Contract…, Feather…, Border…, Smooth…
pub struct ModifyDialog {
    op: ModifyOp,
    amount: f64,
}

impl ModifyDialog {
    pub fn new(op: ModifyOp) -> Self {
        let index = ModifyOp::ALL.iter().position(|o| *o == op).unwrap_or(0);
        let amount = LAST_AMOUNTS.lock().map(|a| a[index]).unwrap_or(5.0);
        ModifyDialog { op, amount }
    }
}

impl Dialog for ModifyDialog {
    fn title(&self) -> String {
        format!("{} Selection", self.op.name())
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut DialogCtx) -> DialogState {
        ui.horizontal(|ui| {
            ui.label(self.op.label());
            let speed = if self.op == ModifyOp::Feather { 0.1 } else { 1.0 };
            let decimals = if self.op == ModifyOp::Feather { 1 } else { 0 };
            ui.add(egui::DragValue::new(&mut self.amount).range(self.op.range()).speed(speed).max_decimals(decimals).suffix(" px"));
        });
        match ok_cancel(ui, "OK") {
            Some(true) => {
                if self.op != ModifyOp::Feather {
                    self.amount = self.amount.round();
                }
                if let Ok(mut last) = LAST_AMOUNTS.lock() {
                    last[ModifyOp::ALL.iter().position(|o| *o == self.op).unwrap_or(0)] = self.amount;
                }
                let Some(project) = ctx.project.as_deref_mut() else { return DialogState::Closed };
                let Some(selection) = project.doc.selection.clone() else { return DialogState::Closed };
                let result = Selection::from_mask(self.op.apply(&selection.mask, self.amount));
                if result.is_none() {
                    *ctx.status = Some("No pixels are selected after that change.".into());
                }
                project.edit(&format!("{} Selection", self.op.name()), |doc| doc.selection = result);
                DialogState::Closed
            }
            Some(false) => DialogState::Closed,
            None => DialogState::Open,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::selection::tight_bounds;
    use image::Luma;

    fn square(size: u32, r: (u32, u32, u32, u32)) -> GrayImage {
        GrayImage::from_fn(size, size, |x, y| Luma([if x >= r.0 && x < r.2 && y >= r.1 && y < r.3 { 255 } else { 0 }]))
    }

    #[test]
    fn distance_transform_matches_brute_force() {
        let (w, h) = (13, 9);
        let feature: Vec<bool> = (0..w * h).map(|i| i % 17 == 3 || i == 50).collect();
        let d = distance_sq(&feature, w, h);
        for y in 0..h {
            for x in 0..w {
                let mut best = f64::MAX;
                for (i, f) in feature.iter().enumerate() {
                    if *f {
                        let (fx, fy) = ((i % w) as f64, (i / w) as f64);
                        best = best.min((fx - x as f64).powi(2) + (fy - y as f64).powi(2));
                    }
                }
                assert_eq!(d[y * w + x] as f64, best, "at {x},{y}");
            }
        }
    }

    #[test]
    fn expand_and_contract() {
        let mask = square(40, (10, 10, 20, 20));
        let grown = expand(&mask, 3.0);
        assert_eq!(tight_bounds(&grown), Some((7, 7, 23, 23)));
        // Straight edges stay hard; corners round off.
        assert_eq!(grown.get_pixel(7, 15)[0], 255);
        assert_eq!(grown.get_pixel(6, 15)[0], 0);
        assert!(grown.get_pixel(7, 7)[0] < 255);
        let shrunk = contract(&mask, 3.0);
        assert_eq!(tight_bounds(&shrunk), Some((13, 13, 17, 17)));
        assert_eq!(shrunk.get_pixel(13, 13)[0], 255);
        // Contracting by half the width or more leaves nothing.
        assert!(tight_bounds(&contract(&mask, 5.0)).is_none());
    }

    #[test]
    fn contract_counts_the_canvas_edge() {
        let all = GrayImage::from_pixel(20, 20, Luma([255]));
        assert_eq!(tight_bounds(&contract(&all, 4.0)), Some((4, 4, 16, 16)));
    }

    #[test]
    fn feather_softens_and_keeps_select_all() {
        let mask = square(60, (20, 20, 40, 40));
        let soft = feather(&mask, 8.0);
        assert!(soft.get_pixel(30, 30)[0] > 250);
        let edge = soft.get_pixel(20, 30)[0];
        assert!((100..=160).contains(&edge), "edge {edge}");
        assert!(soft.get_pixel(17, 30)[0] > 0);
        let all = GrayImage::from_pixel(20, 20, Luma([255]));
        assert!(feather(&all, 6.0).pixels().all(|p| p[0] == 255));
    }

    #[test]
    fn border_is_a_ring() {
        let mask = square(60, (20, 20, 40, 40));
        let ring = border(&mask, 6.0);
        assert_eq!(ring.get_pixel(30, 30)[0], 0);
        assert_eq!(ring.get_pixel(20, 30)[0], 255);
        assert_eq!(ring.get_pixel(18, 30)[0], 255);
        assert_eq!(ring.get_pixel(10, 30)[0], 0);
    }

    #[test]
    fn smooth_removes_specks() {
        let mut mask = square(40, (10, 10, 30, 30));
        mask.put_pixel(2, 2, Luma([255]));
        let smoothed = smooth(&mask, 2);
        assert_eq!(smoothed.get_pixel(2, 2)[0], 0);
        assert_eq!(smoothed.get_pixel(20, 20)[0], 255);
    }

    #[test]
    fn box_blur_keeps_flat_areas() {
        let mut values = vec![3.0f32; 7 * 5];
        box_blur(&mut values, 7, 5, 2);
        assert!(values.iter().all(|v| (v - 3.0).abs() < 1e-5));
    }
}
