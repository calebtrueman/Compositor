//! The Filter menu's filters and their settings, as in the macOS app's `Filters.swift` (plus
//! Unsharp Mask). They run on straight-alpha float pixels of a layer.

use std::sync::Mutex;

use rayon::prelude::*;

use crate::adjust::spatial::{self, Px};
use crate::adjust::tone::{rec709, AdjustmentColor};
use crate::adjust::ui::{grid, slider};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    GaussianBlur,
    MotionBlur,
    AddNoise,
    UnsharpMask,
    Vignette,
    BloomGlow,
    TonalContrast,
    LensCorrection,
}

impl FilterKind {
    /// Filter menu order.
    pub const ALL: [FilterKind; 8] = [
        FilterKind::GaussianBlur,
        FilterKind::MotionBlur,
        FilterKind::AddNoise,
        FilterKind::UnsharpMask,
        FilterKind::Vignette,
        FilterKind::BloomGlow,
        FilterKind::TonalContrast,
        FilterKind::LensCorrection,
    ];

    pub fn name(self) -> &'static str {
        match self {
            FilterKind::GaussianBlur => "Gaussian Blur",
            FilterKind::MotionBlur => "Motion Blur",
            FilterKind::AddNoise => "Add Noise",
            FilterKind::UnsharpMask => "Unsharp Mask",
            FilterKind::Vignette => "Vignette",
            FilterKind::BloomGlow => "Bloom / Glow",
            FilterKind::TonalContrast => "Tonal Contrast",
            FilterKind::LensCorrection => "Lens Correction",
        }
    }

    /// Filters whose result spreads past the layer's edges: the layer grows to make room.
    pub fn spreads(self) -> bool {
        matches!(self, FilterKind::GaussianBlur | FilterKind::MotionBlur | FilterKind::BloomGlow)
    }
}

/// Every filter's settings; each filter reads only its own. Distances are in layer pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterSettings {
    pub kind: FilterKind,
    /// Gaussian Blur's standard deviation, 0.1–250.
    pub radius: f64,
    /// Motion Blur direction, degrees counterclockwise from horizontal, −90–90.
    pub angle: f64,
    /// Motion Blur streak length, 1–2000.
    pub distance: f64,
    /// Add Noise strength as Photoshop's percentage, 0.1–400.
    pub amount: f64,
    pub gaussian: bool,
    pub monochromatic: bool,
    /// Unsharp Mask: strength in percent (1–500), blur radius (0.1–250) and threshold (0–255 levels).
    pub sharpen_amount: f64,
    pub sharpen_radius: f64,
    pub sharpen_threshold: f64,
    pub vignette_amount: f64,
    pub vignette_color: AdjustmentColor,
    pub vignette_midpoint: f64,
    pub vignette_roundness: f64,
    pub vignette_feather: f64,
    pub vignette_highlights: f64,
    pub bloom_amount: f64,
    pub bloom_radius: f64,
    pub tonal_amount: f64,
    pub tonal_radius: f64,
    pub tonal_shadows: f64,
    pub tonal_midtones: f64,
    pub tonal_highlights: f64,
    /// Lens Correction's Remove Distortion, −100–100: positive straightens barrel distortion.
    pub distortion: f64,
}

impl FilterSettings {
    pub fn new(kind: FilterKind) -> Self {
        FilterSettings {
            kind,
            radius: 1.0,
            angle: 0.0,
            distance: 10.0,
            amount: 10.0,
            gaussian: false,
            monochromatic: false,
            sharpen_amount: 100.0,
            sharpen_radius: 1.0,
            sharpen_threshold: 0.0,
            vignette_amount: 35.0,
            vignette_color: AdjustmentColor::BLACK,
            vignette_midpoint: 50.0,
            vignette_roundness: 100.0,
            vignette_feather: 60.0,
            vignette_highlights: 25.0,
            bloom_amount: 40.0,
            bloom_radius: 24.0,
            tonal_amount: 50.0,
            tonal_radius: 16.0,
            tonal_shadows: 40.0,
            tonal_midtones: 60.0,
            tonal_highlights: 30.0,
            distortion: 0.0,
        }
    }

