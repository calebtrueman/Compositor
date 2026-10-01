//! Layer effects: stroke, drop shadow, color overlay, inner shadow, outer glow and inner glow. Saved
//! as the macOS app's `effects` record (unknown fields survive in `extra` maps) and drawn around the
//! layer's placed pixels on the CPU, in the macOS app's order: drop shadow and outer glow behind, an
//! outside stroke, the pixels, then color overlay, inner glow, inner shadow and an inside stroke.

use std::sync::Mutex;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::project::Project;
use crate::render::{Buffer, Region};

/// Widest stroke, blur and glow the macOS app accepts (layer pixels).
pub const MAX_SIZE: f64 = 500.0;
/// Longest shadow distance the macOS app accepts.
pub const MAX_DISTANCE: f64 = 5000.0;

/// A line drawn around what the layer shows, outside its edge or inside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StrokeEffect {
    /// Missing in older projects means visible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    pub size: f64,
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub opacity: f64,
    pub inside: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for StrokeEffect {
    fn default() -> Self {
        StrokeEffect {
            enabled: None,
            size: 4.0,
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            opacity: 1.0,
            inside: false,
            extra: Map::new(),
        }
    }
}

/// A shadow, behind the layer (drop shadow) or inside its edges (inner shadow).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShadowEffect {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Where the light comes from, in degrees counterclockwise from the right, as Photoshop's
    /// dial is: 90 is from straight above, which drops the shadow straight down.
    pub angle: f64,
    pub distance: f64,
    pub blur: f64,
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub opacity: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for ShadowEffect {
    fn default() -> Self {
        ShadowEffect {
            enabled: None,
            angle: 90.0,
            distance: 20.0,
            blur: 20.0,
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            opacity: 0.5,
            extra: Map::new(),
        }
    }
}

impl ShadowEffect {
    /// The inner shadow's defaults, which sit closer than a drop shadow's.
    pub fn inner() -> Self {
        ShadowEffect { distance: 10.0, blur: 10.0, ..ShadowEffect::default() }
    }

    /// Where the shadow falls, in pixels (y grows downward): away from the light.
    pub fn offset(&self) -> (f64, f64) {
        let radians = self.angle.to_radians();
        (-radians.cos() * self.distance, radians.sin() * self.distance)
    }
}

/// A flat color over everything the layer shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorOverlayEffect {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub opacity: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for ColorOverlayEffect {
    fn default() -> Self {
        ColorOverlayEffect { enabled: None, red: 0.0, green: 0.0, blue: 0.0, opacity: 1.0, extra: Map::new() }
    }
}

/// A soft glow, around the outside of what the layer shows or inward from its edges.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlowEffect {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    pub size: f64,
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub opacity: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for GlowEffect {
    fn default() -> Self {
        GlowEffect { enabled: None, size: 20.0, red: 1.0, green: 1.0, blue: 1.0, opacity: 0.75, extra: Map::new() }
    }
}

impl GlowEffect {
    /// The inner glow's defaults, which reach half as far as an outer glow's.
    pub fn inner() -> Self {
        GlowEffect { size: 10.0, ..GlowEffect::default() }
    }
}

/// What a layer draws around itself. A missing record means the layer doesn't have that effect.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerEffects {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<StrokeEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<ShadowEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_overlay: Option<ColorOverlayEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_shadow: Option<ShadowEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outer_glow: Option<GlowEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_glow: Option<GlowEffect>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The six effects, in the macOS app's list order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Stroke,
    Shadow,
    ColorOverlay,
    InnerShadow,
    OuterGlow,
    InnerGlow,
}

impl Kind {
    pub const ALL: [Kind; 6] =
        [Kind::Stroke, Kind::Shadow, Kind::ColorOverlay, Kind::InnerShadow, Kind::OuterGlow, Kind::InnerGlow];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Stroke => "Stroke",
            Kind::Shadow => "Drop Shadow",
            Kind::ColorOverlay => "Color Overlay",
            Kind::InnerShadow => "Inner Shadow",
            Kind::OuterGlow => "Outer Glow",
            Kind::InnerGlow => "Inner Glow",
        }
    }
}

impl LayerEffects {
    pub fn is_empty(&self) -> bool {
        Kind::ALL.iter().all(|k| !self.contains(*k))
    }

    pub fn contains(&self, kind: Kind) -> bool {
        match kind {
            Kind::Stroke => self.stroke.is_some(),
            Kind::Shadow => self.shadow.is_some(),
            Kind::ColorOverlay => self.color_overlay.is_some(),
            Kind::InnerShadow => self.inner_shadow.is_some(),
            Kind::OuterGlow => self.outer_glow.is_some(),
            Kind::InnerGlow => self.inner_glow.is_some(),
        }
    }

