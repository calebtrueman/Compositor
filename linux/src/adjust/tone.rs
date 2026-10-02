//! Exposure, Gradient Map, Black & White, Color Balance and Grain, as in the macOS app's
//! `ImageAdjustments.swift` and `AdjustPixels.c`. Colors are straight sRGB, 0…1.

use serde::{Deserialize, Serialize};

use super::levels::Lut;

fn clamp(value: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(lo, hi)
    } else {
        fallback
    }
}

#[inline]
pub fn rec709(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// A straight sRGB color stored with an adjustment, 0–1 per channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdjustmentColor {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
}

impl AdjustmentColor {
    pub const BLACK: AdjustmentColor = AdjustmentColor { red: 0.0, green: 0.0, blue: 0.0 };
    pub const WHITE: AdjustmentColor = AdjustmentColor { red: 1.0, green: 1.0, blue: 1.0 };

    pub fn from_rgba8(c: [u8; 4]) -> Self {
        AdjustmentColor { red: c[0] as f64 / 255.0, green: c[1] as f64 / 255.0, blue: c[2] as f64 / 255.0 }
    }

    pub fn clamped(&self) -> Self {
        AdjustmentColor { red: clamp(self.red, 0.0, 1.0, 0.0), green: clamp(self.green, 0.0, 1.0, 0.0), blue: clamp(self.blue, 0.0, 1.0, 0.0) }
    }

    pub fn rgb(&self) -> [f32; 3] {
        let c = self.clamped();
        [c.red as f32, c.green as f32, c.blue as f32]
    }
}

/// Photoshop's Exposure: `exposure` (stops) scales linear light and `offset` shifts it, then gamma
/// correction bends the result. The same curve runs on every channel.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExposureSettings {
    /// Stops of light, −20…20.
    pub exposure: f64,
    /// Added in linear light, −0.5…0.5.
    pub offset: f64,
    /// Gamma correction, 0.01…9.99; above 1 brightens the midtones.
    pub gamma: f64,
}

impl Default for ExposureSettings {
    fn default() -> Self {
        ExposureSettings { exposure: 0.0, offset: 0.0, gamma: 1.0 }
    }
}

impl ExposureSettings {
    pub fn normalized(&self) -> Self {
        ExposureSettings {
            exposure: clamp(self.exposure, -20.0, 20.0, 0.0),
            offset: clamp(self.offset, -0.5, 0.5, 0.0),
            gamma: clamp(self.gamma, 0.01, 9.99, 1.0),
        }
    }

    pub fn value(&self, encoded: f64) -> f64 {
        let s = self.normalized();
        let scale = 2f64.powf(s.exposure);
        let mut linear = if encoded <= 0.04045 { encoded / 12.92 } else { ((encoded + 0.055) / 1.055).powf(2.4) };
        linear = (linear * scale + s.offset).max(0.0).powf(1.0 / s.gamma);
        let output = if linear <= 0.0031308 { linear * 12.92 } else { 1.055 * linear.powf(1.0 / 2.4) - 0.055 };
        output.clamp(0.0, 1.0)
    }

    pub fn tables(&self) -> Lut {
        Lut::from_fn(|_, v| self.value(v))
    }
}

/// Gradient Map: each pixel's brightness picks a color between `shadows` and `highlights` (the
/// other way round when reversed).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GradientMapSettings {
    pub shadows: AdjustmentColor,
    pub highlights: AdjustmentColor,
    pub reversed: bool,
}

impl Default for GradientMapSettings {
    fn default() -> Self {
        GradientMapSettings { shadows: AdjustmentColor::BLACK, highlights: AdjustmentColor::WHITE, reversed: false }
    }
}

impl GradientMapSettings {
    /// The colors for the darkest and lightest tones, in the order they apply.
    pub fn ends(&self) -> ([f32; 3], [f32; 3]) {
        if self.reversed {
            (self.highlights.rgb(), self.shadows.rgb())
        } else {
            (self.shadows.rgb(), self.highlights.rgb())
        }
    }

    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let (dark, light) = self.ends();
        let t = rec709(c).clamp(0.0, 1.0);
        [dark[0] + (light[0] - dark[0]) * t, dark[1] + (light[1] - dark[1]) * t, dark[2] + (light[2] - dark[2]) * t]
    }
}

/// Black & White, as Photoshop's: how bright each family of colors becomes in gray.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BlackWhiteSettings {
    pub reds: f64,
    pub yellows: f64,
    pub greens: f64,
    pub cyans: f64,
    pub blues: f64,
    pub magentas: f64,
    /// Color the result while keeping its tones, for a sepia or a cyanotype.
    pub tint: bool,
    pub tint_hue: f64,
    pub tint_saturation: f64,
}

