//! Hue/Saturation with Photoshop's color ranges, as in the macOS app's `HueSaturation.swift`.
//! Colors go through a 33³ lookup built from the settings, read with trilinear interpolation.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// The six color ranges plus Master, as in Photoshop's Ctrl+U.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ColorRange {
    Master,
    Reds,
    Yellows,
    Greens,
    Cyans,
    Blues,
    Magentas,
}

impl ColorRange {
    pub const ALL: [ColorRange; 7] = [
        ColorRange::Master,
        ColorRange::Reds,
        ColorRange::Yellows,
        ColorRange::Greens,
        ColorRange::Cyans,
        ColorRange::Blues,
        ColorRange::Magentas,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ColorRange::Master => "Master",
            ColorRange::Reds => "Reds",
            ColorRange::Yellows => "Yellows",
            ColorRange::Greens => "Greens",
            ColorRange::Cyans => "Cyans",
            ColorRange::Blues => "Blues",
            ColorRange::Magentas => "Magentas",
        }
    }

    /// Photoshop's starting hue band.
    pub fn default_band(self) -> HueBand {
        let (a, b, c, d) = match self {
            ColorRange::Master => (0.0, 0.0, 360.0, 360.0),
            ColorRange::Reds => (315.0, 345.0, 15.0, 45.0),
            ColorRange::Yellows => (15.0, 45.0, 75.0, 105.0),
            ColorRange::Greens => (75.0, 105.0, 135.0, 165.0),
            ColorRange::Cyans => (135.0, 165.0, 195.0, 225.0),
            ColorRange::Blues => (195.0, 225.0, 255.0, 285.0),
            ColorRange::Magentas => (255.0, 285.0, 315.0, 345.0),
        };
        HueBand { falloff_start: a, range_start: b, range_end: c, falloff_end: d }
    }
}

/// A hue band in degrees, wrapping at 360: full strength between `range_start` and `range_end`,
/// fading to nothing at `falloff_start` and `falloff_end`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HueBand {
    pub falloff_start: f64,
    pub range_start: f64,
    pub range_end: f64,
    pub falloff_end: f64,
}

fn wrap(value: f64) -> f64 {
    let r = value % 360.0;
    if r < 0.0 {
        r + 360.0
    } else {
        r
    }
}

impl HueBand {
    /// Degrees from `from` forward to `to`, always 0…360.
    pub fn forward(from: f64, to: f64) -> f64 {
        let delta = (to - from) % 360.0;
        if delta < 0.0 {
            delta + 360.0
        } else {
            delta
        }
    }

    /// 1 inside the range, ramping linearly through each falloff shoulder, 0 outside.
    pub fn weight(&self, hue: f64) -> f64 {
        let span = Self::forward(self.falloff_start, self.falloff_end);
        if span <= 0.0 {
            return 1.0; // Master covers everything.
        }
        let position = Self::forward(self.falloff_start, hue);
        if position > span {
            return 0.0;
        }
        let ramp_in = Self::forward(self.falloff_start, self.range_start);
        let plateau_end = Self::forward(self.falloff_start, self.range_end);
        if position < ramp_in {
            return if ramp_in > 0.0 { position / ramp_in } else { 1.0 };
        }
        if position <= plateau_end {
            return 1.0;
        }
        let ramp_out = span - plateau_end;
        if ramp_out > 0.0 {
            (span - position) / ramp_out
        } else {
            1.0
        }
    }

    pub fn handles(&self) -> [f64; 4] {
        [self.falloff_start, self.range_start, self.range_end, self.falloff_end]
    }

    /// Moves one handle, keeping the four in order and the band under a full circle.
    pub fn set_handle(&mut self, index: usize, degrees: f64) {
        let mut updated = *self;
        let value = wrap(degrees);
        match index {
            0 => updated.falloff_start = value,
            1 => updated.range_start = value,
            2 => updated.range_end = value,
            _ => updated.falloff_end = value,
        }
        let span = Self::forward(updated.falloff_start, updated.falloff_end);
        let to_start = Self::forward(updated.falloff_start, updated.range_start);
        let to_end = Self::forward(updated.falloff_start, updated.range_end);
        if span > 1.0 && span <= 350.0 && to_start <= to_end && to_end <= span {
            *self = updated;
        }
    }

