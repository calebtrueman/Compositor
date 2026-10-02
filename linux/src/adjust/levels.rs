//! Levels: per-channel input black/white points, gamma and output range, as in the macOS app's
//! `Levels.swift`. The composite RGB range runs after each channel's own.

use serde::{Deserialize, Serialize};

/// Which channel a Levels or Curves editor is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LevelsChannel {
    #[default]
    #[serde(rename = "RGB")]
    Rgb,
    Red,
    Green,
    Blue,
}

impl LevelsChannel {
    pub const ALL: [LevelsChannel; 4] = [LevelsChannel::Rgb, LevelsChannel::Red, LevelsChannel::Green, LevelsChannel::Blue];

    pub fn index(self) -> usize {
        match self {
            LevelsChannel::Rgb => 0,
            LevelsChannel::Red => 1,
            LevelsChannel::Green => 2,
            LevelsChannel::Blue => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LevelsChannel::Rgb => "RGB",
            LevelsChannel::Red => "Red",
            LevelsChannel::Green => "Green",
            LevelsChannel::Blue => "Blue",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LevelRange {
    pub black: f64,
    pub gamma: f64,
    pub white: f64,
    pub output_black: f64,
    pub output_white: f64,
}

impl Default for LevelRange {
    fn default() -> Self {
        LevelRange { black: 0.0, gamma: 1.0, white: 255.0, output_black: 0.0, output_white: 255.0 }
    }
}

fn clamp(n: f64, lo: f64, hi: f64, fallback: f64) -> f64 {
    if n.is_finite() {
        n.clamp(lo, hi)
    } else {
        fallback
    }
}

impl LevelRange {
    pub fn normalized(&self) -> Self {
        let black = clamp(self.black, 0.0, 254.0, 0.0);
        LevelRange {
            black,
            white: clamp(self.white, black + 1.0, 255.0, 255.0),
            gamma: clamp(self.gamma, 0.1, 9.99, 1.0),
            output_black: clamp(self.output_black, 0.0, 255.0, 0.0),
            output_white: clamp(self.output_white, 0.0, 255.0, 255.0),
        }
    }

    /// Maps a 0…1 value.
    pub fn apply(&self, value: f64) -> f64 {
        let s = self.normalized();
        let input = ((value * 255.0 - s.black) / (s.white - s.black)).clamp(0.0, 1.0);
        (s.output_black + input.powf(1.0 / s.gamma) * (s.output_white - s.output_black)) / 255.0
    }

    pub fn is_identity(&self) -> bool {
        self.normalized() == LevelRange::default()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LevelsSettings {
    pub channel: LevelsChannel,
    /// RGB, then red, green and blue.
    pub ranges: Vec<LevelRange>,
}

impl Default for LevelsSettings {
    fn default() -> Self {
        LevelsSettings { channel: LevelsChannel::Rgb, ranges: vec![LevelRange::default(); 4] }
    }
}

impl LevelsSettings {
    pub fn range(&self, index: usize) -> LevelRange {
        self.ranges.get(index).copied().unwrap_or_default()
    }

    pub fn is_identity(&self) -> bool {
        (0..4).all(|i| self.range(i).is_identity())
    }

    /// Individual channels, followed by the composite RGB adjustment.
    pub fn apply(&self, value: f64, channel: usize) -> f64 {
        self.range(0).apply(self.range(channel).apply(value))
    }

    /// Lookup tables for red, green and blue.
    pub fn tables(&self) -> Lut {
        Lut::from_fn(|channel, v| self.apply(v, channel + 1))
    }

    /// Photoshop's Auto buttons, from a histogram (RGB then red, green, blue).
    pub fn automatic(mode: LevelsAuto, histogram: &Histogram) -> LevelsSettings {
        let mut result = LevelsSettings::default();
        fn endpoints(bins: &[f64; 256]) -> Option<(f64, f64)> {
            let total: f64 = bins.iter().sum();
            if total <= 0.0 {
                return None;
            }
            let (mut low, mut high) = (0, 255);
            let mut sum = 0.0;
            for (i, b) in bins.iter().enumerate() {
                sum += b;
                if sum > total * 0.001 {
                    low = i;
                    break;
                }
            }
            sum = 0.0;
            for i in (0..256).rev() {
                sum += bins[i];
                if sum > total * 0.001 {
                    high = i;
                    break;
                }
            }
            (low < high).then_some((low as f64, high as f64))
        }
        if mode == LevelsAuto::Contrast {
            // A shared interval preserves channel relationships.
            let limits: Vec<(f64, f64)> = histogram.bins[1..].iter().filter_map(endpoints).collect();
            let low = limits.iter().map(|l| l.0).fold(f64::MAX, f64::min);
            let high = limits.iter().map(|l| l.1).fold(f64::MIN, f64::max);
            if !limits.is_empty() && low < high {
                result.ranges[0] = LevelRange { black: low, white: high, ..Default::default() };
            }
        } else {
            for c in 1..4 {
                let Some((low, high)) = endpoints(&histogram.bins[c]) else { continue };
                let mut range = LevelRange { black: low, white: high, ..Default::default() };
                if mode == LevelsAuto::Neutral {
                    let total: f64 = histogram.bins[c].iter().sum();
                    let mean = histogram.bins[c].iter().enumerate().map(|(i, b)| range.apply(i as f64 / 255.0) * b).sum::<f64>() / total;
                    if mean > 0.0 && mean < 1.0 {
                        range.gamma = (mean.ln() / 0.5f64.ln()).clamp(0.1, 9.99);
                    }
                }
                result.ranges[c] = range;
            }
        }
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelsAuto {
    Contrast,
    Color,
    Neutral,
}

/// Counts of straight 8-bit values: the RGB composite (the mean of the three channels, not a
/// luminance histogram), then red, green and blue. Weighted by alpha.
#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    pub bins: [[f64; 256]; 4],
}

impl Histogram {
    /// `coverage(i)` weights pixel `i` (for a selection), 0…1.
    pub fn of(pixels: &[[u8; 4]], coverage: impl Fn(usize) -> f32) -> Histogram {
        let mut bins = [[0.0f64; 256]; 4];
        for (i, p) in pixels.iter().enumerate() {
            if p[3] == 0 {
                continue;
            }
            let weight = p[3] as f64 / 255.0 * coverage(i) as f64;
            if weight <= 0.0 {
                continue;
            }
            for c in 0..3 {
                let value = ((p[c] as f64 * 255.0 / p[3] as f64).round() as usize).min(255);
                bins[c + 1][value] += weight;
                bins[0][value] += weight / 3.0;
            }
        }
        Histogram { bins }
    }

    /// Display-only vertical scale: linear, but isolated spikes are capped so a large solid
    /// background doesn't flatten everything else.
    pub fn display_scale(bins: &[f64; 256]) -> f64 {
        let peak = bins.iter().copied().filter(|b| b.is_finite() && *b > 0.0).fold(0.0, f64::max);
        if peak <= 0.0 {
            return 0.0;
        }
        let mut interior: Vec<f64> = bins[1..255].iter().copied().filter(|b| b.is_finite() && *b > 0.0).collect();
        if interior.is_empty() {
            return peak;
        }
        interior.sort_by(|a, b| a.total_cmp(b));
        let typical = interior[((interior.len() - 1) as f64 * 0.95) as usize];
        peak.min(typical * 4.0)
    }
}

/// Three 256-entry tables (red, green, blue) mapping 0…1 to 0…1, read with linear interpolation
/// between entries as the macOS app's `levels_apply` does.
pub struct Lut {
    pub tables: [[f32; 256]; 3],
}

impl Lut {
    pub fn from_fn(f: impl Fn(usize, f64) -> f64) -> Lut {
        let mut tables = [[0.0f32; 256]; 3];
        for (c, table) in tables.iter_mut().enumerate() {
            for (i, v) in table.iter_mut().enumerate() {
                *v = f(c, i as f64 / 255.0) as f32;
            }
        }
        Lut { tables }
    }

    #[inline]
    pub fn lookup(&self, channel: usize, value: f32) -> f32 {
        let x = (value * 255.0).clamp(0.0, 255.0);
        let lo = x as usize;
        let hi = (lo + 1).min(255);
        let t = &self.tables[channel];
        t[lo] + (t[hi] - t[lo]) * (x - lo as f32)
    }

    #[inline]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        [self.lookup(0, rgb[0]), self.lookup(1, rgb[1]), self.lookup(2, rgb[2])]
    }
}
