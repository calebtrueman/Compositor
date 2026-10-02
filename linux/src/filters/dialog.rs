//! The dialog behind every Image > Adjustments command and every filter: settings on the left of
//! OK/Cancel, a live preview on the canvas (the layer's pixels are swapped for a rendered copy while
//! the dialog is open), and one undo step on OK.

use std::sync::Arc;

use image::{GrayImage, Luma, RgbaImage};
use rayon::prelude::*;

use super::kinds::FilterSettings;
use crate::adjust::spatial::Px;
use crate::adjust::{Adjustment, AdjustmentKind, Histogram};
use crate::doc::{Affine, Document, Id, Layer, LayerMask, LayerTransform, MaskPixels, Selection};
use crate::project::{EditTarget, Project};
use crate::ui::dialogs::{ok_cancel, Dialog, DialogCtx, DialogState};

/// What a dialog applies.
#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    Adjust(Adjustment),
    Filter(FilterSettings),
}

impl Job {
    pub fn title(&self) -> String {
        match self {
            Job::Adjust(a) => crate::adjust::kind_name(a.kind).to_string(),
            Job::Filter(f) => f.kind.name().to_string(),
        }
    }

    /// Room needed around the layer, in layer pixels.
    fn margin(&self) -> f64 {
        match self {
            Job::Adjust(a) => crate::adjust::sampling_margin(a),
            Job::Filter(f) => f.margin(),
        }
    }

    fn spreads(&self) -> bool {
        match self {
            Job::Adjust(a) => a.changes_alpha(),
            Job::Filter(f) => f.kind.spreads(),
        }
    }

    fn is_identity(&self) -> bool {
        match self {
            Job::Adjust(a) => a.is_identity(),
            Job::Filter(f) => f.is_identity(),
        }
    }

    /// Longest side previews render at: color lookups are quick at full size, neighborhoods are
    /// not; noise and grain would look coarser made small and enlarged.
    fn preview_limit(&self) -> Option<u32> {
        match self {
            Job::Adjust(a) if matches!(a.kind, AdjustmentKind::Grain | AdjustmentKind::AddNoise) => None,
            Job::Adjust(a) if a.changes_alpha() => Some(2048),
            Job::Adjust(_) => Some(4096),
            Job::Filter(f) if f.kind == super::kinds::FilterKind::AddNoise => None,
            Job::Filter(_) => Some(2048),
        }
    }

    /// Runs over straight pixels at `scale` pixels per full-size layer pixel.
    pub fn run(&self, px: &mut Vec<Px>, width: usize, height: usize, scale: f64, seed: u32, fills_clear: bool) {
        match self {
            Job::Adjust(a) => {
                let mut buffer = crate::render::Buffer { width, height, px: std::mem::take(px) };
                let region = crate::render::Region { x: 0, y: 0, width, height, scale: 1.0 / scale.max(1e-6) };
                crate::adjust::apply(a, &mut buffer, region);
                *px = buffer.px;
            }
            Job::Filter(f) => f.run(px, width, height, scale, seed, fills_clear),
        }
    }
}

// --- Pixel plumbing ---------------------------------------------------------------------------

pub fn to_px(image: &RgbaImage) -> Vec<Px> {
    image.as_raw().par_chunks(4).map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0]).collect()
}

pub fn from_px(px: &[Px], width: u32, height: u32) -> RgbaImage {
    let mut out = RgbaImage::new(width, height);
    out.as_mut().par_chunks_mut(4).zip(px.par_iter()).for_each(|(o, p)| {
        let a = crate::render::to_u8(p[3]);
        if a == 0 {
            o.copy_from_slice(&[0, 0, 0, 0]);
        } else {
            o.copy_from_slice(&[crate::render::to_u8(p[0]), crate::render::to_u8(p[1]), crate::render::to_u8(p[2]), a]);
        }
    });
    out
}