    /// The whole band moved by `delta` degrees.
    pub fn rotated(&self, delta: f64) -> HueBand {
        HueBand {
            falloff_start: wrap(self.falloff_start + delta),
            range_start: wrap(self.range_start + delta),
            range_end: wrap(self.range_end + delta),
            falloff_end: wrap(self.falloff_end + delta),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RangeAdjustment {
    pub hue: f64,
    pub saturation: f64,
    pub lightness: f64,
}

impl RangeAdjustment {
    pub fn is_zero(&self) -> bool {
        *self == RangeAdjustment::default()
    }
}

/// Hue is −180…180 (0…360 when colorizing), Saturation −100…100 (0…100 colorizing), Lightness
/// −100…100. Each color range keeps its own values; Master applies everywhere.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HueSaturationSettings {
    /// Which range the sliders and spectrum edit.
    pub range: ColorRange,
    pub colorize: bool,
    /// Applies the selected range to everything outside its band instead.
    pub invert_range: bool,
    #[serde(with = "range_map")]
    pub adjustments: BTreeMap<ColorRange, RangeAdjustment>,
    #[serde(with = "range_map")]
    pub bands: BTreeMap<ColorRange, HueBand>,
}

impl Default for HueSaturationSettings {
    fn default() -> Self {
        HueSaturationSettings::new(0.0, 0.0, 0.0, false)
    }
}

impl HueSaturationSettings {
    pub fn new(hue: f64, saturation: f64, lightness: f64, colorize: bool) -> Self {
        let mut adjustments = BTreeMap::new();
        adjustments.insert(ColorRange::Master, RangeAdjustment { hue, saturation, lightness });
        HueSaturationSettings {
            range: ColorRange::Master,
            colorize,
            invert_range: false,
            adjustments,
            bands: ColorRange::ALL.iter().map(|r| (*r, r.default_band())).collect(),
        }
    }

    /// Photoshop's starting point when Colorize is switched on.
    pub fn colorize_start() -> Self {
        HueSaturationSettings::new(0.0, 25.0, 0.0, true)
    }

    /// The selected range's values, which the sliders edit.
    pub fn current(&self) -> RangeAdjustment {
        self.adjustments.get(&self.range).copied().unwrap_or_default()
    }

    pub fn current_mut(&mut self) -> &mut RangeAdjustment {
        self.adjustments.entry(self.range).or_default()
    }

    pub fn band(&self, range: ColorRange) -> HueBand {
        self.bands.get(&range).copied().unwrap_or_else(|| range.default_band())
    }

    pub fn is_identity(&self) -> bool {
        !self.colorize && self.adjustments.values().all(RangeAdjustment::is_zero)
    }

    /// How much a range applies to one hue: Master everywhere, others through their band.
    pub fn weight(&self, range: ColorRange, hue: f64) -> f64 {
        if range == ColorRange::Master {
            return 1.0;
        }
        let weight = self.band(range).weight(hue);
        if self.invert_range && range == self.range {
            1.0 - weight
        } else {
            weight
        }
    }

    pub fn is_valid(&self) -> bool {
        self.adjustments.values().all(|a| {
            a.hue.is_finite()
                && a.hue.abs() <= 360.0
                && a.saturation.is_finite()
                && a.saturation.abs() <= 100.0
                && a.lightness.is_finite()
                && a.lightness.abs() <= 100.0
        }) && self.bands.values().all(|b| b.handles().iter().all(|h| h.is_finite()))
    }

    /// How much every range shifts each whole degree of hue: (shift, saturation, lightness).
    fn hue_response(&self) -> Vec<[f64; 3]> {
        (0..=360)
            .map(|degree| {
                let mut response = [0.0; 3];
                for (range, adjustment) in &self.adjustments {
                    if adjustment.is_zero() {
                        continue;
                    }
                    let weight = self.weight(*range, degree as f64);
                    if weight <= 0.0 {
                        continue;
                    }
                    response[0] += adjustment.hue * weight;
                    response[1] += adjustment.saturation * weight;
                    response[2] += adjustment.lightness * weight;
                }
                response
            })
            .collect()
    }

    /// One color through the adjustment, all 0…1.
    pub fn adjust(&self, rgb: [f64; 3], response: &[[f64; 3]]) -> [f64; 3] {
        let (mut hue, mut saturation, mut lightness) = to_hsl(rgb);
        let lightness_amount;
        if self.colorize {
            let current = self.current();
            hue = current.hue % 360.0;
            saturation = (current.saturation / 100.0).clamp(0.0, 1.0);
            lightness_amount = current.lightness / 100.0;
        } else {
            // Every range contributes, weighted by how strongly it claims the original hue.
            let sampled = response[(hue.round().max(0.0) as usize).min(response.len() - 1)];
            lightness_amount = sampled[2] / 100.0;
            hue = (hue + sampled[0]) % 360.0;
            if hue < 0.0 {
                hue += 360.0;
            }
            saturation = adjusted_saturation(saturation, sampled[1]);
        }
        // Lightness pulls toward white above 0 and toward black below, reaching either at ±100.
        let amount = lightness_amount.clamp(-1.0, 1.0);
        lightness = if amount >= 0.0 { lightness + (1.0 - lightness) * amount } else { lightness * (1.0 + amount) };
        to_rgb(hue, saturation, lightness.clamp(0.0, 1.0))
    }

    /// The hue a spectrum swatch becomes, for the "after" bar.
    pub fn shifted_hue(&self, hue: f64) -> f64 {
        let shift: f64 = self.adjustments.iter().map(|(range, a)| a.hue * self.weight(*range, hue)).sum();
        wrap(hue + shift)
    }

    /// The lookup table for these settings, cached since adjustment layers redraw with the same
    /// settings on every frame.
    pub fn cube(&self) -> Arc<Cube> {
        static CUBES: Mutex<Vec<(HueSaturationSettings, Arc<Cube>)>> = Mutex::new(Vec::new());
        let mut cubes = CUBES.lock().unwrap();
        if let Some(index) = cubes.iter().position(|(s, _)| s == self) {
            let entry = cubes.remove(index);
            let cube = entry.1.clone();
            cubes.insert(0, entry);
            return cube;
        }
        let response = self.hue_response();
        let n = CUBE_SIZE;
        let step = (n - 1) as f64;
        let mut values = Vec::with_capacity(n * n * n);
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let c = self.adjust([r as f64 / step, g as f64 / step, b as f64 / step], &response);
                    values.push([c[0] as f32, c[1] as f32, c[2] as f32]);
                }
            }
        }
        let cube = Arc::new(Cube { values });
        cubes.insert(0, (self.clone(), cube.clone()));
        cubes.truncate(8);
        cube
    }
}