    /// The effect's `enabled` flag, if the layer has it.
    fn enabled_flag(&mut self, kind: Kind) -> Option<&mut Option<bool>> {
        match kind {
            Kind::Stroke => self.stroke.as_mut().map(|e| &mut e.enabled),
            Kind::Shadow => self.shadow.as_mut().map(|e| &mut e.enabled),
            Kind::ColorOverlay => self.color_overlay.as_mut().map(|e| &mut e.enabled),
            Kind::InnerShadow => self.inner_shadow.as_mut().map(|e| &mut e.enabled),
            Kind::OuterGlow => self.outer_glow.as_mut().map(|e| &mut e.enabled),
            Kind::InnerGlow => self.inner_glow.as_mut().map(|e| &mut e.enabled),
        }
    }

    pub fn is_enabled(&self, kind: Kind) -> bool {
        let flag = match kind {
            Kind::Stroke => self.stroke.as_ref().map(|e| e.enabled),
            Kind::Shadow => self.shadow.as_ref().map(|e| e.enabled),
            Kind::ColorOverlay => self.color_overlay.as_ref().map(|e| e.enabled),
            Kind::InnerShadow => self.inner_shadow.as_ref().map(|e| e.enabled),
            Kind::OuterGlow => self.outer_glow.as_ref().map(|e| e.enabled),
            Kind::InnerGlow => self.inner_glow.as_ref().map(|e| e.enabled),
        };
        flag.is_some_and(|e| e.unwrap_or(true))
    }

    pub fn set_enabled(&mut self, kind: Kind, enabled: bool) {
        if let Some(flag) = self.enabled_flag(kind) {
            *flag = Some(enabled);
        }
    }

    /// Adds the effect with its defaults; a layer that already has it keeps its settings.
    pub fn add(&mut self, kind: Kind) {
        match kind {
            Kind::Stroke => {
                self.stroke.get_or_insert_with(StrokeEffect::default);
            }
            Kind::Shadow => {
                self.shadow.get_or_insert_with(ShadowEffect::default);
            }
            Kind::ColorOverlay => {
                self.color_overlay.get_or_insert_with(ColorOverlayEffect::default);
            }
            Kind::InnerShadow => {
                self.inner_shadow.get_or_insert_with(ShadowEffect::inner);
            }
            Kind::OuterGlow => {
                self.outer_glow.get_or_insert_with(GlowEffect::default);
            }
            Kind::InnerGlow => {
                self.inner_glow.get_or_insert_with(GlowEffect::inner);
            }
        }
    }

    pub fn remove(&mut self, kind: Kind) {
        match kind {
            Kind::Stroke => self.stroke = None,
            Kind::Shadow => self.shadow = None,
            Kind::ColorOverlay => self.color_overlay = None,
            Kind::InnerShadow => self.inner_shadow = None,
            Kind::OuterGlow => self.outer_glow = None,
            Kind::InnerGlow => self.inner_glow = None,
        }
    }

    /// Only the effects that are shown.
    pub fn visible(&self) -> LayerEffects {
        fn shown<T: Clone>(effect: &Option<T>, enabled: impl Fn(&T) -> Option<bool>) -> Option<T> {
            effect.as_ref().filter(|e| enabled(e).unwrap_or(true)).cloned()
        }
        LayerEffects {
            stroke: shown(&self.stroke, |e| e.enabled),
            shadow: shown(&self.shadow, |e| e.enabled),
            color_overlay: shown(&self.color_overlay, |e| e.enabled),
            inner_shadow: shown(&self.inner_shadow, |e| e.enabled),
            outer_glow: shown(&self.outer_glow, |e| e.enabled),
            inner_glow: shown(&self.inner_glow, |e| e.enabled),
            extra: Map::new(),
        }
    }
}

/// Document pixels the effects reach past the layer's own pixels. Inner effects count too: a tile
/// needs everything within their reach to draw its own pixels the same as its neighbors do.
pub fn margin(effects: &LayerEffects) -> f64 {
    let e = effects.visible();
    let mut reach: f64 = 0.0;
    if let Some(stroke) = &e.stroke {
        reach = reach.max(clamp_size(stroke.size));
    }
    for shadow in [&e.shadow, &e.inner_shadow].into_iter().flatten() {
        reach = reach.max(shadow.distance.clamp(0.0, MAX_DISTANCE) + blur_reach(shadow.blur));
    }
    for glow in [&e.outer_glow, &e.inner_glow].into_iter().flatten() {
        reach = reach.max(blur_reach(glow.size));
    }
    if e.is_empty() {
        0.0
    } else {
        reach.ceil() + 2.0
    }
}