fn gray_to_px(gray: &GrayImage) -> Vec<Px> {
    gray.as_raw().par_iter().map(|v| {
        let v = *v as f32 / 255.0;
        [v, v, v, 1.0]
    }).collect()
}

fn px_to_gray(px: &[Px], width: u32, height: u32) -> GrayImage {
    let raw: Vec<u8> = px.par_iter().map(|p| crate::render::to_u8(p[0])).collect();
    GrayImage::from_raw(width, height, raw).unwrap()
}

/// Mixes `result` back toward `original` outside the selection, by coverage (premultiplied).
fn blend_selection(result: &mut [Px], original: &[Px], width: usize, to_doc: Affine, selection: &Selection) {
    result.par_chunks_mut(width).zip(original.par_chunks(width)).enumerate().for_each(|(y, (rrow, orow))| {
        for (x, (r, o)) in rrow.iter_mut().zip(orow).enumerate() {
            let (dx, dy) = to_doc.apply(x as f64 + 0.5, y as f64 + 0.5);
            let c = selection.coverage(dx.floor() as i64, dy.floor() as i64) as f32 / 255.0;
            if c >= 1.0 {
                continue;
            }
            let alpha = o[3] + (r[3] - o[3]) * c;
            if alpha <= 1e-6 {
                *r = [0.0; 4];
                continue;
            }
            for i in 0..3 {
                r[i] = (o[i] * o[3] + (r[i] * r[3] - o[i] * o[3]) * c) / alpha;
            }
            r[3] = alpha;
        }
    });
}

/// `image` with `margin` transparent pixels on every side.
fn pad(image: &RgbaImage, margin: u32) -> RgbaImage {
    if margin == 0 {
        return image.clone();
    }
    let mut out = RgbaImage::new(image.width() + 2 * margin, image.height() + 2 * margin);
    image::imageops::replace(&mut out, image, margin as i64, margin as i64);
    out
}

/// The transform placing a `width`×`height` grid grown by `margin` on every side where the
/// original grid was.
fn grown_transform(t: &LayerTransform, width: u32, height: u32, margin: u32) -> LayerTransform {
    let mut out = *t;
    out.size = [
        t.size[0] * (width + 2 * margin) as f64 / width.max(1) as f64,
        t.size[1] * (height + 2 * margin) as f64 / height.max(1) as f64,
    ];
    let (cx, cy) = t.center();
    out.origin = [cx - out.size[0] / 2.0, cy - out.size[1] / 2.0];
    out
}

/// The transform placing the pixels `(x0, y0)…(x1, y1)` of a `width`×`height` grid placed by `t`.
pub fn cropped_transform(t: &LayerTransform, width: u32, height: u32, (x0, y0, x1, y1): (u32, u32, u32, u32)) -> LayerTransform {
    let mut out = *t;
    out.size = [t.size[0] * (x1 - x0) as f64 / width.max(1) as f64, t.size[1] * (y1 - y0) as f64 / height.max(1) as f64];
    let (mx, my) = t.pixel_to_document(width, height).apply((x0 + x1) as f64 / 2.0, (y0 + y1) as f64 / 2.0);
    out.origin = [mx - out.size[0] / 2.0, my - out.size[1] / 2.0];
    out
}

/// A mask resampled onto a new grid, its edge tone carried past its old edges.
pub fn remap_mask(mask: &GrayImage, from: &LayerTransform, to: &LayerTransform, width: u32, height: u32) -> GrayImage {
    let (mw, mh) = mask.dimensions();
    let Some(to_mask) = from.pixel_to_document(mw, mh).inverse() else { return GrayImage::from_pixel(width, height, Luma([255])) };
    let to_doc = to.pixel_to_document(width, height);
    let map = to_doc.then(&to_mask);
    let mut out = GrayImage::new(width, height);
    out.as_mut().par_chunks_mut(width as usize).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let (sx, sy) = map.apply(x as f64 + 0.5, y as f64 + 0.5);
            let sx = (sx - 0.5).clamp(0.0, (mw - 1) as f64);
            let sy = (sy - 0.5).clamp(0.0, (mh - 1) as f64);
            let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
            let (x1, y1) = ((x0 + 1).min(mw - 1), (y0 + 1).min(mh - 1));
            let (fx, fy) = (sx - x0 as f64, sy - y0 as f64);
            let at = |x: u32, y: u32| mask.get_pixel(x, y)[0] as f64;
            let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * fx;
            let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * fx;
            *v = (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8;
        }
    });
    out
}

