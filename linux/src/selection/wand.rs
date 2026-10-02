//! The Magic Wand's pixel matching (after `WandPixels.c`): pixels whose premultiplied RGBA
//! channels each lie within a tolerance of a reference color, everywhere or only those connected
//! to the clicked one. Select > Grow and Similar reuse it.

use std::sync::Mutex;

use image::{GrayImage, RgbaImage};
use rayon::prelude::*;

use crate::doc::Document;
use crate::render::{self, Region, RenderCache};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WandSettings {
    /// How far (0–255) each channel may differ from the sampled color and still be selected.
    pub tolerance: u8,
    /// Pixels either side of the click averaged into the reference color (0, 1 or 2).
    pub sample_radius: u32,
    pub contiguous: bool,
    pub anti_alias: bool,
    /// Read the visible composite rather than just the active layer.
    pub sample_all_layers: bool,
}

impl Default for WandSettings {
    fn default() -> Self {
        DEFAULT
    }
}

const DEFAULT: WandSettings =
    WandSettings { tolerance: 32, sample_radius: 0, contiguous: true, anti_alias: true, sample_all_layers: false };

/// The wand's settings live here so Select > Grow and Similar use the same ones.
static SETTINGS: Mutex<WandSettings> = Mutex::new(DEFAULT);

pub fn settings() -> WandSettings {
    SETTINGS.lock().map(|s| *s).unwrap_or_default()
}

pub fn set_settings(settings: WandSettings) {
    if let Ok(mut s) = SETTINGS.lock() {
        *s = settings;
    }
}

/// What the wand reads, at canvas size and premultiplied: the visible composite, or the active
/// layer's own pixels (a folder or blank layer reads as transparent).
pub fn sample_image(doc: &Document, all_layers: bool, cache: &RenderCache) -> Option<RgbaImage> {
    let region = Region::full(doc);
    let buffer = if all_layers {
        render::composite(doc, region, cache)
    } else {
        match doc.active_layer().filter(|l| !l.is_group && l.adjustment.is_none()) {
            Some(layer) => render::place_layer(layer, region, cache).unwrap_or_else(|| render::Buffer::new(region.width, region.height)),
            None => return None,
        }
    };
    let mut out = RgbaImage::new(doc.width, doc.height);
    out.as_mut().par_chunks_mut(4).zip(buffer.px.par_iter()).for_each(|(o, p)| {
        for c in 0..4 {
            o[c] = render::to_u8(p[c]);
        }
    });
    Some(out)
}

#[inline]
fn matches(p: &[u8], low: &[i32; 4], high: &[i32; 4]) -> bool {
    (0..4).all(|c| (low[c]..=high[c]).contains(&(p[c] as i32)))
}

/// The average color of the `(2r+1)²` square around `(x, y)`, clipped to the image.
fn reference(image: &RgbaImage, x: u32, y: u32, radius: u32) -> [i32; 4] {
    let (w, h) = image.dimensions();
    let (x0, x1) = (x.saturating_sub(radius), (x + radius).min(w - 1));
    let (y0, y1) = (y.saturating_sub(radius), (y + radius).min(h - 1));
    let mut sums = [0u64; 4];
    let mut n = 0u64;
    for yy in y0..=y1 {
        for xx in x0..=x1 {
            let p = image.get_pixel(xx, yy);
            for c in 0..4 {
                sums[c] += p[c] as u64;
            }
            n += 1;
        }
    }
    sums.map(|s| ((s + n / 2) / n) as i32)
}

/// Pixels within `tolerance` of the color at `seed` (averaged over `radius`): 255 selected, 0 not.
pub fn wand_mask(image: &RgbaImage, seed: (u32, u32), radius: u32, tolerance: i32, contiguous: bool) -> GrayImage {
    let (w, h) = image.dimensions();
    let mut mask = GrayImage::new(w, h);
    if seed.0 >= w || seed.1 >= h {
        return mask;
    }
    let r = reference(image, seed.0, seed.1, radius);
    let low = r.map(|v| v - tolerance);
    let high = r.map(|v| v + tolerance);
    if contiguous {
        flood(image, &mut mask, &[seed], &low, &high);
    } else {
        select_matching(image, &mut mask, &low, &high);
    }
    mask
}

