//! Curves: a smooth curve through points for the RGB composite and each channel, as in the macOS
//! app's `Curves.swift`.

use serde::{Deserialize, Serialize};

use super::levels::{LevelsChannel, Lut};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CurvesSettings {
    pub channel: LevelsChannel,
    /// RGB, then red, green and blue; each runs from x 0 to x 255 in increasing x.
    pub channels: Vec<Vec<CurvePoint>>,
}

pub fn identity_curve() -> Vec<CurvePoint> {
    vec![CurvePoint { x: 0.0, y: 0.0 }, CurvePoint { x: 255.0, y: 255.0 }]
}

impl Default for CurvesSettings {
    fn default() -> Self {
        CurvesSettings { channel: LevelsChannel::Rgb, channels: vec![identity_curve(); 4] }
    }
}

/// Most points a curve may have.
pub const MAX_POINTS: usize = 32;

pub fn is_valid_curve(points: &[CurvePoint]) -> bool {
    (2..=MAX_POINTS).contains(&points.len())
        && points.first().is_some_and(|p| p.x == 0.0)
        && points.last().is_some_and(|p| p.x == 255.0)
        && points.iter().all(|p| p.x.is_finite() && p.y.is_finite() && (0.0..=255.0).contains(&p.x) && (0.0..=255.0).contains(&p.y))
        && points.windows(2).all(|w| w[0].x < w[1].x)
}

impl CurvesSettings {
    pub fn is_valid(&self) -> bool {
        self.channels.len() == 4 && self.channels.iter().all(|c| is_valid_curve(c))
    }

    pub fn is_identity(&self) -> bool {
        self.channels.iter().all(|c| c.iter().all(|p| p.x == p.y))
    }

    /// The curve of `channel` at `x` (both 0…255). Shape-preserving cubic Hermite interpolation
    /// (Fritsch–Carlson slopes) avoids overshoot between handles.
    pub fn value(&self, x: f64, channel: usize) -> f64 {
        match self.channels.get(channel) {
            Some(points) if is_valid_curve(points) => curve_value(points, x),
            _ => x.clamp(0.0, 255.0),
        }
    }

    /// Each channel's curve followed by the RGB composite's, for red, green and blue.
    pub fn tables(&self) -> Lut {
        Lut::from_fn(|c, v| self.value(self.value(v * 255.0, c + 1), 0) / 255.0)
    }
}

pub fn curve_value(p: &[CurvePoint], x: f64) -> f64 {
    let n = p.len();
    let i = p.iter().rposition(|q| q.x <= x).unwrap_or(0).min(n - 2);
    let d: Vec<f64> = p.windows(2).map(|w| (w[1].y - w[0].y) / (w[1].x - w[0].x)).collect();
    let slope = |j: usize| -> f64 {
        if j == 0 {
            return d[0];
        }
        if j == n - 1 {
            return d[d.len() - 1];
        }
        if d[j - 1] * d[j] <= 0.0 {
            return 0.0;
        }
        2.0 / (1.0 / d[j - 1] + 1.0 / d[j])
    };
    let h = p[i + 1].x - p[i].x;
    let t = ((x - p[i].x) / h).clamp(0.0, 1.0);
    let (t2, t3) = (t * t, t * t * t);
    let y = (2.0 * t3 - 3.0 * t2 + 1.0) * p[i].y
        + (t3 - 2.0 * t2 + t) * h * slope(i)
        + (-2.0 * t3 + 3.0 * t2) * p[i + 1].y
        + (t3 - t2) * h * slope(i + 1);
    y.clamp(0.0, 255.0)
}