/// How far a blur of `size` reads: three standard deviations (the deviation is half the size),
/// with room for the box approximation's rounding.
fn blur_reach(size: f64) -> f64 {
    clamp_size(size) * 2.0
}

fn clamp_size(size: f64) -> f64 {
    if size.is_finite() {
        size.clamp(0.0, MAX_SIZE)
    } else {
        0.0
    }
}

fn unit(v: f64) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

pub fn is_empty(effects: &LayerEffects) -> bool {
    effects.is_empty()
}

/// The layer's placed pixels (premultiplied, `region.scale` document pixels per buffer pixel) with
/// its effects drawn. Only what lies within the buffer is seen, so the caller grows the region by
/// `margin` and crops; every pass is position-independent, so neighboring tiles agree.
pub fn apply(effects: &LayerEffects, content: Buffer, region: Region) -> Buffer {
    let e = effects.visible();
    let (w, h) = (content.width, content.height);
    if e.is_empty() || w == 0 || h == 0 {
        return content;
    }
    let scale = if region.scale.is_finite() && region.scale > 0.0 { region.scale } else { 1.0 };
    let shape: Vec<f32> = content.px.par_iter().map(|p| p[3].clamp(0.0, 1.0)).collect();
    let color = |r: f64, g: f64, b: f64| [unit(r), unit(g), unit(b)];

    // Each effect as a coverage map and its color with opacity.
    let stroke = e.stroke.as_ref().filter(|s| s.size > 0.0 && s.opacity > 0.0).map(|s| {
        let reach = clamp_size(s.size) / scale;
        let ring = ring(&shape, w, h, reach, s.inside);
        (ring, color(s.red, s.green, s.blue), unit(s.opacity), s.inside)
    });
    let shadow = e.shadow.as_ref().filter(|s| s.opacity > 0.0).map(|s| {
        let moved = moved_and_softened(&shape, w, h, s, scale);
        (moved, color(s.red, s.green, s.blue), unit(s.opacity))
    });
    let outer_glow = e.outer_glow.as_ref().filter(|g| g.size > 0.0 && g.opacity > 0.0).map(|g| {
        let mut soft = shape.clone();
        gaussian(&mut soft, w, h, clamp_size(g.size) / 2.0 / scale);
        // Kept to what lies outside the layer.
        soft.par_iter_mut().zip(&shape).for_each(|(s, a)| *s = (*s * (1.0 - a)).clamp(0.0, 1.0));
        (soft, color(g.red, g.green, g.blue), unit(g.opacity))
    });
    let overlay = e.color_overlay.as_ref().filter(|o| o.opacity > 0.0).map(|o| (color(o.red, o.green, o.blue), unit(o.opacity)));
    let inner_glow = e.inner_glow.as_ref().filter(|g| g.size > 0.0 && g.opacity > 0.0).map(|g| {
        let mut soft = shape.clone();
        gaussian(&mut soft, w, h, clamp_size(g.size) / 2.0 / scale);
        // The shape softened inward: strongest at the edge, gone deep inside.
        soft.par_iter_mut().zip(&shape).for_each(|(s, a)| *s = (a * (1.0 - *s)).clamp(0.0, 1.0));
        (soft, color(g.red, g.green, g.blue), unit(g.opacity))
    });
    let inner_shadow = e.inner_shadow.as_ref().filter(|s| s.opacity > 0.0).map(|s| {
        // What lies outside the layer, moved and softened, kept to the layer's own shape.
        let mut moved = moved_and_softened(&shape, w, h, s, scale);
        moved.par_iter_mut().zip(&shape).for_each(|(m, a)| *m = (a * (1.0 - *m)).clamp(0.0, 1.0));
        (moved, color(s.red, s.green, s.blue), unit(s.opacity))
    });

    let mut out = content;
    out.px.par_chunks_mut(w).enumerate().for_each(|(j, row)| {
        for (i, p) in row.iter_mut().enumerate() {
            let index = j * w + i;
            // Premultiplied, built up from the back.
            let mut acc = [0.0f32; 4];
            if let Some((map, c, opacity)) = &shadow {
                source_over(&mut acc, tint(*c, map[index] * opacity));
            }
            if let Some((map, c, opacity)) = &outer_glow {
                source_over(&mut acc, tint(*c, map[index] * opacity));
            }
            if let Some((map, c, opacity, false)) = &stroke {
                source_over(&mut acc, tint(*c, map[index] * opacity));
            }
            // The layer's own pixels over what is behind them.
            source_over(&mut acc, *p);
            if let Some((c, opacity)) = &overlay {
                source_over(&mut acc, tint(*c, shape[index] * opacity));
            }
            if let Some((map, c, opacity)) = &inner_glow {
                source_over(&mut acc, tint(*c, map[index] * opacity));
            }
            if let Some((map, c, opacity)) = &inner_shadow {
                source_over(&mut acc, tint(*c, map[index] * opacity));
            }
            if let Some((map, c, opacity, true)) = &stroke {
                source_over(&mut acc, tint(*c, map[index] * opacity));
            }
            acc[3] = acc[3].clamp(0.0, 1.0);
            *p = acc;
        }
    });
    out
}