fn select_matching(image: &RgbaImage, mask: &mut GrayImage, low: &[i32; 4], high: &[i32; 4]) {
    let w = image.width() as usize;
    let raw: &mut [u8] = mask;
    raw.par_chunks_mut(w).zip(image.as_raw().par_chunks(w * 4)).for_each(|(out, row)| {
        for (o, p) in out.iter_mut().zip(row.chunks_exact(4)) {
            if matches(p, low, high) {
                *o = 255;
            }
        }
    });
}

/// Scanline flood fill from `seeds`, marking matching pixels 255. Pixels already marked count
/// as filled; a seed among them grows from its edge.
fn flood(image: &RgbaImage, mask: &mut GrayImage, seeds: &[(u32, u32)], low: &[i32; 4], high: &[i32; 4]) {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let px = image.as_raw();
    let out: &mut [u8] = mask;
    let ok = |x: usize, y: usize| matches(&px[(y * w + x) * 4..(y * w + x) * 4 + 4], low, high);
    let mut stack: Vec<(usize, usize)> = Vec::with_capacity(4096);
    let push_neighbors = |out: &[u8], stack: &mut Vec<(usize, usize)>, left: usize, right: usize, y: usize| {
        for ny in [y.wrapping_sub(1), y + 1] {
            if ny >= h {
                continue;
            }
            let mut in_run = false;
            for nx in left..=right {
                let candidate = out[ny * w + nx] == 0 && ok(nx, ny);
                if candidate && !in_run {
                    stack.push((nx, ny));
                }
                in_run = candidate;
            }
        }
    };
    for &(sx, sy) in seeds {
        let (sx, sy) = (sx as usize, sy as usize);
        if out[sy * w + sx] != 0 {
            // A seed inside an existing selection grows into its unselected neighbors.
            for (nx, ny) in [(sx.wrapping_sub(1), sy), (sx + 1, sy), (sx, sy.wrapping_sub(1)), (sx, sy + 1)] {
                if nx < w && ny < h && out[ny * w + nx] == 0 {
                    stack.push((nx, ny));
                }
            }
        } else {
            stack.push((sx, sy));
        }
        while let Some((x, y)) = stack.pop() {
            if out[y * w + x] != 0 || !ok(x, y) {
                continue;
            }
            let mut left = x;
            let mut right = x;
            while left > 0 && out[y * w + left - 1] == 0 && ok(left - 1, y) {
                left -= 1;
            }
            while right + 1 < w && out[y * w + right + 1] == 0 && ok(right + 1, y) {
                right += 1;
            }
            out[y * w + left..=y * w + right].fill(255);
            push_neighbors(out, &mut stack, left, right, y);
        }
    }
}