impl Default for BlackWhiteSettings {
    fn default() -> Self {
        BlackWhiteSettings {
            reds: 40.0,
            yellows: 60.0,
            greens: 40.0,
            cyans: 60.0,
            blues: 20.0,
            magentas: 80.0,
            tint: false,
            tint_hue: 40.0,
            tint_saturation: 20.0,
        }
    }
}

impl BlackWhiteSettings {
    /// A color is min(r,g,b) of gray, plus (mid−min) of the secondary between its two brightest
    /// channels, plus (max−mid) of the primary of its brightest.
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let weights = [self.reds, self.yellows, self.greens, self.cyans, self.blues, self.magentas].map(|w| (w / 100.0) as f32);
        let [r, g, b] = c.map(|v| v.clamp(0.0, 1.0));
        let mx = r.max(g).max(b);
        let mn = r.min(g).min(b);
        let md = r + g + b - mx - mn;
        // 0 red, 1 yellow, 2 green, 3 cyan, 4 blue, 5 magenta
        let (primary, secondary) = if mx == r {
            (0, if g >= b { 1 } else { 5 })
        } else if mx == g {
            (2, if r >= b { 1 } else { 3 })
        } else {
            (4, if g >= r { 3 } else { 5 })
        };
        let gray = (mn + (md - mn) * weights[secondary] + (mx - md) * weights[primary]).clamp(0.0, 1.0);
        if !(self.tint && self.tint_saturation > 0.0) {
            return [gray; 3];
        }
        // The gray becomes the lightness of a color at the chosen hue.
        let gray = gray as f64;
        let chroma = (1.0 - (2.0 * gray - 1.0).abs()) * self.tint_saturation / 100.0;
        let hp = (self.tint_hue % 360.0) / 60.0;
        let x = chroma * (1.0 - ((hp % 2.0) - 1.0).abs());
        let (r1, g1, b1) = if hp < 1.0 {
            (chroma, x, 0.0)
        } else if hp < 2.0 {
            (x, chroma, 0.0)
        } else if hp < 3.0 {
            (0.0, chroma, x)
        } else if hp < 4.0 {
            (0.0, x, chroma)
        } else if hp < 5.0 {
            (x, 0.0, chroma)
        } else {
            (chroma, 0.0, x)
        };
        let m = gray - chroma / 2.0;
        [(r1 + m).clamp(0.0, 1.0) as f32, (g1 + m).clamp(0.0, 1.0) as f32, (b1 + m).clamp(0.0, 1.0) as f32]
    }
}

/// Color Balance: shifts color towards one end of each opposing pair, separately for shadows,
/// midtones and highlights.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ColorBalanceSettings {
    pub shadow_cyan_red: f64,
    pub shadow_magenta_green: f64,
    pub shadow_yellow_blue: f64,
    pub mid_cyan_red: f64,
    pub mid_magenta_green: f64,
    pub mid_yellow_blue: f64,
    pub highlight_cyan_red: f64,
    pub highlight_magenta_green: f64,
    pub highlight_yellow_blue: f64,
    pub preserve_luminosity: bool,
}

impl Default for ColorBalanceSettings {
    fn default() -> Self {
        ColorBalanceSettings {
            shadow_cyan_red: 0.0,
            shadow_magenta_green: 0.0,
            shadow_yellow_blue: 0.0,
            mid_cyan_red: 0.0,
            mid_magenta_green: 0.0,
            mid_yellow_blue: 0.0,
            highlight_cyan_red: 0.0,
            highlight_magenta_green: 0.0,
            highlight_yellow_blue: 0.0,
            preserve_luminosity: true,
        }
    }
}

/// How much a tone belongs to the shadows, midtones and highlights: three overlapping curves.
fn tonal_weights(v: f32) -> (f32, f32, f32) {
    let (a, b, scale) = (0.25f32, 0.333f32, 0.7f32);
    let s = ((v - b) / -a + 0.5).clamp(0.0, 1.0);
    let h = ((v + b - 1.0) / a + 0.5).clamp(0.0, 1.0);
    let m1 = ((v - b) / a + 0.5).clamp(0.0, 1.0);
    let m2 = ((v + b - 1.0) / -a + 0.5).clamp(0.0, 1.0);
    (s * scale, m1 * m2 * scale, h * scale)
}