fn downscale_rgba(image: &RgbaImage, limit: Option<u32>) -> (RgbaImage, f64) {
    let longest = image.width().max(image.height());
    match limit {
        Some(limit) if longest > limit => {
            let scale = limit as f64 / longest as f64;
            let w = ((image.width() as f64 * scale).round() as u32).max(1);
            let h = ((image.height() as f64 * scale).round() as u32).max(1);
            (image::imageops::resize(image, w, h, image::imageops::FilterType::Triangle), w as f64 / image.width() as f64)
        }
        _ => (image.clone(), 1.0),
    }
}

/// What a job is applied to: the active layer's pixels, or its mask.
#[derive(Clone)]
pub enum Source {
    Image {
        image: Arc<RgbaImage>,
        transform: LayerTransform,
        /// Vignette on an empty layer: a clear canvas it frames and fills.
        fills_clear: bool,
        /// Pixels per full-size layer pixel (below 1 for a preview copy).
        scale: f64,
    },
    Mask {
        gray: Arc<GrayImage>,
        placement: LayerTransform,
        scale: f64,
    },
}

/// The pixels a job leaves on the layer.
pub enum Rendered {
    Image { image: RgbaImage, transform: LayerTransform },
    Mask(GrayImage),
}

impl Source {
    /// The active layer's pixels (or mask) in `doc`, or `None` when there's nothing to filter.
    pub fn of(doc: &Document, target: EditTarget, allow_empty: bool) -> Option<Source> {
        let layer = doc.active_layer()?;
        if layer.is_group || layer.adjustment.is_some() || !doc.is_effectively_visible(layer.id) {
            return None;
        }
        if target == EditTarget::Mask {
            let mask = layer.mask.as_ref()?;
            let placement = if mask.linked { layer.transform } else { mask.placement.unwrap_or(layer.transform) };
            let (w, h) = match &mask.pixels {
                MaskPixels::Pixels(p) => p.dimensions(),
                MaskPixels::Uniform(_) => layer.pixel_size(),
            };
            let gray = match &mask.pixels {
                MaskPixels::Pixels(p) => p.clone(),
                MaskPixels::Uniform(_) => Arc::new(crate::doc::ops::expand_mask(&mask.pixels, w, h)),
            };
            return Some(Source::Mask { gray, placement, scale: 1.0 });
        }
        match &layer.image {
            Some(image) => Some(Source::Image { image: image.clone(), transform: layer.transform, fills_clear: false, scale: 1.0 }),
            None if allow_empty && layer.text.is_none() => Some(Source::Image {
                image: Arc::new(RgbaImage::new(doc.width, doc.height)),
                transform: doc.full_canvas_transform(),
                fills_clear: true,
                scale: 1.0,
            }),
            None => None,
        }
    }

    pub fn histogram(&self, selection: Option<&Selection>) -> Histogram {
        let Source::Image { image, transform, .. } = self else { return Histogram { bins: [[0.0; 256]; 4] } };
        let (small, _) = downscale_rgba(image, Some(1024));
        let to_doc = transform.pixel_to_document(small.width(), small.height());
        let width = small.width() as usize;
        let pixels: Vec<[u8; 4]> = small.pixels().map(|p| p.0).collect();
        Histogram::of(&pixels, |i| match selection {
            Some(s) => {
                let (dx, dy) = to_doc.apply((i % width) as f64 + 0.5, (i / width) as f64 + 0.5);
                s.coverage(dx.floor() as i64, dy.floor() as i64) as f32 / 255.0
            }
            None => 1.0,
        })
    }

