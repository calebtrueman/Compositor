//! Sampling layer images and masks through their transforms.

use std::sync::Arc;

use image::{GrayImage, RgbaImage};

use super::{Region, RenderCache};
use crate::doc::{Affine, LayerTransform, Sampling};

enum Source {
    Rgba(Arc<RgbaImage>),
    Gray(Arc<GrayImage>),
    Uniform(f32),
}

/// Maps output pixels back into a source image and reads premultiplied RGBA (for gray sources,
/// coverage in all four channels).
pub struct Sampler {
    source: Source,
    /// Document → source pixel coordinates.
    inverse: Affine,
    width: f64,
    height: f64,
    nearest: bool,
    /// Integer offset when the image lands 1:1 on whole output pixels.
    aligned: Option<(i64, i64)>,
}

impl Sampler {
    pub fn new(image: &Arc<RgbaImage>, transform: &LayerTransform, scale: f64, cache: &RenderCache) -> Option<Self> {
        let (w, h) = image.dimensions();
        let level = mip_level(transform, w, h, scale);
        let source = cache.mip(image, level);
        let (sw, sh) = source.dimensions();
        Sampler::build(Source::Rgba(source), transform, sw, sh, scale, (w, h))
    }

    pub fn gray(image: &Arc<GrayImage>, transform: &LayerTransform, scale: f64, _cache: &RenderCache) -> Option<Self> {
        let (w, h) = image.dimensions();
        Sampler::build(Source::Gray(image.clone()), transform, w, h, scale, (w, h))
    }

    pub fn uniform(value: u8, transform: &LayerTransform) -> Self {
        Sampler::build(Source::Uniform(value as f32 / 255.0), transform, 1, 1, 1.0, (1, 1)).unwrap()
    }

    fn build(source: Source, t: &LayerTransform, w: u32, h: u32, scale: f64, full: (u32, u32)) -> Option<Self> {
        let inverse = t.pixel_to_document(w, h).inverse()?;
        let nearest = t.sampling == Sampling::Nearest || matches!(source, Source::Uniform(_));
        let aligned = (scale == 1.0 && full == (w, h) && t.is_pixel_aligned(w, h))
            .then(|| (t.origin[0] as i64, t.origin[1] as i64));
        Some(Sampler { source, inverse, width: w as f64, height: h as f64, nearest, aligned })
    }

    /// Fills `row` (output row `j` of `region`) with premultiplied samples.
    pub fn sample_row(&self, region: Region, j: usize, row: &mut [[f32; 4]]) {
        if let Some((ox, oy)) = self.aligned {
            if region.scale == 1.0 {
                let sy = region.y + j as i64 - oy;
                for (i, out) in row.iter_mut().enumerate() {
                    let sx = region.x + i as i64 - ox;
                    *out = self.texel(sx, sy);
                }
                return;
            }
        }
        let (x0, y0) = region.doc_point(0, j);
        let (mut u, mut v) = self.inverse.apply(x0, y0);
        let (du, dv) = self.inverse.apply_vector(region.scale, 0.0);
        for out in row.iter_mut() {
            *out = if u < 0.0 || v < 0.0 || u >= self.width || v >= self.height {
                [0.0; 4]
            } else if self.nearest {
                self.texel(u as i64, v as i64)
            } else {
                self.bilinear(u - 0.5, v - 0.5)
            };
            u += du;
            v += dv;
        }
    }

    #[inline]
    fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        match &self.source {
            Source::Uniform(v) => {
                if x == 0 && y == 0 {
                    [*v; 4]
                } else {
                    [0.0; 4]
                }
            }
            Source::Rgba(image) => {
                if x < 0 || y < 0 || x >= image.width() as i64 || y >= image.height() as i64 {
                    return [0.0; 4];
                }
                let p = image.get_pixel(x as u32, y as u32).0;
                let a = p[3] as f32 / 255.0;
                [p[0] as f32 / 255.0 * a, p[1] as f32 / 255.0 * a, p[2] as f32 / 255.0 * a, a]
            }
            Source::Gray(image) => {
                if x < 0 || y < 0 || x >= image.width() as i64 || y >= image.height() as i64 {
                    return [0.0; 4];
                }
                [image.get_pixel(x as u32, y as u32).0[0] as f32 / 255.0; 4]
            }
        }
    }

    /// Clamped-edge bilinear sampling, so a layer's own edge pixels aren't faded by the
    /// transparent outside; the caller has already rejected points outside the image.
    #[inline]
    fn bilinear(&self, x: f64, y: f64) -> [f32; 4] {
        let max_x = self.width as i64 - 1;
        let max_y = self.height as i64 - 1;
        let fx = x.floor();
        let fy = y.floor();
        let tx = (x - fx) as f32;
        let ty = (y - fy) as f32;
        let x0 = (fx as i64).clamp(0, max_x);
        let y0 = (fy as i64).clamp(0, max_y);
        let x1 = (fx as i64 + 1).clamp(0, max_x);
        let y1 = (fy as i64 + 1).clamp(0, max_y);
        let a = self.texel(x0, y0);
        let b = self.texel(x1, y0);
        let c = self.texel(x0, y1);
        let d = self.texel(x1, y1);
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bottom = c[i] + (d[i] - c[i]) * tx;
            out[i] = top + (bottom - top) * ty;
        }
        out
    }
}

/// How many times to halve a source before sampling, so each output pixel reads about one texel.
fn mip_level(t: &LayerTransform, w: u32, h: u32, scale: f64) -> usize {
    if t.sampling == Sampling::Nearest {
        return 0;
    }
    let footprint_x = w as f64 / t.size[0] * scale;
    let footprint_y = h as f64 / t.size[1] * scale;
    let footprint = footprint_x.min(footprint_y);
    if footprint < 2.0 {
        0
    } else {
        (footprint.log2().floor() as usize).min(12)
    }
}

/// A half-size copy, averaging 2×2 blocks with alpha weighting so edges don't darken.
pub fn halve(image: &RgbaImage) -> RgbaImage {
    let (w, h) = image.dimensions();
    let nw = w.div_ceil(2).max(1);
    let nh = h.div_ceil(2).max(1);
    let mut out = RgbaImage::new(nw, nh);
    use rayon::prelude::*;
    let src = image.as_raw();
    out.as_mut().par_chunks_mut(nw as usize * 4).enumerate().for_each(|(y, row)| {
        for x in 0..nw as usize {
            let mut sum = [0u32; 4];
            let mut count = 0u32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let sx = (x * 2 + dx).min(w as usize - 1);
                    let sy = (y * 2 + dy).min(h as usize - 1);
                    let i = (sy * w as usize + sx) * 4;
                    let a = src[i + 3] as u32;
                    sum[0] += src[i] as u32 * a;
                    sum[1] += src[i + 1] as u32 * a;
                    sum[2] += src[i + 2] as u32 * a;
                    sum[3] += a;
                    count += 1;
                }
            }
            let o = &mut row[x * 4..x * 4 + 4];
            if sum[3] > 0 {
                o[0] = (sum[0] / sum[3]) as u8;
                o[1] = (sum[1] / sum[3]) as u8;
                o[2] = (sum[2] / sum[3]) as u8;
            }
            o[3] = (sum[3] / count) as u8;
        }
    });
    out
}