/// Photoshop's Saturation: below 0 it scales toward gray (−100 is gray); above 0 it divides by
/// what's left, so +50 doubles it and +100 takes any color all the way.
pub fn adjusted_saturation(saturation: f64, amount: f64) -> f64 {
    let amount = (amount / 100.0).clamp(-1.0, 1.0);
    if amount <= 0.0 {
        return (saturation * (1.0 + amount)).max(0.0);
    }
    if amount >= 1.0 {
        if saturation > 0.0 {
            1.0
        } else {
            0.0
        }
    } else {
        (saturation / (1.0 - amount)).min(1.0)
    }
}

pub fn to_hsl(rgb: [f64; 3]) -> (f64, f64, f64) {
    let [red, green, blue] = rgb;
    let high = red.max(green).max(blue);
    let low = red.min(green).min(blue);
    let lightness = (high + low) / 2.0;
    let delta = high - low;
    if delta <= 0.0 {
        return (0.0, 0.0, lightness);
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let mut hue = if high == red {
        (green - blue) / delta
    } else if high == green {
        (blue - red) / delta + 2.0
    } else {
        (red - green) / delta + 4.0
    };
    hue *= 60.0;
    if hue < 0.0 {
        hue += 360.0;
    }
    (hue, saturation.min(1.0), lightness)
}

pub fn to_rgb(hue: f64, saturation: f64, lightness: f64) -> [f64; 3] {
    if saturation <= 0.0 {
        return [lightness; 3];
    }
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue / 60.0;
    let second = chroma * (1.0 - ((sector % 2.0) - 1.0).abs());
    let base = lightness - chroma / 2.0;
    let (r, g, b) = match sector as i64 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    [(r + base).clamp(0.0, 1.0), (g + base).clamp(0.0, 1.0), (b + base).clamp(0.0, 1.0)]
}

/// Points per axis of the lookup: fast to build, smooth enough.
pub const CUBE_SIZE: usize = 33;

/// A color lookup of `CUBE_SIZE`³ entries, red varying fastest.
pub struct Cube {
    pub values: Vec<[f32; 3]>,
}

impl Cube {
    /// Blended between the eight nearest entries.
    #[inline]
    pub fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = CUBE_SIZE;
        let scale = (n - 1) as f32;
        let mut lo = [0usize; 3];
        let mut f = [0f32; 3];
        for c in 0..3 {
            let p = rgb[c].clamp(0.0, 1.0) * scale;
            lo[c] = (p as usize).min(n - 2);
            f[c] = p - lo[c] as f32;
        }
        let (dy, dz) = (n, n * n);
        let base = lo[0] + lo[1] * dy + lo[2] * dz;
        let v = &self.values;
        let mut out = [0f32; 3];
        for (c, o) in out.iter_mut().enumerate() {
            let at = |i: usize| v[i][c];
            let x00 = at(base) + (at(base + 1) - at(base)) * f[0];
            let x10 = at(base + dy) + (at(base + dy + 1) - at(base + dy)) * f[0];
            let x01 = at(base + dz) + (at(base + dz + 1) - at(base + dz)) * f[0];
            let x11 = at(base + dz + dy) + (at(base + dz + dy + 1) - at(base + dz + dy)) * f[0];
            let y0 = x00 + (x10 - x00) * f[1];
            let y1 = x01 + (x11 - x01) * f[1];
            *o = y0 + (y1 - y0) * f[2];
        }
        out
    }
}