/// A straight color at `coverage`, premultiplied.
#[inline]
fn tint(c: [f32; 3], coverage: f32) -> [f32; 4] {
    let a = coverage.clamp(0.0, 1.0);
    [c[0] * a, c[1] * a, c[2] * a, a]
}

/// Premultiplied `top` over `acc`.
#[inline]
fn source_over(acc: &mut [f32; 4], top: [f32; 4]) {
    let keep = 1.0 - top[3].clamp(0.0, 1.0);
    for k in 0..4 {
        acc[k] = top[k] + acc[k] * keep;
    }
}

/// Where a stroke lands: the shape grown (or shrunk) by `reach` buffer pixels, less the shape
/// itself. A square reach, as in the macOS app: a round one eats into the corners of a rectangle.
/// Below a pixel (zoomed far out) the one-pixel ring fades by the fraction instead.
fn ring(shape: &[f32], w: usize, h: usize, reach: f64, inside: bool) -> Vec<f32> {
    let whole = reach.round().max(1.0) as usize;
    let fraction = reach.min(1.0) as f32;
    let mut moved = shape.to_vec();
    separable(&mut moved, w, h, |input, output| extreme_line(input, output, whole, inside));
    moved
        .par_iter_mut()
        .zip(shape)
        .for_each(|(m, a)| *m = (if inside { a - *m } else { *m - a }).clamp(0.0, 1.0) * fraction);
    moved
}

/// The shape moved by a shadow's offset and softened by its blur, both scaled to the buffer.
fn moved_and_softened(shape: &[f32], w: usize, h: usize, shadow: &ShadowEffect, scale: f64) -> Vec<f32> {
    let mut shadow = shadow.clone();
    shadow.distance = if shadow.distance.is_finite() { shadow.distance.clamp(0.0, MAX_DISTANCE) } else { 0.0 };
    if !shadow.angle.is_finite() {
        shadow.angle = 90.0;
    }
    let (dx, dy) = shadow.offset();
    let mut moved = shift(shape, w, h, dx / scale, dy / scale);
    gaussian(&mut moved, w, h, clamp_size(shadow.blur) / 2.0 / scale);
    moved
}

/// `source` moved by `(dx, dy)` buffer pixels, between pixels where the offset is fractional.
/// Nothing comes in from past the edges.
fn shift(source: &[f32], w: usize, h: usize, dx: f64, dy: f64) -> Vec<f32> {
    let (ix, iy) = (dx.floor(), dy.floor());
    let (fx, fy) = ((dx - ix) as f32, (dy - iy) as f32);
    let (ix, iy) = (ix as i64, iy as i64);
    let at = |x: i64, y: i64| -> f32 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            0.0
        } else {
            source[y as usize * w + x as usize]
        }
    };
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(j, row)| {
        let sy = j as i64 - iy;
        for (i, o) in row.iter_mut().enumerate() {
            let sx = i as i64 - ix;
            // Pixel (i, j) takes from (i - dx, j - dy), which lies between (sx - 1, sy - 1) and (sx, sy).
            let top = at(sx - 1, sy - 1) * fx + at(sx, sy - 1) * (1.0 - fx);
            let bottom = at(sx - 1, sy) * fx + at(sx, sy) * (1.0 - fx);
            *o = top * fy + bottom * (1.0 - fy);
        }
    });
    out
}

/// A gaussian blur of standard deviation `sigma` buffer pixels: an exact kernel when it's small,
/// otherwise three box passes, whose cost doesn't grow with the size.
fn gaussian(data: &mut Vec<f32>, w: usize, h: usize, sigma: f64) {
    if !(sigma > 0.01) {
        return;
    }
    if sigma < 2.0 {
        let radius = (sigma * 3.0).ceil() as usize;
        let kernel: Vec<f32> =
            (0..=2 * radius).map(|i| (-((i as f64 - radius as f64).powi(2)) / (2.0 * sigma * sigma)).exp() as f32).collect();
        let total: f32 = kernel.iter().sum();
        let kernel: Vec<f32> = kernel.iter().map(|k| k / total).collect();
        separable(data, w, h, |input, output| convolve_line(input, output, &kernel));
    } else {
        let radii = box_radii(sigma);
        separable(data, w, h, |input, output| {
            let mut scratch = input.to_vec();
            for (n, radius) in radii.iter().enumerate() {
                if n % 2 == 0 {
                    box_line(&scratch, output, *radius);
                } else {
                    box_line(output, &mut scratch, *radius);
                }
            }
            // Three passes leave the result in `output`.
        });
    }
}

