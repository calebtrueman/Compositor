//! CPU compositing. Everything renders into a `Region` of the canvas — a rectangle of output
//! pixels at some scale — so the canvas can redraw only what changed, and zoomed-out views can
//! render at a fraction of full resolution.

pub mod blend;
pub mod sample;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use image::RgbaImage;
use rayon::prelude::*;

use crate::doc::{Document, Id, Layer, LayerMask, MaskPixels};
use sample::Sampler;

/// Premultiplied RGBA in 0…1, row-major.
#[derive(Clone, Debug)]
pub struct Buffer {
    pub width: usize,
    pub height: usize,
    pub px: Vec<[f32; 4]>,
}

impl Buffer {
    pub fn new(width: usize, height: usize) -> Self {
        Buffer { width, height, px: vec![[0.0; 4]; width * height] }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> [f32; 4] {
        self.px[y * self.width + x]
    }

    /// Straight-alpha 8-bit pixels.
    pub fn to_rgba8(&self) -> RgbaImage {
        let mut out = RgbaImage::new(self.width as u32, self.height as u32);
        out.as_mut().par_chunks_mut(4).zip(self.px.par_iter()).for_each(|(o, p)| {
            let a = p[3].clamp(0.0, 1.0);
            if a > 0.0 {
                let inv = 1.0 / a;
                o[0] = to_u8(p[0] * inv);
                o[1] = to_u8(p[1] * inv);
                o[2] = to_u8(p[2] * inv);
                o[3] = to_u8(a);
            } else {
                o.copy_from_slice(&[0, 0, 0, 0]);
            }
        });
        out
    }

    /// Crops `[x, x+w) × [y, y+h)`.
    pub fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> Buffer {
        let mut out = Buffer::new(w, h);
        for row in 0..h {
            let src = (y + row) * self.width + x;
            out.px[row * w..row * w + w].copy_from_slice(&self.px[src..src + w]);
        }
        out
    }
}

#[inline]
pub fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Output pixel `(i, j)` covers document pixels starting at `((x + i) · scale, (y + j) · scale)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub x: i64,
    pub y: i64,
    pub width: usize,
    pub height: usize,
    /// Document pixels per output pixel (1 at full resolution, 2 at half, …).
    pub scale: f64,
}

impl Region {
    pub fn full(doc: &Document) -> Self {
        Region { x: 0, y: 0, width: doc.width as usize, height: doc.height as usize, scale: 1.0 }
    }

    /// Document coordinates of the center of output pixel `(i, j)`.
    #[inline]
    pub fn doc_point(&self, i: usize, j: usize) -> (f64, f64) {
        ((self.x as f64 + i as f64 + 0.5) * self.scale, (self.y as f64 + j as f64 + 0.5) * self.scale)
    }

    pub fn grown(&self, margin: usize) -> Region {
        Region {
            x: self.x - margin as i64,
            y: self.y - margin as i64,
            width: self.width + 2 * margin,
            height: self.height + 2 * margin,
            scale: self.scale,
        }
    }
}

/// Shared caches for rendering: downsampled copies of layer images for zoomed-out views.
#[derive(Default)]
pub struct RenderCache {
    mips: Mutex<HashMap<usize, (Weak<RgbaImage>, Vec<Arc<RgbaImage>>)>>,
}

impl RenderCache {
    /// `image` halved `level` times (level 0 is the image itself).
    pub fn mip(&self, image: &Arc<RgbaImage>, level: usize) -> Arc<RgbaImage> {
        if level == 0 {
            return image.clone();
        }
        let key = Arc::as_ptr(image) as usize;
        let mut mips = self.mips.lock().unwrap();
        mips.retain(|_, (weak, _)| weak.strong_count() > 0);
        let entry = mips.entry(key).or_insert_with(|| (Arc::downgrade(image), Vec::new()));
        if entry.0.upgrade().is_none_or(|i| !Arc::ptr_eq(&i, image)) {
            *entry = (Arc::downgrade(image), Vec::new());
        }
        while entry.1.len() < level {
            let source = entry.1.last().cloned().unwrap_or_else(|| image.clone());
            if source.width() <= 1 && source.height() <= 1 {
                break;
            }
            entry.1.push(Arc::new(sample::halve(&source)));
        }
        entry.1.get(level - 1).or(entry.1.last()).cloned().unwrap_or_else(|| image.clone())
    }
}

/// Renders the document into `region`, premultiplied.
pub fn composite(doc: &Document, region: Region, cache: &RenderCache) -> Buffer {
    composite_layers(doc, region, cache, None)
}

/// Like `composite`, but stops before the layer `until` (exclusive) when given; tools that
/// sample "all layers below" use it.
pub fn composite_layers(doc: &Document, region: Region, cache: &RenderCache, until: Option<Id>) -> Buffer {
    let margin = sampling_margin(doc, region.scale);
    let work = region.grown(margin);
    let mut buf = Buffer::new(work.width, work.height);
    let mut ancestor_cache: HashMap<Id, (bool, f32)> = HashMap::new();
    for layer in &doc.layers {
        if Some(layer.id) == until {
            break;
        }
        if layer.is_group {
            continue;
        }
        let (visible, opacity) = effective_visibility(doc, layer, &mut ancestor_cache);
        if !visible || opacity <= 0.0 {
            continue;
        }
        let mut coverage = Coverage::new(doc, layer, work, cache);
        if let Some(adjustment) = &layer.adjustment {
            apply_adjustment_layer(layer, adjustment, &mut buf, work, opacity, &coverage);
        } else if layer.image.is_some() || layer.text.is_some() {
            let Some(mut src) = place_layer(layer, work, cache) else { continue };
            if let Some(effects) = &layer.effects {
                // Effects follow the masked shape, and their sizes are in layer pixels.
                if let Some(own) = &coverage.own_mask {
                    apply_mask(&mut src, own, work);
                }
                let effect_region = Region { scale: work.scale / layer_scale(layer), ..work };
                src = crate::effects::apply(effects, src, effect_region);
                coverage.own_mask = None;
            }
            blend_buffer(layer, &src, &mut buf, work, opacity, &coverage);
        }
    }
    if margin > 0 {
        buf.crop(margin, margin, region.width, region.height)
    } else {
        buf
    }
}

/// The widest neighborhood any visible effect or adjustment reads, in output pixels.
fn sampling_margin(doc: &Document, scale: f64) -> usize {
    let mut total = 0.0;
    for layer in &doc.layers {
        if !layer.visible {
            continue;
        }
        if let Some(adjustment) = &layer.adjustment {
            total += crate::adjust::sampling_margin(adjustment);
        }
        if let Some(effects) = &layer.effects {
            total += crate::effects::margin(effects) * layer_scale(layer);
        }
    }
    (total / scale).ceil().min(4096.0) as usize
}

/// Inherited visibility and the product of folder opacities with the layer's own.
fn effective_visibility(doc: &Document, layer: &Layer, cache: &mut HashMap<Id, (bool, f32)>) -> (bool, f32) {
    let mut visible = layer.visible;
    let mut opacity = layer.opacity;
    if let Some(parent) = layer.parent {
        let (pv, po) = folder_state(doc, parent, cache, 0);
        visible &= pv;
        opacity *= po;
    }
    (visible, opacity)
}

fn folder_state(doc: &Document, id: Id, cache: &mut HashMap<Id, (bool, f32)>, depth: usize) -> (bool, f32) {
    if let Some(state) = cache.get(&id) {
        return *state;
    }
    let Some(folder) = doc.layer(id) else { return (true, 1.0) };
    let mut state = (folder.visible, folder.opacity);
    if let (Some(parent), true) = (folder.parent, depth < 64) {
        let (pv, po) = folder_state(doc, parent, cache, depth + 1);
        state = (state.0 && pv, state.1 * po);
    }
    cache.insert(id, state);
    state
}

/// The layer's pixels placed on the canvas: straight RGB with alpha, as a premultiplied buffer.
pub fn place_layer(layer: &Layer, region: Region, cache: &RenderCache) -> Option<Buffer> {
    let image = layer.image.as_ref()?;
    let sampler = Sampler::new(image, &layer.transform, region.scale, cache)?;
    let mut out = Buffer::new(region.width, region.height);
    out.px.par_chunks_mut(region.width).enumerate().for_each(|(j, row)| {
        sampler.sample_row(region, j, row);
    });
    Some(out)
}

/// Mask, folder masks and clipping coverage of one layer over a region, multiplied together.
pub struct Coverage {
    /// The layer's own mask, kept apart so effects can be drawn from the masked pixels.
    pub own_mask: Option<Sampler>,
    /// Masks of enclosing folders.
    pub masks: Vec<(Sampler, bool)>,
    pub clip: Option<Vec<f32>>,
}

impl Coverage {
    pub fn new(doc: &Document, layer: &Layer, region: Region, cache: &RenderCache) -> Coverage {
        let own_mask = layer.mask.as_ref().filter(|m| m.enabled).and_then(|m| mask_sampler(layer, m, region, cache)).map(|(s, _)| s);
        let mut masks = Vec::new();
        for ancestor in doc.ancestors(layer.id) {
            if let Some(folder) = doc.layer(ancestor) {
                if let Some(mask) = folder.mask.as_ref().filter(|m| m.enabled) {
                    if let Some(s) = mask_sampler(folder, mask, region, cache) {
                        masks.push(s);
                    }
                }
            }
        }
        let clip = layer.clip_source.map(|source| clip_alpha(doc, source, region, cache, 0));
        Coverage { own_mask, masks, clip }
    }

    pub fn is_full(&self) -> bool {
        self.own_mask.is_none() && self.masks.is_empty() && self.clip.is_none()
    }

    /// Coverage for one row, multiplied into `row`.
    pub fn apply_row(&self, region: Region, j: usize, row: &mut [f32]) {
        let mut scratch = vec![[0.0f32; 4]; row.len()];
        for sampler in self.own_mask.iter().chain(self.masks.iter().map(|(s, _)| s)) {
            sampler.sample_row(region, j, &mut scratch);
            for (r, s) in row.iter_mut().zip(&scratch) {
                // Masks are gray images expanded to RGBA; a mask outside its rectangle hides.
                *r *= s[0];
            }
        }
        if let Some(clip) = &self.clip {
            let start = j * region.width;
            for (r, c) in row.iter_mut().zip(&clip[start..start + region.width]) {
                *r *= c;
            }
        }
    }
}

fn mask_sampler(layer: &Layer, mask: &LayerMask, region: Region, cache: &RenderCache) -> Option<(Sampler, bool)> {
    let transform = if mask.linked { layer.transform } else { mask.placement.unwrap_or(layer.transform) };
    let sampler = match &mask.pixels {
        MaskPixels::Uniform(v) => Sampler::uniform(*v, &transform),
        MaskPixels::Pixels(gray) => Sampler::gray(gray, &transform, region.scale, cache)?,
    };
    Some((sampler, true))
}

/// Multiplies premultiplied pixels by a mask's coverage.
fn apply_mask(buf: &mut Buffer, mask: &Sampler, region: Region) {
    let width = buf.width;
    buf.px.par_chunks_mut(width).enumerate().for_each(|(j, row)| {
        let mut coverage = vec![[0.0f32; 4]; width];
        mask.sample_row(region, j, &mut coverage);
        for (p, c) in row.iter_mut().zip(&coverage) {
            for v in p.iter_mut() {
                *v *= c[0];
            }
        }
    });
}

/// Document pixels per layer pixel (the geometric mean of both axes), so effect sizes, which
/// are in layer pixels, grow and shrink with the layer.
fn layer_scale(layer: &Layer) -> f64 {
    let Some(image) = &layer.image else { return 1.0 };
    let sx = layer.transform.size[0] / image.width().max(1) as f64;
    let sy = layer.transform.size[1] / image.height().max(1) as f64;
    let k = (sx * sy).sqrt();
    if k.is_finite() && k > 0.0 { k } else { 1.0 }
}

/// The alpha a clipping base contributes: its pixels, opacity, mask and its own clipping.
fn clip_alpha(doc: &Document, id: Id, region: Region, cache: &RenderCache, depth: usize) -> Vec<f32> {
    let mut alpha = vec![0.0f32; region.width * region.height];
    let Some(layer) = doc.layer(id) else { return alpha };
    if depth > 256 {
        return alpha;
    }
    if layer.adjustment.is_some() {
        alpha.iter_mut().for_each(|a| *a = layer.opacity);
    } else if let Some(placed) = place_layer(layer, region, cache) {
        for (a, p) in alpha.iter_mut().zip(&placed.px) {
            *a = p[3] * layer.opacity;
        }
    } else {
        return alpha;
    }
    let mut coverage = Coverage::new(doc, layer, region, cache);
    if coverage.clip.is_none() && layer.clip_source.is_some() {
        coverage.clip = layer.clip_source.map(|s| clip_alpha(doc, s, region, cache, depth + 1));
    }
    if !coverage.is_full() {
        alpha.par_chunks_mut(region.width).enumerate().for_each(|(j, row)| coverage.apply_row(region, j, row));
    }
    alpha
}

fn blend_buffer(layer: &Layer, src: &Buffer, dst: &mut Buffer, region: Region, opacity: f32, coverage: &Coverage) {
    let width = dst.width;
    let mode = layer.blend;
    dst.px.par_chunks_mut(width).zip(src.px.par_chunks(width)).enumerate().for_each(|(j, (drow, srow))| {
        let mut cov = vec![opacity; width];
        if !coverage.is_full() {
            coverage.apply_row(region, j, &mut cov);
        }
        for ((d, s), c) in drow.iter_mut().zip(srow).zip(&cov) {
            let a = s[3] * c;
            if a <= 0.0 {
                continue;
            }
            let inv = 1.0 / s[3];
            blend::composite_over(mode, d, [s[0] * inv, s[1] * inv, s[2] * inv], a);
        }
    });
}

fn apply_adjustment_layer(
    layer: &Layer,
    adjustment: &crate::adjust::Adjustment,
    buf: &mut Buffer,
    region: Region,
    opacity: f32,
    coverage: &Coverage,
) {
    let mut straight = buf.clone();
    for p in &mut straight.px {
        if p[3] > 0.0 {
            let inv = 1.0 / p[3];
            p[0] *= inv;
            p[1] *= inv;
            p[2] *= inv;
        }
    }
    crate::adjust::apply(adjustment, &mut straight, region);
    let width = buf.width;
    let mode = layer.blend;
    buf.px.par_chunks_mut(width).zip(straight.px.par_chunks(width)).enumerate().for_each(|(j, (drow, arow))| {
        let mut cov = vec![opacity; width];
        if !coverage.is_full() {
            coverage.apply_row(region, j, &mut cov);
        }
        for ((d, a), c) in drow.iter_mut().zip(arow).zip(&cov) {
            let ab = d[3];
            if ab <= 0.0 || *c <= 0.0 {
                continue;
            }
            let inv = 1.0 / ab;
            let b = [d[0] * inv, d[1] * inv, d[2] * inv];
            let adjusted = [a[0].clamp(0.0, 1.0), a[1].clamp(0.0, 1.0), a[2].clamp(0.0, 1.0)];
            let mixed = blend::blend(mode, b, adjusted);
            for i in 0..3 {
                d[i] = (b[i] + (mixed[i] - b[i]) * c) * ab;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{BlendMode, Layer, LayerTransform};

    fn doc_with_layers() -> Document {
        let mut doc = Document::new(4000, 3000, Some([200, 100, 50, 255]));
        let mut top = Layer::new_pixel("Top", RgbaImage::from_pixel(3000, 2000, image::Rgba([10, 200, 90, 180])), LayerTransform::rect(500.0, 500.0, 3000.0, 2000.0));
        top.blend = BlendMode::Multiply;
        doc.layers.push(top);
        let mut rotated = Layer::new_pixel("Rotated", RgbaImage::from_pixel(1000, 1000, image::Rgba([0, 0, 255, 255])), LayerTransform::rect(100.0, 100.0, 1500.0, 1500.0));
        rotated.transform.rotation = 30.0;
        rotated.opacity = 0.5;
        doc.layers.push(rotated);
        doc
    }

    #[test]
    fn composites_expected_colors() {
        let doc = doc_with_layers();
        let cache = RenderCache::default();
        let region = Region { x: 0, y: 0, width: 4, height: 4, scale: 1.0 };
        let out = composite(&doc, region, &cache);
        let p = out.get(0, 0);
        assert!((p[0] - 200.0 / 255.0).abs() < 0.01 && (p[3] - 1.0).abs() < 1e-6);
        let region = Region { x: 3400, y: 2400, width: 1, height: 1, scale: 1.0 };
        let p = composite(&doc, region, &cache).get(0, 0);
        // Multiply at 180/255 alpha over orange.
        let a = 180.0 / 255.0;
        let expected_g = (100.0 / 255.0) * (1.0 - a) + (100.0 / 255.0 * 200.0 / 255.0) * a;
        assert!((p[1] - expected_g).abs() < 0.01, "{p:?}");
    }

    #[test]
    #[ignore]
    fn composite_speed() {
        let doc = doc_with_layers();
        let cache = RenderCache::default();
        let start = std::time::Instant::now();
        let out = composite(&doc, Region::full(&doc), &cache);
        println!("full 4000×3000, 3 layers: {:?}", start.elapsed());
        assert_eq!(out.width, 4000);
        let start = std::time::Instant::now();
        composite(&doc, Region { x: 0, y: 0, width: 1000, height: 750, scale: 4.0 }, &cache);
        println!("quarter-scale view: {:?}", start.elapsed());
        let start = std::time::Instant::now();
        composite(&doc, Region { x: 1024, y: 1024, width: 256, height: 256, scale: 1.0 }, &cache);
        println!("one 256 tile: {:?}", start.elapsed());
    }
}