/// Select > Grow (`contiguous`) or Similar: pixels whose colors fall within `tolerance` of the
/// range of colors already selected, touching the selection or anywhere. The result includes
/// the original selection.
pub fn extend_similar(image: &RgbaImage, selection: &GrayImage, tolerance: i32, contiguous: bool) -> GrayImage {
    let (w, h) = image.dimensions();
    // The selected colors' range per channel, ignoring the extreme half percent at each end.
    let mut histograms = [[0u64; 256]; 4];
    let mut count = 0u64;
    for (p, s) in image.pixels().zip(selection.pixels()) {
        if s[0] >= 128 {
            for c in 0..4 {
                histograms[c][p[c] as usize] += 1;
            }
            count += 1;
        }
    }
    let mut out = GrayImage::new(w, h);
    if count == 0 {
        return out;
    }
    let skip = count / 200;
    let mut low = [0i32; 4];
    let mut high = [255i32; 4];
    for c in 0..4 {
        let mut seen = 0;
        low[c] = (0..256).find(|v| {
            seen += histograms[c][*v];
            seen > skip
        }).unwrap_or(0) as i32
            - tolerance;
        seen = 0;
        high[c] = (0..256).rev().find(|v| {
            seen += histograms[c][*v];
            seen > skip
        }).unwrap_or(255) as i32
            + tolerance;
    }
    if contiguous {
        let raw: &mut [u8] = &mut out;
        let mut seeds = Vec::new();
        for (i, (o, s)) in raw.iter_mut().zip(selection.as_raw()).enumerate() {
            if *s >= 128 {
                *o = 255;
                // Only the selection's edge needs to grow outward.
                let (x, y) = (i % w as usize, i / w as usize);
                let edge = [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)]
                    .iter()
                    .any(|&(nx, ny)| nx < w as usize && ny < h as usize && selection.as_raw()[ny * w as usize + nx] < 128);
                if edge {
                    seeds.push((x as u32, y as u32));
                }
            }
        }
        flood(image, &mut out, &seeds, &low, &high);
    } else {
        select_matching(image, &mut out, &low, &high);
    }
    // Keep the original's soft edge where it reaches further.
    let raw: &mut [u8] = &mut out;
    raw.par_iter_mut().zip(selection.as_raw().par_iter()).for_each(|(o, s)| *o = (*o).max(*s));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::selection::tight_bounds;
    use image::{Luma, Rgba};

    /// Two red squares on white, apart.
    fn two_squares() -> RgbaImage {
        RgbaImage::from_fn(30, 10, |x, y| {
            let red = (2..8).contains(&y) && ((2..8).contains(&x) || (20..28).contains(&x));
            if red { Rgba([200, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) }
        })
    }

    #[test]
    fn contiguous_selects_only_the_clicked_region() {
        let image = two_squares();
        let mask = wand_mask(&image, (4, 4), 0, 32, true);
        assert_eq!(tight_bounds(&mask), Some((2, 2, 8, 8)));
        let everywhere = wand_mask(&image, (4, 4), 0, 32, false);
        assert_eq!(tight_bounds(&everywhere), Some((2, 2, 28, 8)));
    }

    #[test]
    fn contiguous_fills_around_obstacles() {
        // A U-shaped white region: the fill has to go down, across and back up.
        let image = RgbaImage::from_fn(9, 9, |x, y| {
            let wall = x == 4 && y < 7;
            if wall { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) }
        });
        let mask = wand_mask(&image, (0, 0), 0, 10, true);
        assert_eq!(mask.get_pixel(8, 0)[0], 255);
        assert_eq!(mask.get_pixel(4, 3)[0], 0);
        assert_eq!(mask.pixels().filter(|p| p[0] == 255).count(), 81 - 7);
    }

    #[test]
    fn tolerance_widens_the_match() {
        let image = RgbaImage::from_fn(10, 1, |x, _| Rgba([x as u8 * 10, 0, 0, 255]));
        let narrow = wand_mask(&image, (0, 0), 0, 15, true);
        assert_eq!(tight_bounds(&narrow), Some((0, 0, 2, 1)));
        let wide = wand_mask(&image, (0, 0), 0, 45, true);
        assert_eq!(tight_bounds(&wide), Some((0, 0, 5, 1)));
    }

    #[test]
    fn grow_and_similar() {
        let image = two_squares();
        let mut selection = GrayImage::new(30, 10);
        selection.put_pixel(4, 4, Luma([255]));
        let grown = extend_similar(&image, &selection, 10, true);
        assert_eq!(tight_bounds(&grown), Some((2, 2, 8, 8)));
        let similar = extend_similar(&image, &selection, 10, false);
        assert_eq!(tight_bounds(&similar), Some((2, 2, 28, 8)));
    }
}