/// Radii of three box blurs whose sum approximates a gaussian of `sigma` (Kutskir's method).
fn box_radii(sigma: f64) -> [usize; 3] {
    let n = 3.0;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut lower = ideal.floor() as i64;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let lower = lower.max(1);
    let upper = lower + 2;
    let l = lower as f64;
    let m = ((12.0 * sigma * sigma - n * l * l - 4.0 * n * l - 3.0 * n) / (-4.0 * l - 4.0)).round() as i64;
    let mut radii = [0usize; 3];
    for (i, r) in radii.iter_mut().enumerate() {
        let width = if (i as i64) < m { lower } else { upper };
        *r = ((width - 1) / 2) as usize;
    }
    radii
}

/// A running-sum box average of `radius` along one line; past the ends is empty.
fn box_line(input: &[f32], output: &mut [f32], radius: usize) {
    let n = input.len();
    let inv = 1.0 / (2 * radius + 1) as f64;
    let mut sum: f64 = input[..radius.min(n)].iter().map(|v| *v as f64).sum();
    for i in 0..n {
        if i + radius < n {
            sum += input[i + radius] as f64;
        }
        if i > radius {
            sum -= input[i - radius - 1] as f64;
        }
        output[i] = (sum * inv).max(0.0) as f32;
    }
}

fn convolve_line(input: &[f32], output: &mut [f32], kernel: &[f32]) {
    let n = input.len() as i64;
    let radius = (kernel.len() / 2) as i64;
    for (i, o) in output.iter_mut().enumerate() {
        let mut total = 0.0;
        for (k, weight) in kernel.iter().enumerate() {
            let x = i as i64 + k as i64 - radius;
            if x >= 0 && x < n {
                total += input[x as usize] * weight;
            }
        }
        *o = total;
    }
}

/// The largest (or smallest) value within `reach` on each side, with a monotonic queue, so the
/// cost doesn't grow with the reach. Past the ends is empty.
fn extreme_line(input: &[f32], output: &mut [f32], reach: usize, smallest: bool) {
    let n = input.len();
    let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::with_capacity(2 * reach + 2);
    let mut next = 0;
    for center in 0..n {
        while next < n && next <= center + reach {
            let value = input[next];
            while let Some(&last) = queue.back() {
                let previous = input[last];
                if if smallest { previous < value } else { previous > value } {
                    break;
                }
                queue.pop_back();
            }
            queue.push_back(next);
            next += 1;
        }
        while queue.front().is_some_and(|&f| f + reach < center) {
            queue.pop_front();
        }
        let outside = center < reach || center + reach >= n;
        output[center] = if smallest && outside { 0.0 } else { queue.front().map_or(0.0, |&f| input[f]) };
    }
}

/// Runs `pass` along every row, then every column, in parallel.
fn separable(data: &mut Vec<f32>, w: usize, h: usize, pass: impl Fn(&[f32], &mut [f32]) + Sync) {
    let mut rows = vec![0.0f32; w * h];
    rows.par_chunks_mut(w).zip(data.par_chunks(w)).for_each(|(o, i)| pass(i, o));
    let columns = transpose(&rows, w, h);
    let mut done = vec![0.0f32; w * h];
    done.par_chunks_mut(h).zip(columns.par_chunks(h)).for_each(|(o, i)| pass(i, o));
    *data = transpose(&done, h, w);
}

/// `source` is `h` rows of `w`; the result is `w` rows of `h`.
fn transpose(source: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(h).enumerate().for_each(|(x, column)| {
        for (y, v) in column.iter_mut().enumerate() {
            *v = source[y * w + x];
        }
    });
    out
}

// MARK: - Panel and menu

/// The style Copy Layer Style took, for Paste Layer Style (this session only).
static COPIED: Mutex<Option<LayerEffects>> = Mutex::new(None);

/// Whether the active layer can have effects: a pixel or text layer.
fn can_edit(project: &Project) -> bool {
    project.doc.active_layer().is_some_and(|l| !l.is_group && l.adjustment.is_none())
}

/// Replaces the active layer's effects in one named undo step; empty effects are removed.
fn set_effects(project: &mut Project, name: &str, effects: LayerEffects) {
    let Some(id) = project.doc.active else { return };
    let new = (!effects.is_empty()).then_some(effects);
    if project.doc.layer(id).is_none_or(|l| l.effects == new) {
        return;
    }
    project.edit(name, |doc| {
        if let Some(layer) = doc.layer_mut(id) {
            layer.effects = new;
        }
    });
}