impl ColorBalanceSettings {
    pub fn is_identity(&self) -> bool {
        [
            self.shadow_cyan_red,
            self.shadow_magenta_green,
            self.shadow_yellow_blue,
            self.mid_cyan_red,
            self.mid_magenta_green,
            self.mid_yellow_blue,
            self.highlight_cyan_red,
            self.highlight_magenta_green,
            self.highlight_yellow_blue,
        ]
        .iter()
        .all(|v| *v == 0.0)
    }

    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let shadows = [self.shadow_cyan_red, self.shadow_magenta_green, self.shadow_yellow_blue].map(|v| (v / 100.0) as f32);
        let mids = [self.mid_cyan_red, self.mid_magenta_green, self.mid_yellow_blue].map(|v| (v / 100.0) as f32);
        let highs = [self.highlight_cyan_red, self.highlight_magenta_green, self.highlight_yellow_blue].map(|v| (v / 100.0) as f32);
        let mut c = c.map(|v| v.clamp(0.0, 1.0));
        let luma = |c: &[f32; 3]| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
        let before = luma(&c);
        for i in 0..3 {
            let (s, m, h) = tonal_weights(c[i]);
            c[i] = (c[i] + shadows[i] * s + mids[i] * m + highs[i] * h).clamp(0.0, 1.0);
        }
        if self.preserve_luminosity {
            let after = luma(&c);
            if after > 0.0001 {
                let ratio = before / after;
                c = c.map(|v| (v * ratio).clamp(0.0, 1.0));
            }
        }
        c
    }
}

/// Film grain: brightness noise, strongest in the midtones, fixed in document space by `seed`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GrainSettings {
    /// Strength, 0–100.
    pub amount: f64,
    /// Grain scale in document pixels, 0.5–20.
    pub size: f64,
    /// 0–100: how much smaller, irregular detail roughens the main grain particles.
    pub roughness: f64,
    pub seed: u32,
}

impl Default for GrainSettings {
    fn default() -> Self {
        GrainSettings { amount: 25.0, size: 1.5, roughness: 50.0, seed: 0 }
    }
}

#[inline]
pub(crate) fn mix32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^= x >> 16;
    x
}

/// A value in −1…1 for an integer lattice point; two uniform halves give a triangular spread.
#[inline]
fn lattice(ix: i64, iy: i64, seed: u32) -> f32 {
    let h = mix32((ix as u32).wrapping_mul(0x9E3779B1) ^ mix32((iy as u32).wrapping_mul(0x85EBCA77) ^ seed));
    (h & 0xFFFF) as f32 / 65535.0 + (h >> 16) as f32 / 65535.0 - 1.0
}

/// Smooth seeded noise whose features follow `scale` document pixels.
#[inline]
fn grain_field(u: f64, v: f64, scale: f64, seed: u32) -> f32 {
    let cell_x = (u / scale).floor();
    let cell_y = (v / scale).floor();
    let mut tx = (u / scale - cell_x) as f32;
    let mut ty = (v / scale - cell_y) as f32;
    tx = tx * tx * (3.0 - 2.0 * tx);
    ty = ty * ty * (3.0 - 2.0 * ty);
    let (ix, iy) = (cell_x as i64, cell_y as i64);
    let n00 = lattice(ix, iy, seed);
    let n10 = lattice(ix + 1, iy, seed);
    let n01 = lattice(ix, iy + 1, seed);
    let n11 = lattice(ix + 1, iy + 1, seed);
    let top = n00 + (n10 - n00) * tx;
    let bottom = n01 + (n11 - n01) * tx;
    (top + (bottom - top) * ty) * 1.6
}

impl GrainSettings {
    pub fn normalized(&self) -> Self {
        GrainSettings {
            amount: clamp(self.amount, 0.0, 100.0, 25.0),
            size: clamp(self.size, 0.5, 20.0, 1.5),
            roughness: clamp(self.roughness, 0.0, 100.0, 50.0),
            seed: self.seed,
        }
    }

    /// Grains one row of straight pixels. Pixel `x` of the row covers document point
    /// `(origin.0 + (x + 0.5) · units, v)`.
    pub fn apply_row(&self, row: &mut [[f32; 4]], origin_x: f64, v: f64, units: f64) {
        let s = self.normalized();
        if s.amount <= 0.0 || !(units > 0.0) {
            return;
        }
        let strength = (s.amount / 100.0) as f32 * 0.35;
        let rough = (s.roughness / 100.0) as f32;
        let fine_seed = mix32(s.seed ^ 0xA511E9B3);
        let detail = (s.size * 0.35).max(0.5);
        for (x, p) in row.iter_mut().enumerate() {
            if p[3] <= 0.0 {
                continue;
            }
            let u = origin_x + (x as f64 + 0.5) * units;
            let smooth = grain_field(u, v, s.size, s.seed);
            let fine = grain_field(u, v, detail, fine_seed);
            let noise = smooth + (fine - smooth) * rough;
            let c = [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0), p[2].clamp(0.0, 1.0)];
            let level = rec709(c).min(1.0);
            // Film grain shows most in the midtones.
            let delta = noise * strength * (0.4 + 2.4 * level * (1.0 - level));
            for i in 0..3 {
                p[i] = (c[i] + delta).clamp(0.0, 1.0);
            }
        }
    }
}