    /// The settings this filter was last applied with this session, as in the macOS app.
    pub fn last_used(kind: FilterKind) -> Self {
        LAST.lock().unwrap().iter().find(|s| s.kind == kind).cloned().unwrap_or_else(|| FilterSettings::new(kind))
    }

    pub fn remember(&self) {
        let mut last = LAST.lock().unwrap();
        last.retain(|s| s.kind != self.kind);
        last.push(self.clone());
    }

    /// Whether OK would change nothing (closes as Cancel does).
    pub fn is_identity(&self) -> bool {
        match self.kind {
            FilterKind::LensCorrection => self.distortion == 0.0,
            FilterKind::Vignette => self.vignette_amount == 0.0,
            FilterKind::BloomGlow => self.bloom_amount == 0.0,
            FilterKind::TonalContrast => {
                self.tonal_amount == 0.0 || (self.tonal_shadows == 0.0 && self.tonal_midtones == 0.0 && self.tonal_highlights == 0.0)
            }
            FilterKind::UnsharpMask => self.sharpen_amount == 0.0,
            _ => false,
        }
    }

    /// The room the filter needs around the layer, in layer pixels.
    pub fn margin(&self) -> f64 {
        match self.kind {
            FilterKind::GaussianBlur => self.radius * 3.0 + 2.0,
            FilterKind::MotionBlur => self.distance / 2.0 + 2.0,
            FilterKind::BloomGlow => self.bloom_radius * 3.0 + 2.0,
            _ => 0.0,
        }
    }