/// The Effects section of the Properties panel: each effect with its visibility and settings.
pub fn properties_ui(ui: &mut egui::Ui, project: &mut Project) {
    let Some(layer) = project.doc.active_layer() else { return };
    if layer.adjustment.is_some() {
        return;
    }
    let original = layer.effects.clone().unwrap_or_default();
    let mut effects = original.clone();
    let mut structural: Option<(String, LayerEffects)> = None;
    ui.horizontal(|ui| {
        ui.strong("Effects");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button(crate::ui::icons::PLUS, |ui| {
                for kind in Kind::ALL {
                    if ui.add_enabled(!effects.contains(kind), egui::Button::new(kind.name())).clicked() {
                        let mut added = effects.clone();
                        added.add(kind);
                        structural = Some((format!("Add {}", kind.name()), added));
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Add an effect");
        });
    });
    if effects.is_empty() {
        ui.weak("No effects");
    }
    for kind in Kind::ALL {
        if !effects.contains(kind) {
            continue;
        }
        ui.horizontal(|ui| {
            let mut enabled = effects.is_enabled(kind);
            if ui.checkbox(&mut enabled, kind.name()).changed() {
                effects.set_enabled(kind, enabled);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button(crate::ui::icons::TRASH).on_hover_text(format!("Remove {}", kind.name())).clicked() {
                    let mut removed = effects.clone();
                    removed.remove(kind);
                    structural = Some((format!("Remove {}", kind.name()), removed));
                }
            });
        });
        ui.indent(kind.name(), |ui| {
            egui::Grid::new(("effect", kind.name())).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                effect_ui(ui, &mut effects, kind);
            });
        });
    }
    if let Some((name, effects)) = structural {
        set_effects(project, &name, effects);
    } else if effects != original {
        let new = (!effects.is_empty()).then_some(effects);
        crate::ui::properties::change(project, |doc| {
            if let Some(l) = doc.active_layer_mut() {
                l.effects = new;
            }
        });
    }
}

fn effect_ui(ui: &mut egui::Ui, effects: &mut LayerEffects, kind: Kind) {
    match kind {
        Kind::Stroke => {
            let Some(e) = effects.stroke.as_mut() else { return };
            size_row(ui, "Size", &mut e.size, MAX_SIZE);
            ui.label("Position");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut e.inside, false, "Outside");
                ui.selectable_value(&mut e.inside, true, "Inside");
            });
            ui.end_row();
            color_rows(ui, &mut e.red, &mut e.green, &mut e.blue, &mut e.opacity);
        }
        Kind::Shadow | Kind::InnerShadow => {
            let e = if kind == Kind::Shadow { effects.shadow.as_mut() } else { effects.inner_shadow.as_mut() };
            let Some(e) = e else { return };
            ui.label("Angle");
            ui.add(egui::DragValue::new(&mut e.angle).speed(1.0).range(-360.0..=360.0).suffix("°"));
            ui.end_row();
            size_row(ui, "Distance", &mut e.distance, MAX_DISTANCE);
            size_row(ui, "Blur", &mut e.blur, MAX_SIZE);
            color_rows(ui, &mut e.red, &mut e.green, &mut e.blue, &mut e.opacity);
        }
        Kind::ColorOverlay => {
            let Some(e) = effects.color_overlay.as_mut() else { return };
            color_rows(ui, &mut e.red, &mut e.green, &mut e.blue, &mut e.opacity);
        }
        Kind::OuterGlow | Kind::InnerGlow => {
            let e = if kind == Kind::OuterGlow { effects.outer_glow.as_mut() } else { effects.inner_glow.as_mut() };
            let Some(e) = e else { return };
            size_row(ui, "Size", &mut e.size, MAX_SIZE);
            color_rows(ui, &mut e.red, &mut e.green, &mut e.blue, &mut e.opacity);
        }
    }
}

fn size_row(ui: &mut egui::Ui, label: &str, value: &mut f64, max: f64) {
    ui.label(label);
    ui.add(egui::Slider::new(value, 0.0..=max).logarithmic(true).suffix(" px").max_decimals(1));
    ui.end_row();
}

fn color_rows(ui: &mut egui::Ui, red: &mut f64, green: &mut f64, blue: &mut f64, opacity: &mut f64) {
    ui.label("Color");
    let mut rgb = [*red as f32, *green as f32, *blue as f32];
    if egui::color_picker::color_edit_button_rgb(ui, &mut rgb).changed() {
        [*red, *green, *blue] = rgb.map(|c| c.clamp(0.0, 1.0) as f64);
    }
    ui.end_row();
    ui.label("Opacity");
    let mut percent = *opacity * 100.0;
    if ui.add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix("%").max_decimals(0)).changed() {
        *opacity = (percent / 100.0).clamp(0.0, 1.0);
    }
    ui.end_row();
}