    /// A copy no larger than `limit` on its longest side, for previews.
    pub fn downscaled(&self, limit: Option<u32>) -> Source {
        match self {
            Source::Mask { gray, placement, scale } => {
                let (w, h) = gray.dimensions();
                match limit {
                    Some(limit) if w.max(h) > limit => {
                        let s = limit as f64 / w.max(h) as f64;
                        let (sw, sh) = (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1));
                        let small = image::imageops::resize(&**gray, sw, sh, image::imageops::FilterType::Triangle);
                        Source::Mask { gray: Arc::new(small), placement: *placement, scale: scale * sw as f64 / w as f64 }
                    }
                    _ => self.clone(),
                }
            }
            Source::Image { image, transform, fills_clear, scale } => {
                if !limit.is_some_and(|l| image.width().max(image.height()) > l) {
                    return self.clone();
                }
                let (small, factor) = downscale_rgba(image, limit);
                Source::Image { image: Arc::new(small), transform: *transform, fills_clear: *fills_clear, scale: scale * factor }
            }
        }
    }

    /// Runs `job`, limited to the selection. To `commit`, a filter that spread is trimmed back
    /// to the pixels it left.
    pub fn render(&self, job: &Job, selection: Option<&Selection>, seed: u32, commit: bool) -> Rendered {
        match self {
            Source::Mask { gray, placement, scale } => {
                let (w, h) = gray.dimensions();
                let original = gray_to_px(gray);
                let mut px = original.clone();
                job.run(&mut px, w as usize, h as usize, *scale, seed, false);
                // A mask is opaque: what a blur spreads past its edge is dropped.
                px.par_iter_mut().for_each(|p| p[3] = 1.0);
                if let Some(selection) = selection {
                    blend_selection(&mut px, &original, w as usize, placement.pixel_to_document(w, h), selection);
                }
                Rendered::Mask(px_to_gray(&px, w, h))
            }
            Source::Image { image, transform, fills_clear, scale } => {
                let margin = if job.spreads() { (job.margin() * scale).ceil().max(0.0) as u32 } else { 0 };
                let grown = pad(image, margin);
                let placed = grown_transform(transform, image.width(), image.height(), margin);
                let (w, h) = grown.dimensions();
                let original = to_px(&grown);
                let mut px = original.clone();
                job.run(&mut px, w as usize, h as usize, *scale, seed, *fills_clear);
                if let Some(selection) = selection {
                    blend_selection(&mut px, &original, w as usize, placed.pixel_to_document(w, h), selection);
                }
                let result = from_px(&px, w, h);
                if commit && margin > 0 {
                    // What the blur left empty is cut away again.
                    if let Some(bounds) = crate::doc::ops::alpha_bounds(&result) {
                        if bounds != (0, 0, w, h) {
                            let cropped = image::imageops::crop_imm(&result, bounds.0, bounds.1, bounds.2 - bounds.0, bounds.3 - bounds.1).to_image();
                            return Rendered::Image { image: cropped, transform: cropped_transform(&placed, w, h, bounds) };
                        }
                    }
                }
                Rendered::Image { image: result, transform: placed }
            }
        }
    }
}

/// Puts rendered pixels on `layer`, carrying a linked mask onto a grid that grew or shrank.
fn place(layer: &mut Layer, original: &Layer, rendered: Rendered) {
    match rendered {
        Rendered::Mask(gray) => {
            if let Some(mask) = layer.mask.as_mut() {
                mask.pixels = MaskPixels::Pixels(Arc::new(gray));
            }
        }
        Rendered::Image { image, transform } => {
            let (w, h) = image.dimensions();
            if transform != original.transform {
                if let Some(LayerMask { pixels: MaskPixels::Pixels(gray), linked: true, .. }) = &original.mask {
                    let remapped = remap_mask(gray, &original.transform, &transform, w, h);
                    if let Some(mask) = layer.mask.as_mut() {
                        mask.pixels = MaskPixels::Pixels(Arc::new(remapped));
                    }
                }
            }
            layer.image = Some(Arc::new(image));
            layer.transform = transform;
            layer.rasterized();
        }
    }
}