/// The macOS app stores its range-keyed dictionaries the way Swift encodes a dictionary whose
/// keys aren't strings: an array alternating key and value. Objects are accepted too.
mod range_map {
    use super::*;

    pub fn serialize<S: Serializer, V: Serialize>(map: &BTreeMap<ColorRange, V>, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(map.len() * 2))?;
        for (k, v) in map {
            seq.serialize_element(k)?;
            seq.serialize_element(v)?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>, V: DeserializeOwned>(d: D) -> Result<BTreeMap<ColorRange, V>, D::Error> {
        use serde::de::Error;
        let value = Value::deserialize(d)?;
        let mut map = BTreeMap::new();
        match value {
            Value::Array(items) => {
                for pair in items.chunks(2) {
                    let [key, value] = pair else { return Err(D::Error::custom("odd-length range dictionary")) };
                    let key = ColorRange::deserialize(key).map_err(D::Error::custom)?;
                    map.insert(key, V::deserialize(value).map_err(D::Error::custom)?);
                }
            }
            Value::Object(entries) => {
                for (key, value) in entries {
                    let key = ColorRange::deserialize(Value::String(key)).map_err(D::Error::custom)?;
                    map.insert(key, V::deserialize(value).map_err(D::Error::custom)?);
                }
            }
            Value::Null => {}
            _ => return Err(D::Error::custom("expected a range dictionary")),
        }
        Ok(map)
    }
}
