//! Photoshop's blend modes on gamma-encoded sRGB components in 0…1, as Photoshop computes them.

use crate::doc::BlendMode;

#[inline]
fn screen(b: f32, s: f32) -> f32 {
    b + s - b * s
}

#[inline]
fn color_burn(b: f32, s: f32) -> f32 {
    if b >= 1.0 {
        1.0
    } else if s <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - b) / s).min(1.0)
    }
}

#[inline]
fn color_dodge(b: f32, s: f32) -> f32 {
    if b <= 0.0 {
        0.0
    } else if s >= 1.0 {
        1.0
    } else {
        (b / (1.0 - s)).min(1.0)
    }
}

#[inline]
fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        b * 2.0 * s
    } else {
        screen(b, 2.0 * s - 1.0)
    }
}

#[inline]
fn soft_light(b: f32, s: f32) -> f32 {
    // Photoshop's variant (square root above mid gray), not the W3C one.
    if s <= 0.5 {
        b - (1.0 - 2.0 * s) * b * (1.0 - b)
    } else {
        b + (2.0 * s - 1.0) * (b.max(0.0).sqrt() - b)
    }
}

#[inline]
fn vivid_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        color_burn(b, 2.0 * s)
    } else {
        color_dodge(b, 2.0 * (s - 0.5))
    }
}

#[inline]
pub fn separable(mode: BlendMode, b: f32, s: f32) -> f32 {
    match mode {
        BlendMode::Normal => s,
        BlendMode::Darken => b.min(s),
        BlendMode::Multiply => b * s,
        BlendMode::ColorBurn => color_burn(b, s),
        BlendMode::LinearBurn => (b + s - 1.0).max(0.0),
        BlendMode::Lighten => b.max(s),
        BlendMode::Screen => screen(b, s),
        BlendMode::ColorDodge => color_dodge(b, s),
        BlendMode::LinearDodge => (b + s).min(1.0),
        BlendMode::Overlay => hard_light(s, b),
        BlendMode::SoftLight => soft_light(b, s),
        BlendMode::HardLight => hard_light(b, s),
        BlendMode::VividLight => vivid_light(b, s),
        BlendMode::LinearLight => (b + 2.0 * s - 1.0).clamp(0.0, 1.0),
        BlendMode::PinLight => {
            if s <= 0.5 {
                b.min(2.0 * s)
            } else {
                b.max(2.0 * s - 1.0)
            }
        }
        BlendMode::HardMix => {
            if b + s >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        BlendMode::Difference => (b - s).abs(),
        BlendMode::Exclusion => b + s - 2.0 * b * s,
        BlendMode::Subtract => (b - s).max(0.0),
        BlendMode::Divide => {
            if s <= 0.0 {
                if b <= 0.0 { 0.0 } else { 1.0 }
            } else {
                (b / s).min(1.0)
            }
        }
        BlendMode::Hue | BlendMode::Saturation | BlendMode::Color | BlendMode::Luminosity => s,
    }
}

#[inline]
fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

#[inline]
fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        let d = l - n;
        for v in &mut out {
            *v = if d > 0.0 { l + (*v - l) * l / d } else { l };
        }
    }
    if x > 1.0 {
        let d = x - l;
        for v in &mut out {
            *v = if d > 0.0 { l + (*v - l) * (1.0 - l) / d } else { l };
        }
    }
    out
}

#[inline]
fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

#[inline]
fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

#[inline]
fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let range = max - min;
    let mut out = [0.0; 3];
    if range > 0.0 {
        for i in 0..3 {
            out[i] = (c[i] - min) * s / range;
        }
    }
    out
}

/// The blended color B(backdrop, source) for any mode, on straight colors.
#[inline]
pub fn blend(mode: BlendMode, b: [f32; 3], s: [f32; 3]) -> [f32; 3] {
    match mode {
        BlendMode::Normal => s,
        BlendMode::Hue => set_lum(set_sat(s, sat(b)), lum(b)),
        BlendMode::Saturation => set_lum(set_sat(b, sat(s)), lum(b)),
        BlendMode::Color => set_lum(s, lum(b)),
        BlendMode::Luminosity => set_lum(b, lum(s)),
        _ => [separable(mode, b[0], s[0]), separable(mode, b[1], s[1]), separable(mode, b[2], s[2])],
    }
}

/// Composites a straight-alpha source color `s` with coverage `alpha` over a premultiplied
/// backdrop `dst`.
#[inline]
pub fn composite_over(mode: BlendMode, dst: &mut [f32; 4], s: [f32; 3], alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let ab = dst[3];
    if mode == BlendMode::Normal || ab <= 0.0 {
        let k = 1.0 - alpha;
        dst[0] = s[0] * alpha + dst[0] * k;
        dst[1] = s[1] * alpha + dst[1] * k;
        dst[2] = s[2] * alpha + dst[2] * k;
        dst[3] = alpha + ab * k;
        return;
    }
    let inv = 1.0 / ab;
    let b = [(dst[0] * inv).min(1.0), (dst[1] * inv).min(1.0), (dst[2] * inv).min(1.0)];
    let mixed = blend(mode, b, s);
    let k = 1.0 - alpha;
    for i in 0..3 {
        let m = (1.0 - ab) * s[i] + ab * mixed[i];
        dst[i] = alpha * m + k * dst[i];
    }
    dst[3] = alpha + ab * k;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiply_and_screen() {
        assert_eq!(separable(BlendMode::Multiply, 0.5, 0.5), 0.25);
        assert_eq!(separable(BlendMode::Screen, 0.5, 0.5), 0.75);
    }

    #[test]
    fn normal_over_opaque() {
        let mut d = [0.2, 0.2, 0.2, 1.0];
        composite_over(BlendMode::Normal, &mut d, [1.0, 0.0, 0.0], 0.5);
        assert!((d[0] - 0.6).abs() < 1e-6 && (d[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn luminosity_preserves_backdrop_hue() {
        let out = blend(BlendMode::Luminosity, [1.0, 0.0, 0.0], [0.3, 0.3, 0.3]);
        assert!(out[0] > out[1]);
    }
}
