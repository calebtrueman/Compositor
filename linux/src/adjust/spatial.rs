//! Neighborhood operations shared by adjustment layers and the Filter menu: Gaussian and motion
//! blur on premultiplied pixels (transparent past the edges, so a blur spreads and softens the
//! border rather than smearing it), and seeded noise.

use rayon::prelude::*;

use super::tone::mix32;

pub type Px = [f32; 4];

/// Gaussian blur with standard deviation `sigma` (pixels), in place. Small radii use an exact
/// separable kernel; larger ones three box passes, which converge on the same curve.
pub fn gaussian_blur(px: &mut [Px], width: usize, height: usize, sigma: f64) {
    if sigma < 0.05 || width == 0 || height == 0 {
        return;
    }
    if sigma <= 3.0 {
        let kernel = gaussian_kernel(sigma);
        convolve_rows(px, width, &kernel);
        let mut t = transpose(px, width, height);
        convolve_rows(&mut t, height, &kernel);
        transpose_into(&t, height, width, px);
    } else {
        let boxes = boxes_for_gauss(sigma, 3);
        for r in &boxes {
            box_rows(px, width, *r);
        }
        let mut t = transpose(px, width, height);
        for r in &boxes {
            box_rows(&mut t, height, *r);
        }
        transpose_into(&t, height, width, px);
    }
}

fn gaussian_kernel(sigma: f64) -> Vec<f32> {
    let radius = (sigma * 3.0).ceil().max(1.0) as i64;
    let mut k: Vec<f64> = (-radius..=radius).map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f64 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    k.into_iter().map(|v| v as f32).collect()
}

fn convolve_rows(px: &mut [Px], width: usize, kernel: &[f32]) {
    let radius = (kernel.len() / 2) as i64;
    px.par_chunks_mut(width).for_each(|row| {
        let source = row.to_vec();
        for (x, out) in row.iter_mut().enumerate() {
            let mut acc = [0f32; 4];
            for (k, w) in kernel.iter().enumerate() {
                let sx = x as i64 + k as i64 - radius;
                if sx < 0 || sx >= width as i64 {
                    continue;
                }
                let s = source[sx as usize];
                for c in 0..4 {
                    acc[c] += s[c] * w;
                }
            }
            *out = acc;
        }
    });
}

/// Radii of `n` box blurs approximating a Gaussian of `sigma`.
fn boxes_for_gauss(sigma: f64, n: usize) -> Vec<usize> {
    let ideal = (12.0 * sigma * sigma / n as f64 + 1.0).sqrt();
    let mut wl = ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m_ideal = (12.0 * sigma * sigma - (n as i64 * wl * wl) as f64 - (4 * n as i64 * wl) as f64 - 3.0 * n as f64) / (-4.0 * wl as f64 - 4.0);
    let m = m_ideal.round() as i64;
    (0..n as i64).map(|i| (((if i < m { wl } else { wu }) - 1) / 2).max(0) as usize).collect()
}

/// A box of `2r + 1` pixels along each row, zero past the ends.
fn box_rows(px: &mut [Px], width: usize, r: usize) {
    if r == 0 {
        return;
    }
    let norm = 1.0 / (2 * r + 1) as f32;
    px.par_chunks_mut(width).for_each(|row| {
        let source = row.to_vec();
        let mut acc = [0f64; 4];
        for s in source.iter().take(r.min(width)) {
            for c in 0..4 {
                acc[c] += s[c] as f64;
            }
        }
        for x in 0..width {
            let add = x + r;
            if add < width {
                for c in 0..4 {
                    acc[c] += source[add][c] as f64;
                }
            }
            if x > r {
                let sub = x - r - 1;
                for c in 0..4 {
                    acc[c] -= source[sub][c] as f64;
                }
            }
            for c in 0..4 {
                row[x][c] = (acc[c] as f32 * norm).max(0.0);
            }
        }
    });
}

fn transpose(px: &[Px], width: usize, height: usize) -> Vec<Px> {
    let mut out = vec![[0f32; 4]; width * height];
    transpose_into(px, width, height, &mut out);
    out
}

/// Writes the `width`×`height` image `px` transposed into `out` (`height`×`width`).
fn transpose_into(px: &[Px], width: usize, height: usize, out: &mut [Px]) {
    const TILE: usize = 64;
    out.par_chunks_mut(height * TILE).enumerate().for_each(|(band, chunk)| {
        // `chunk` holds output rows x0..x0+TILE (each `height` long).
        let x0 = band * TILE;
        let rows = chunk.len() / height;
        for y in 0..height {
            let src = &px[y * width..(y + 1) * width];
            for dx in 0..rows {
                chunk[dx * height + y] = src[x0 + dx];
            }
        }
    });
}

#[inline]
fn bilinear(px: &[Px], width: usize, height: usize, x: f32, y: f32) -> Px {
    let fx = x.floor();
    let fy = y.floor();
    let (tx, ty) = (x - fx, y - fy);
    let (x0, y0) = (fx as i64, fy as i64);
    let mut out = [0f32; 4];
    for (j, wy) in [(0i64, 1.0 - ty), (1, ty)] {
        let yy = y0 + j;
        if wy <= 0.0 || yy < 0 || yy >= height as i64 {
            continue;
        }
        for (i, wx) in [(0i64, 1.0 - tx), (1, tx)] {
            let xx = x0 + i;
            if wx <= 0.0 || xx < 0 || xx >= width as i64 {
                continue;
            }
            let p = px[yy as usize * width + xx as usize];
            let w = wx * wy;
            for c in 0..4 {
                out[c] += p[c] * w;
            }
        }
    }
    out
}