/// Puts the original layer's pixels back.
fn restore(doc: &mut Document, original: &Layer) {
    if let Some(layer) = doc.layer_mut(original.id) {
        layer.image = original.image.clone();
        layer.transform = original.transform;
        layer.mask = original.mask.clone();
        layer.text = original.text.clone();
        layer.shape = original.shape.clone();
    }
}

/// Applies `job` to the active layer (or its mask) at once, as one undo step: commands without
/// settings (Desaturate) and the dialogs' OK.
pub fn apply_now(project: &mut Project, name: &str, job: &Job, seed: u32) -> bool {
    let Some(source) = Source::of(&project.doc, project.target, matches!(job, Job::Filter(f) if f.kind == super::kinds::FilterKind::Vignette))
    else {
        return false;
    };
    let Some(original) = project.doc.active_layer().cloned() else { return false };
    let rendered = source.render(job, project.doc.selection.as_ref(), seed, true);
    project.edit(name, |doc| {
        if let Some(layer) = doc.layer_mut(original.id) {
            place(layer, &original, rendered);
        }
    });
    true
}

pub struct LiveDialog {
    job: Job,
    layer: Id,
    original: Layer,
    source: Source,
    /// The downscaled copy previews run on, made the first time one is needed.
    preview_source: Option<Source>,
    preview: bool,
    /// What the canvas currently shows, so unchanged settings aren't rendered again.
    shown: Option<(Job, bool)>,
    histogram: Option<Histogram>,
    seed: u32,
}

impl LiveDialog {
    /// A dialog for `job` on the active layer, or `None` when there's nothing it can change.
    pub fn new(project: &Project, job: Job) -> Option<LiveDialog> {
        let allow_empty = matches!(&job, Job::Filter(f) if f.kind == super::kinds::FilterKind::Vignette);
        let source = Source::of(&project.doc, project.target, allow_empty)?;
        let original = project.doc.active_layer()?.clone();
        let histogram = match &job {
            Job::Adjust(a) if matches!(a.kind, AdjustmentKind::Levels | AdjustmentKind::Curves) => {
                Some(source.histogram(project.doc.selection.as_ref()))
            }
            _ => None,
        };
        Some(LiveDialog { job, layer: original.id, original, source, preview_source: None, preview: true, shown: None, histogram, seed: rand::random() })
    }

    fn show_preview(&mut self, project: &mut Project) {
        let state = (self.job.clone(), self.preview);
        if self.shown.as_ref() == Some(&state) {
            return;
        }
        if !self.preview {
            restore(&mut project.doc, &self.original);
        } else {
            let limit = self.job.preview_limit();
            let source = self.preview_source.get_or_insert_with(|| self.source.downscaled(limit));
            let rendered = source.render(&self.job, project.doc.selection.as_ref(), self.seed, false);
            if let Some(layer) = project.doc.layer_mut(self.layer) {
                place(layer, &self.original, rendered);
            }
        }
        self.shown = Some(state);
        project.invalidate_all();
    }
}