/// Layer > Layer Style items.
pub fn menu(ui: &mut egui::Ui, project: &mut Project) {
    let editable = can_edit(project);
    let current = project.doc.active_layer().and_then(|l| l.effects.clone()).unwrap_or_default();
    for kind in Kind::ALL {
        let has = current.contains(kind);
        let label = if has { format!("Remove {}", kind.name()) } else { format!("{}…", kind.name()) };
        if ui.add_enabled(editable, egui::Button::new(label)).clicked() {
            let mut effects = current.clone();
            if has {
                effects.remove(kind);
                set_effects(project, &format!("Remove {}", kind.name()), effects);
            } else {
                effects.add(kind);
                set_effects(project, &format!("Add {}", kind.name()), effects);
            }
            ui.close();
        }
    }
    ui.separator();
    if ui.add_enabled(editable && !current.is_empty(), egui::Button::new("Copy Layer Style")).clicked() {
        *COPIED.lock().unwrap() = Some(current.clone());
        ui.close();
    }
    let copied = COPIED.lock().unwrap().clone();
    if ui.add_enabled(editable && copied.is_some(), egui::Button::new("Paste Layer Style")).clicked() {
        if let Some(effects) = copied {
            set_effects(project, "Paste Layer Style", effects);
        }
        ui.close();
    }
    if ui.add_enabled(editable && !current.is_empty(), egui::Button::new("Clear Layer Style")).clicked() {
        set_effects(project, "Clear Layer Style", LayerEffects::default());
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_macos_fields() {
        let json = r#"{
            "stroke": {"enabled": false, "size": 6, "red": 1, "green": 0.5, "blue": 0, "opacity": 0.8, "inside": true, "future": 1},
            "shadow": {"angle": 120, "distance": 8, "blur": 4, "red": 0, "green": 0, "blue": 0, "opacity": 0.5},
            "colorOverlay": {"red": 0.2, "green": 0.3, "blue": 0.4, "opacity": 1},
            "innerShadow": {"angle": 90, "distance": 10, "blur": 10, "red": 0, "green": 0, "blue": 0, "opacity": 0.5},
            "outerGlow": {"size": 20, "red": 1, "green": 1, "blue": 1, "opacity": 0.75},
            "innerGlow": {"enabled": true, "size": 10, "red": 1, "green": 1, "blue": 1, "opacity": 0.75},
            "bevel": {"depth": 3}
        }"#;
        let effects: LayerEffects = serde_json::from_str(json).unwrap();
        let stroke = effects.stroke.as_ref().unwrap();
        assert_eq!(stroke.size, 6.0);
        assert!(stroke.inside);
        assert_eq!(stroke.enabled, Some(false));
        assert!(!effects.is_enabled(Kind::Stroke));
        assert!(effects.is_enabled(Kind::Shadow), "a missing `enabled` means visible");
        assert_eq!(effects.shadow.as_ref().unwrap().angle, 120.0);
        assert_eq!(effects.color_overlay.as_ref().unwrap().blue, 0.4);
        // Written back, the same record (unknown fields included) reads the same.
        let written = serde_json::to_value(&effects).unwrap();
        let original: Value = serde_json::from_str(json).unwrap();
        for key in ["stroke", "shadow", "colorOverlay", "innerShadow", "outerGlow", "innerGlow", "bevel"] {
            let a = &written[key];
            let b = &original[key];
            for (field, value) in b.as_object().unwrap() {
                assert_eq!(a[field].as_f64().or(a[field].as_bool().map(|v| v as u8 as f64)),
                           value.as_f64().or(value.as_bool().map(|v| v as u8 as f64)), "{key}.{field}");
            }
            assert_eq!(a.as_object().unwrap().len(), b.as_object().unwrap().len(), "{key}");
        }
        assert!(written["shadow"].get("enabled").is_none(), "a missing `enabled` stays missing");
        assert_eq!(serde_json::from_value::<LayerEffects>(written).unwrap(), effects);
    }

    #[test]
    fn new_effects_take_macos_defaults() {
        let mut effects = LayerEffects::default();
        for kind in Kind::ALL {
            effects.add(kind);
        }
        let json = serde_json::to_value(&effects).unwrap();
        assert_eq!(json["stroke"]["size"], 4.0);
        assert_eq!(json["stroke"]["inside"], false);
        assert_eq!(json["shadow"]["distance"], 20.0);
        assert_eq!(json["innerShadow"]["distance"], 10.0);
        assert_eq!(json["outerGlow"]["size"], 20.0);
        assert_eq!(json["innerGlow"]["size"], 10.0);
        assert_eq!(json["colorOverlay"]["opacity"], 1.0);
    }

    fn square(size: usize, from: usize, to: usize) -> Buffer {
        let mut buffer = Buffer::new(size, size);
        for y in from..to {
            for x in from..to {
                buffer.px[y * size + x] = [1.0, 0.0, 0.0, 1.0];
            }
        }
        buffer
    }

    fn region(size: usize, scale: f64) -> Region {
        Region { x: 0, y: 0, width: size, height: size, scale }
    }

    #[test]
    fn outside_stroke_widens_alpha_by_its_size() {
        let effects = LayerEffects {
            stroke: Some(StrokeEffect { size: 5.0, green: 1.0, ..StrokeEffect::default() }),
            ..LayerEffects::default()
        };
        let out = apply(&effects, square(40, 15, 25), region(40, 1.0));
        let alpha = |x: usize, y: usize| out.get(x, y)[3];
        // The square reaches out five pixels on every side (corners too) and no further.
        assert_eq!(alpha(10, 20), 1.0);
        assert_eq!(alpha(29, 20), 1.0);
        assert_eq!(alpha(10, 10), 1.0);
        assert_eq!(alpha(9, 20), 0.0);
        assert_eq!(alpha(30, 20), 0.0);
        assert_eq!(out.get(12, 20), [0.0, 1.0, 0.0, 1.0], "the stroke's color");
        assert_eq!(out.get(20, 20), [1.0, 0.0, 0.0, 1.0], "the pixels sit over an outside stroke");
        // At half resolution the stroke is half as many buffer pixels (2.5, rounded to 3).
        let half = apply(&effects, square(20, 7, 13), region(20, 2.0));
        assert!(half.get(4, 10)[3] > 0.99 && half.get(3, 10)[3] == 0.0);
    }

    #[test]
    fn inside_stroke_stays_within_the_shape() {
        let effects = LayerEffects {
            stroke: Some(StrokeEffect { size: 3.0, inside: true, blue: 1.0, ..StrokeEffect::default() }),
            ..LayerEffects::default()
        };
        let out = apply(&effects, square(40, 10, 30), region(40, 1.0));
        assert_eq!(out.get(9, 20)[3], 0.0);
        assert_eq!(out.get(10, 20), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(out.get(12, 20), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(out.get(13, 20), [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn drop_shadow_falls_below_and_tiles_agree() {
        let effects = LayerEffects {
            shadow: Some(ShadowEffect { distance: 6.0, blur: 6.0, opacity: 1.0, ..ShadowEffect::default() }),
            outer_glow: Some(GlowEffect::default()),
            ..LayerEffects::default()
        };
        let full = apply(&effects, square(64, 20, 40), region(64, 1.0));
        assert!(full.get(30, 44)[3] > 0.5, "the shadow falls straight down at 90°");
        assert!(full.get(30, 16)[3] < full.get(30, 44)[3]);
        // A tile cut from the same pixels, with the margin around it, matches the whole.
        let m = margin(&effects) as usize;
        let tile = Region { x: 30 - m as i64, y: 30 - m as i64, width: 2 * m + 4, height: 2 * m + 4, scale: 1.0 };
        let mut content = Buffer::new(tile.width, tile.height);
        for j in 0..tile.height {
            for i in 0..tile.width {
                let (x, y) = (tile.x + i as i64, tile.y + j as i64);
                if (20..40).contains(&x) && (20..40).contains(&y) {
                    content.px[j * tile.width + i] = [1.0, 0.0, 0.0, 1.0];
                }
            }
        }
        let piece = apply(&effects, content, tile);
        for j in 0..4 {
            for i in 0..4 {
                let a = piece.get(m + i, m + j);
                let b = full.get(30 + i, 30 + j);
                for k in 0..4 {
                    assert!((a[k] - b[k]).abs() < 1e-4, "{a:?} vs {b:?}");
                }
            }
        }
    }

    #[test]
    fn hidden_effects_draw_nothing() {
        let mut effects = LayerEffects::default();
        effects.add(Kind::Stroke);
        effects.set_enabled(Kind::Stroke, false);
        assert_eq!(margin(&effects), 0.0);
        let out = apply(&effects, square(20, 5, 15), region(20, 1.0));
        assert_eq!(out.get(4, 10)[3], 0.0);
    }

    #[test]
    fn box_blur_keeps_mass() {
        let mut data = vec![0.0f32; 101 * 101];
        data[50 * 101 + 50] = 1.0;
        gaussian(&mut data, 101, 101, 6.0);
        let total: f32 = data.iter().sum();
        assert!((total - 1.0).abs() < 1e-3);
        assert!(data[50 * 101 + 50] > data[50 * 101 + 56]);
    }
}