/// An even streak `distance` pixels long at `angle` degrees (counterclockwise from horizontal, as
/// in Photoshop), in place. Built from halving passes of two taps each: after k passes every pixel
/// averages 2^k evenly spaced samples along the streak, so it costs O(log distance) per pixel.
pub fn motion_blur(px: &mut [Px], width: usize, height: usize, angle: f64, distance: f64) {
    if distance <= 1.0 || width == 0 || height == 0 {
        return;
    }
    let (sin, cos) = angle.to_radians().sin_cos();
    // Image y points down.
    let dir = (cos as f32, -sin as f32);
    let passes = distance.log2().ceil().max(1.0) as u32;
    let mut source = px.to_vec();
    for k in 1..=passes {
        let offset = (distance / 2f64.powi(k as i32 + 1)) as f32;
        let (ox, oy) = (dir.0 * offset, dir.1 * offset);
        px.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
            for (x, out) in row.iter_mut().enumerate() {
                let (fx, fy) = (x as f32, y as f32);
                let a = bilinear(&source, width, height, fx - ox, fy - oy);
                let b = bilinear(&source, width, height, fx + ox, fy + oy);
                for c in 0..4 {
                    out[c] = (a[c] + b[c]) * 0.5;
                }
            }
        });
        if k < passes {
            source.copy_from_slice(px);
        }
    }
}

#[inline]
fn noise_hash(mut x: u32) -> u32 {
    x = mix32(x);
    x
}

/// Uniform in [0, 1).
#[inline]
fn noise_unit(key: u32) -> f32 {
    (noise_hash(key) >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// Photoshop's Add Noise on one row of straight pixels. Pixel `x` of the row is pattern cell
/// `(origin_x + x, py)`, so the same seed gives the same grain wherever a redraw starts.
pub fn add_noise_row(row: &mut [Px], amount: f32, gaussian: bool, monochromatic: bool, seed: u32, origin_x: i64, py: i64) {
    let spread = amount / 100.0 * 127.5;
    for (x, p) in row.iter_mut().enumerate() {
        if p[3] <= 0.0 {
            continue;
        }
        let cx = (origin_x + x as i64) as u32;
        let cy = py as u32;
        let base = noise_hash(seed ^ noise_hash(cx.wrapping_mul(0x9e3779b9) ^ noise_hash(cy.wrapping_mul(0x85ebca6b))));
        for (c, v) in p.iter_mut().take(3).enumerate() {
            let key = if monochromatic { base } else { base.wrapping_add((c as u32).wrapping_mul(0x9e3779b9)) };
            let n = if gaussian {
                // Box–Muller: two uniform values make one normally distributed one.
                let u1 = noise_unit(key);
                let u2 = noise_unit(key ^ 0x68e31da4);
                (-2.0 * (1.0 - u1).ln()).sqrt() * (std::f32::consts::TAU * u2).cos() * spread * (2.0 / 3.0)
            } else {
                (noise_unit(key) * 2.0 - 1.0) * spread
            };
            *v = ((*v * 255.0 + n).clamp(0.0, 255.0)) / 255.0;
        }
    }
}

/// Straight to premultiplied, in place.
pub fn premultiply(px: &mut [Px]) {
    px.par_iter_mut().for_each(|p| {
        p[0] *= p[3];
        p[1] *= p[3];
        p[2] *= p[3];
    });
}

/// Premultiplied to straight, in place.
pub fn unpremultiply(px: &mut [Px]) {
    px.par_iter_mut().for_each(|p| {
        if p[3] > 1e-6 {
            let inv = 1.0 / p[3];
            p[0] = (p[0] * inv).min(1.0);
            p[1] = (p[1] * inv).min(1.0);
            p[2] = (p[2] * inv).min(1.0);
        } else {
            *p = [0.0; 4];
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_keeps_flat_interior_and_total() {
        for sigma in [1.5, 8.0] {
            let (w, h) = (64, 48);
            let mut px = vec![[0.5f32, 0.25, 0.75, 1.0]; w * h];
            gaussian_blur(&mut px, w, h, sigma);
            let middle = px[24 * w + 32];
            assert!((middle[0] - 0.5).abs() < 1e-3 && (middle[3] - 1.0).abs() < 1e-3);
            // Past the edge is transparent, so the border softens.
            assert!(px[0][3] < 0.9);
        }
        let (w, h) = (101, 101);
        let mut px = vec![[0.0f32; 4]; w * h];
        px[50 * w + 50] = [1.0, 1.0, 1.0, 1.0];
        gaussian_blur(&mut px, w, h, 6.0);
        let total: f32 = px.iter().map(|p| p[3]).sum();
        assert!((total - 1.0).abs() < 1e-3);
    }

    #[test]
    fn motion_blur_spreads_along_its_angle() {
        let (w, h) = (101, 101);
        let mut px = vec![[0.0f32; 4]; w * h];
        px[50 * w + 50] = [1.0; 4];
        motion_blur(&mut px, w, h, 0.0, 20.0);
        assert!(px[50 * w + 58][3] > 0.0);
        assert_eq!(px[58 * w + 50][3], 0.0);
        let total: f32 = px.iter().map(|p| p[3]).sum();
        assert!((total - 1.0).abs() < 1e-3);
    }

    #[test]
    fn noise_is_stable_for_a_seed() {
        let mut a = vec![[0.5f32, 0.5, 0.5, 1.0]; 16];
        let mut b = a.clone();
        add_noise_row(&mut a, 25.0, true, false, 7, 3, 9);
        add_noise_row(&mut b, 25.0, true, false, 7, 3, 9);
        assert_eq!(a, b);
        assert!(a.iter().any(|p| (p[0] - 0.5).abs() > 0.01));
    }
}