impl Dialog for LiveDialog {
    fn title(&self) -> String {
        format!("{}…", self.job.title()).replace("……", "…")
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut DialogCtx) -> DialogState {
        let Some(project) = ctx.project.as_deref_mut() else { return DialogState::Closed };
        if project.doc.layer(self.layer).is_none() {
            return DialogState::Closed;
        }
        ui.set_max_width(380.0);
        match &mut self.job {
            Job::Adjust(a) => {
                crate::adjust::ui::editor(ui, a, self.histogram.as_ref());
            }
            Job::Filter(f) => {
                f.controls(ui);
            }
        }
        ui.add_space(4.0);
        ui.checkbox(&mut self.preview, "Preview");
        if project.doc.selection.is_some() {
            ui.weak("Limited to the selection");
        }
        let result = ok_cancel(ui, "OK");
        match result {
            Some(true) => {
                restore(&mut project.doc, &self.original);
                project.invalidate_all();
                if let Job::Filter(f) = &self.job {
                    f.remember();
                }
                if !self.job.is_identity() {
                    let rendered = self.source.render(&self.job, project.doc.selection.as_ref(), self.seed, true);
                    let original = self.original.clone();
                    project.edit(&self.job.title(), |doc| {
                        if let Some(layer) = doc.layer_mut(original.id) {
                            place(layer, &original, rendered);
                        }
                    });
                }
                DialogState::Closed
            }
            Some(false) => {
                restore(&mut project.doc, &self.original);
                project.invalidate_all();
                DialogState::Closed
            }
            None => {
                self.show_preview(project);
                DialogState::Open
            }
        }
    }

    fn cancel(&mut self, ctx: &mut DialogCtx) {
        if let Some(project) = ctx.project.as_deref_mut() {
            restore(&mut project.doc, &self.original);
            project.invalidate_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_and_grow_keep_pixels_in_place() {
        let t = LayerTransform { rotation: 20.0, ..LayerTransform::rect(10.0, 20.0, 100.0, 50.0) };
        let grown = grown_transform(&t, 100, 50, 5);
        let a = t.pixel_to_document(100, 50).apply(3.0, 4.0);
        let b = grown.pixel_to_document(110, 60).apply(8.0, 9.0);
        assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
        let cropped = cropped_transform(&grown, 110, 60, (7, 2, 100, 55));
        let c = cropped.pixel_to_document(93, 53).apply(1.0, 7.0);
        assert!((a.0 - c.0).abs() < 1e-9 && (a.1 - c.1).abs() < 1e-9);
    }

    #[test]
    fn blur_filter_grows_then_trims_the_layer() {
        let mut image = RgbaImage::new(20, 20);
        for p in image.pixels_mut() {
            *p = image::Rgba([200, 100, 50, 255]);
        }
        let source = Source::Image { image: Arc::new(image), transform: LayerTransform::rect(0.0, 0.0, 20.0, 20.0), fills_clear: false, scale: 1.0 };
        let mut settings = FilterSettings::new(super::super::kinds::FilterKind::GaussianBlur);
        settings.radius = 2.0;
        let Rendered::Image { image, transform } = source.render(&Job::Filter(settings), None, 0, true) else { panic!() };
        assert!(image.width() > 20 && image.width() <= 20 + 2 * 8);
        // The middle is untouched and stays where it was.
        let (cx, cy) = transform.center();
        assert!((cx - 10.0).abs() < 1e-9 && (cy - 10.0).abs() < 1e-9);
        let middle = image.get_pixel(image.width() / 2, image.height() / 2);
        assert_eq!(middle.0, [200, 100, 50, 255]);
    }

    #[test]
    fn selection_limits_an_adjustment() {
        let image = RgbaImage::from_pixel(4, 1, image::Rgba([100, 100, 100, 255]));
        let source = Source::Image { image: Arc::new(image), transform: LayerTransform::rect(0.0, 0.0, 4.0, 1.0), fills_clear: false, scale: 1.0 };
        let mut mask = GrayImage::new(4, 1);
        mask.put_pixel(0, 0, Luma([255]));
        mask.put_pixel(1, 0, Luma([128]));
        let selection = Selection::from_mask(mask).unwrap();
        let Rendered::Image { image, .. } = source.render(&Job::Adjust(Adjustment::new(AdjustmentKind::Invert)), Some(&selection), 0, true) else { panic!() };
        assert_eq!(image.get_pixel(0, 0)[0], 155);
        assert!((image.get_pixel(1, 0)[0] as i32 - 128).abs() <= 1);
        assert_eq!(image.get_pixel(3, 0)[0], 100);
    }
}