    /// Runs the filter over straight pixels. `scale` is pixels per full-size layer pixel (below 1
    /// for a downscaled preview); `fills_clear` lets Vignette paint transparent pixels (an empty
    /// layer it frames and fills).
    pub fn run(&self, px: &mut [Px], width: usize, height: usize, scale: f64, seed: u32, fills_clear: bool) {
        if width == 0 || height == 0 {
            return;
        }
        match self.kind {
            FilterKind::GaussianBlur => {
                spatial::premultiply(px);
                spatial::gaussian_blur(px, width, height, self.radius.clamp(0.1, 250.0) * scale);
                spatial::unpremultiply(px);
            }
            FilterKind::MotionBlur => {
                spatial::premultiply(px);
                spatial::motion_blur(px, width, height, self.angle.clamp(-90.0, 90.0), self.distance.clamp(1.0, 2000.0) * scale);
                spatial::unpremultiply(px);
            }
            FilterKind::AddNoise => {
                let amount = self.amount.clamp(0.1, 400.0) as f32;
                px.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
                    spatial::add_noise_row(row, amount, self.gaussian, self.monochromatic, seed, 0, y as i64);
                });
            }
            FilterKind::UnsharpMask => {
                let mut blurred = px.to_vec();
                spatial::premultiply(&mut blurred);
                spatial::gaussian_blur(&mut blurred, width, height, self.sharpen_radius.clamp(0.1, 250.0) * scale);
                spatial::unpremultiply(&mut blurred);
                let amount = (self.sharpen_amount / 100.0) as f32;
                let threshold = (self.sharpen_threshold / 255.0) as f32;
                px.par_iter_mut().zip(blurred.par_iter()).for_each(|(p, b)| {
                    if p[3] <= 0.0 {
                        return;
                    }
                    let base = if b[3] > 0.0 { *b } else { *p };
                    for c in 0..3 {
                        let detail = p[c] - base[c];
                        if detail.abs() >= threshold {
                            p[c] = (p[c] + detail * amount).clamp(0.0, 1.0);
                        }
                    }
                });
            }
            FilterKind::Vignette => self.vignette(px, width, height, fills_clear),
            FilterKind::BloomGlow => {
                spatial::premultiply(px);
                let mut glow = px.to_vec();
                spatial::gaussian_blur(&mut glow, width, height, self.bloom_radius.clamp(1.0, 150.0) * scale);
                let k = (self.bloom_amount.clamp(0.0, 100.0) / 50.0) as f32;
                // The blurred light screened over the original, alpha included.
                px.par_iter_mut().zip(glow.par_iter()).for_each(|(p, g)| {
                    for c in 0..4 {
                        p[c] = (p[c] + g[c] * k * (1.0 - p[c])).clamp(0.0, 1.0);
                    }
                    for c in 0..3 {
                        p[c] = p[c].min(p[3]);
                    }
                });
                spatial::unpremultiply(px);
            }
            FilterKind::TonalContrast => self.tonal_contrast(px, width, height, scale),
            FilterKind::LensCorrection => {
                spatial::premultiply(px);
                lens_distort(px, width, height, self.distortion.clamp(-100.0, 100.0) / 100.0 * 0.35);
                spatial::unpremultiply(px);
            }
        }
    }

    fn vignette(&self, px: &mut [Px], width: usize, height: usize, fills_clear: bool) {
        let strength = (self.vignette_amount / 100.0).clamp(0.0, 1.0);
        if strength <= 0.0 {
            return;
        }
        let color = self.vignette_color.rgb().map(|v| v as f64);
        let (w, h) = (width as f64, height as f64);
        px.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
            for (x, p) in row.iter_mut().enumerate() {
                if p[3] <= 0.0 && !fills_clear {
                    continue;
                }
                let mask = vignette_mask(x as f64 + 0.5, y as f64 + 0.5, w, h, self.vignette_midpoint, self.vignette_roundness, self.vignette_feather);
                if mask <= 0.0 {
                    continue;
                }
                let alpha = p[3] as f64;
                let (mut r, mut g, mut b, mut bright) = (0.0, 0.0, 0.0, 0.0);
                if alpha > 0.0 {
                    r = p[0].min(1.0) as f64;
                    g = p[1].min(1.0) as f64;
                    b = p[2].min(1.0) as f64;
                    bright = ((rec709([r as f32, g as f32, b as f32]) as f64 - 0.45) / 0.55).clamp(0.0, 1.0);
                }
                let effect = strength * mask * (1.0 - (self.vignette_highlights / 100.0) * bright);
                if !fills_clear {
                    // Only the pixels that are there change color; their coverage stays as it was.
                    p[0] = (r + (color[0] - r) * effect) as f32;
                    p[1] = (g + (color[1] - g) * effect) as f32;
                    p[2] = (b + (color[2] - b) * effect) as f32;
                    continue;
                }
                // The color painted over the pixel at `effect`.
                let out = alpha + effect * (1.0 - alpha);
                if out <= 0.0 {
                    continue;
                }
                p[0] = ((color[0] * effect + r * alpha * (1.0 - effect)) / out) as f32;
                p[1] = ((color[1] * effect + g * alpha * (1.0 - effect)) / out) as f32;
                p[2] = ((color[2] * effect + b * alpha * (1.0 - effect)) / out) as f32;
                p[3] = out as f32;
            }
        });
    }

    fn tonal_contrast(&self, px: &mut [Px], width: usize, height: usize, scale: f64) {
        if self.is_identity() {
            return;
        }
        let mut base = px.to_vec();
        spatial::premultiply(&mut base);
        spatial::gaussian_blur(&mut base, width, height, self.tonal_radius.clamp(1.0, 100.0) * scale);
        spatial::unpremultiply(&mut base);
        let strength = self.tonal_amount / 50.0;
        let smooth = |low: f64, high: f64, v: f64| {
            let t = ((v - low) / (high - low)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        px.par_iter_mut().zip(base.par_iter()).for_each(|(p, b)| {
            if p[3] <= 0.0 || b[3] <= 0.0 {
                return;
            }
            let c = [p[0].min(1.0), p[1].min(1.0), p[2].min(1.0)];
            let lum = rec709(c) as f64;
            let base_lum = rec709([b[0], b[1], b[2]]) as f64;
            let shadow = 1.0 - smooth(0.15, 0.5, base_lum);
            let highlight = smooth(0.5, 0.85, base_lum);
            let midtone = 1.0 - shadow - highlight;
            let weight = (self.tonal_shadows * shadow + self.tonal_midtones * midtone + self.tonal_highlights * highlight) / 100.0;
            let detail = lum - base_lum;
            let delta = (0.18 * (detail * 6.0).tanh() * weight * strength * (4.0 * lum * (1.0 - lum))) as f32;
            for i in 0..3 {
                p[i] = (c[i] + delta).clamp(0.0, 1.0);
            }
        });
    }

    /// The filter's controls. Returns true when a setting changed.
    pub fn controls(&mut self, ui: &mut egui::Ui) -> bool {
        let before = self.clone();
        ui.spacing_mut().slider_width = 220.0;
        match self.kind {
            FilterKind::GaussianBlur => grid(ui, "filter", |ui| {
                slider(ui, "Radius", &mut self.radius, 0.1..=250.0, 1, " px", true);
            }),
            FilterKind::MotionBlur => grid(ui, "filter", |ui| {
                slider(ui, "Angle", &mut self.angle, -90.0..=90.0, 0, "°", false);
                slider(ui, "Distance", &mut self.distance, 1.0..=2000.0, 0, " px", true);
            }),
            FilterKind::AddNoise => {
                grid(ui, "filter", |ui| {
                    slider(ui, "Amount", &mut self.amount, 0.1..=400.0, 1, "%", true);
                });
                ui.horizontal(|ui| {
                    ui.label("Distribution");
                    ui.selectable_value(&mut self.gaussian, false, "Uniform");
                    ui.selectable_value(&mut self.gaussian, true, "Gaussian");
                });
                ui.checkbox(&mut self.monochromatic, "Monochromatic");
            }
            FilterKind::UnsharpMask => grid(ui, "filter", |ui| {
                slider(ui, "Amount", &mut self.sharpen_amount, 1.0..=500.0, 0, "%", false);
                slider(ui, "Radius", &mut self.sharpen_radius, 0.1..=250.0, 1, " px", true);
                slider(ui, "Threshold", &mut self.sharpen_threshold, 0.0..=255.0, 0, " levels", false);
            }),
            FilterKind::Vignette => {
                ui.horizontal(|ui| {
                    ui.label("Color");
                    let c = &mut self.vignette_color;
                    let mut rgb = [c.red as f32, c.green as f32, c.blue as f32];
                    if egui::color_picker::color_edit_button_rgb(ui, &mut rgb).changed() {
                        *c = AdjustmentColor { red: rgb[0] as f64, green: rgb[1] as f64, blue: rgb[2] as f64 };
                    }
                });
                grid(ui, "filter", |ui| {
                    slider(ui, "Amount", &mut self.vignette_amount, 0.0..=100.0, 0, "%", false);
                    slider(ui, "Midpoint", &mut self.vignette_midpoint, 0.0..=100.0, 0, "%", false);
                    slider(ui, "Roundness", &mut self.vignette_roundness, -100.0..=100.0, 0, "", false);
                    slider(ui, "Feather", &mut self.vignette_feather, 0.0..=100.0, 0, "%", false);
                    slider(ui, "Highlights", &mut self.vignette_highlights, 0.0..=100.0, 0, "%", false);
                });
            }
            FilterKind::BloomGlow => grid(ui, "filter", |ui| {
                slider(ui, "Amount", &mut self.bloom_amount, 0.0..=100.0, 0, "%", false);
                slider(ui, "Radius", &mut self.bloom_radius, 1.0..=150.0, 0, " px", true);
            }),
            FilterKind::TonalContrast => grid(ui, "filter", |ui| {
                slider(ui, "Amount", &mut self.tonal_amount, 0.0..=100.0, 0, "%", false);
                slider(ui, "Shadows", &mut self.tonal_shadows, -100.0..=100.0, 0, "%", false);
                slider(ui, "Midtones", &mut self.tonal_midtones, -100.0..=100.0, 0, "%", false);
                slider(ui, "Highlights", &mut self.tonal_highlights, -100.0..=100.0, 0, "%", false);
                slider(ui, "Radius", &mut self.tonal_radius, 1.0..=100.0, 0, " px", true);
            }),
            FilterKind::LensCorrection => {
                grid(ui, "filter", |ui| {
                    slider(ui, "Remove Distortion", &mut self.distortion, -100.0..=100.0, 0, "", false);
                });
                ui.label("Positive straightens lines that bow outward (barrel); negative, lines that bow inward (pincushion).");
            }
        }
        *self != before
    }
}

static LAST: Mutex<Vec<FilterSettings>> = Mutex::new(Vec::new());

/// The vignette's strength at a point of a `width` × `height` frame (0 at its middle, 1 past its edges).
fn vignette_mask(px: f64, py: f64, width: f64, height: f64, midpoint: f64, roundness: f64, feather: f64) -> f64 {
    let nx = px / width * 2.0 - 1.0;
    let ny = py / height * 2.0 - 1.0;
    let square = nx.abs().max(ny.abs());
    let circle = nx.hypot(ny) / 2f64.sqrt();
    let shape = (1.0 - roundness / 100.0) * 0.5;
    let dist = circle + (square - circle) * shape;
    let start = (midpoint / 100.0) * 0.85;
    let soft = (feather / 100.0).max(0.05);
    let t = ((dist - start) / soft).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Radial distortion around the middle; `k` moves the corners by that share of their distance.
fn lens_distort(px: &mut [Px], width: usize, height: usize, k: f64) {
    if k == 0.0 {
        return;
    }
    let source = px.to_vec();
    let (cx, cy) = (width as f64 * 0.5, height as f64 * 0.5);
    let half_diagonal2 = cx * cx + cy * cy;
    px.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        let dy = y as f64 + 0.5 - cy;
        for (x, out) in row.iter_mut().enumerate() {
            let dx = x as f64 + 0.5 - cx;
            let s = 1.0 - k * (dx * dx + dy * dy) / half_diagonal2;
            let sx = cx + dx * s - 0.5;
            let sy = cy + dy * s - 0.5;
            let (fx0, fy0) = (sx.floor(), sy.floor());
            let (fx, fy) = ((sx - fx0) as f32, (sy - fy0) as f32);
            let mut sum = [0f32; 4];
            for (j, wy) in [(0i64, 1.0 - fy), (1, fy)] {
                let r = fy0 as i64 + j;
                if wy == 0.0 || r < 0 || r >= height as i64 {
                    continue;
                }
                for (i, wx) in [(0i64, 1.0 - fx), (1, fx)] {
                    let c = fx0 as i64 + i;
                    if wx == 0.0 || c < 0 || c >= width as i64 {
                        continue;
                    }
                    let p = source[r as usize * width + c as usize];
                    for n in 0..4 {
                        sum[n] += p[n] * wx * wy;
                    }
                }
            }
            *out = sum;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_filters_change_nothing() {
        let (w, h) = (24, 24);
        let original: Vec<Px> = (0..w * h).map(|i| [(i % w) as f32 / w as f32, 0.5, (i / w) as f32 / h as f32, 1.0]).collect();
        for kind in [FilterKind::LensCorrection, FilterKind::Vignette, FilterKind::TonalContrast] {
            let mut s = FilterSettings::new(kind);
            s.distortion = 0.0;
            s.vignette_amount = 0.0;
            s.tonal_amount = 0.0;
            let mut px = original.clone();
            s.run(&mut px, w, h, 1.0, 1, false);
            for (a, b) in px.iter().zip(&original) {
                assert!((0..4).all(|c| (a[c] - b[c]).abs() < 1e-5), "{kind:?}");
            }
        }
    }

    #[test]
    fn vignette_darkens_corners_not_the_middle() {
        let (w, h) = (40, 40);
        let mut px = vec![[1.0f32, 1.0, 1.0, 1.0]; w * h];
        let mut s = FilterSettings::new(FilterKind::Vignette);
        s.vignette_amount = 100.0;
        s.vignette_highlights = 0.0;
        s.run(&mut px, w, h, 1.0, 0, false);
        assert!(px[0][0] < 0.1);
        assert!((px[20 * w + 20][0] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn unsharp_mask_raises_edge_contrast() {
        let (w, h) = (20, 4);
        let mut px: Vec<Px> = (0..w * h).map(|i| if i % w < 10 { [0.3, 0.3, 0.3, 1.0] } else { [0.7, 0.7, 0.7, 1.0] }).collect();
        let mut s = FilterSettings::new(FilterKind::UnsharpMask);
        s.sharpen_radius = 2.0;
        s.run(&mut px, w, h, 1.0, 0, false);
        assert!(px[w + 9][0] < 0.3 && px[w + 10][0] > 0.7);
    }
}
