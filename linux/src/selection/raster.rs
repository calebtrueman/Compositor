//! Canvas-sized coverage masks for selection shapes: rectangles, ellipses and polygons.

use image::{GrayImage, Luma};
use tiny_skia::{FillRule, Mask, Path, PathBuilder, Rect, Transform};

/// A whole-pixel rectangle `(x0, y0, x1, y1)`, clipped to the canvas.
pub fn rect_mask(rect: (i64, i64, i64, i64), width: u32, height: u32) -> GrayImage {
    let mut mask = GrayImage::new(width, height);
    let x0 = rect.0.clamp(0, width as i64) as u32;
    let x1 = rect.2.clamp(0, width as i64) as u32;
    let y0 = rect.1.clamp(0, height as i64) as u32;
    let y1 = rect.3.clamp(0, height as i64) as u32;
    for y in y0..y1 {
        for x in x0..x1 {
            mask.put_pixel(x, y, Luma([255]));
        }
    }
    mask
}

/// An ellipse inscribed in `(x0, y0, x1, y1)`.
pub fn ellipse_mask(rect: (f64, f64, f64, f64), width: u32, height: u32, anti_alias: bool) -> GrayImage {
    let path = Rect::from_ltrb(rect.0 as f32, rect.1 as f32, rect.2 as f32, rect.3 as f32).and_then(PathBuilder::from_oval);
    match path {
        Some(path) => path_mask(&path, width, height, anti_alias),
        None => GrayImage::new(width, height),
    }
}

/// A closed polygon through `points` (document coordinates), filled with the nonzero rule.
pub fn polygon_mask(points: &[(f64, f64)], width: u32, height: u32, anti_alias: bool) -> GrayImage {
    if points.len() < 3 {
        return GrayImage::new(width, height);
    }
    let mut builder = PathBuilder::new();
    builder.move_to(points[0].0 as f32, points[0].1 as f32);
    for p in &points[1..] {
        builder.line_to(p.0 as f32, p.1 as f32);
    }
    builder.close();
    match builder.finish() {
        Some(path) => path_mask(&path, width, height, anti_alias),
        None => GrayImage::new(width, height),
    }
}

fn path_mask(path: &Path, width: u32, height: u32, anti_alias: bool) -> GrayImage {
    let Some(mut mask) = Mask::new(width, height) else { return GrayImage::new(width, height) };
    // tiny-skia skips (with a log line) paths with no area; nothing would be selected anyway.
    let bounds = path.bounds();
    if bounds.width() > 0.0 && bounds.height() > 0.0 {
        mask.fill_path(path, FillRule::Winding, anti_alias, Transform::identity());
    }
    GrayImage::from_raw(width, height, mask.take()).unwrap_or_else(|| GrayImage::new(width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::selection::tight_bounds;

    #[test]
    fn polygon_rasterizes_inside() {
        let triangle = [(2.0, 2.0), (18.0, 2.0), (2.0, 18.0)];
        let mask = polygon_mask(&triangle, 20, 20, false);
        assert_eq!(mask.get_pixel(4, 4)[0], 255);
        assert_eq!(mask.get_pixel(16, 16)[0], 0);
        assert_eq!(tight_bounds(&mask), Some((2, 2, 18, 18)));
    }

    #[test]
    fn polygon_anti_aliases_edges() {
        let triangle = [(2.0, 2.0), (18.0, 2.0), (2.0, 18.0)];
        let mask = polygon_mask(&triangle, 20, 20, true);
        // Pixels the diagonal crosses are partly covered.
        assert!(mask.pixels().any(|p| p[0] > 0 && p[0] < 255));
        let hard = polygon_mask(&triangle, 20, 20, false);
        assert!(hard.pixels().all(|p| p[0] == 0 || p[0] == 255));
    }

    #[test]
    fn degenerate_polygons_select_nothing() {
        assert!(tight_bounds(&polygon_mask(&[(1.0, 1.0), (5.0, 5.0)], 10, 10, true)).is_none());
        assert!(tight_bounds(&polygon_mask(&[(1.0, 1.0), (5.0, 1.0), (9.0, 1.0)], 10, 10, true)).is_none());
    }

    #[test]
    fn rect_and_ellipse() {
        let r = rect_mask((-5, 3, 4, 50), 10, 10);
        assert_eq!(tight_bounds(&r), Some((0, 3, 4, 10)));
        let e = ellipse_mask((0.0, 0.0, 10.0, 10.0), 10, 10, true);
        assert_eq!(e.get_pixel(5, 5)[0], 255);
        assert!(e.get_pixel(0, 0)[0] < 40);
    }
}
