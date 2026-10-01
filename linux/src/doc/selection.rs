use std::sync::Arc;

use image::{GrayImage, Luma};

/// A canvas-sized coverage mask (255 = fully selected). Session-only, like the macOS app's.
#[derive(Clone, Debug)]
pub struct Selection {
    pub mask: Arc<GrayImage>,
    /// Tight bounds of nonzero coverage, `(x0, y0, x1, y1)` exclusive.
    pub bounds: (u32, u32, u32, u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionOp {
    Replace,
    Add,
    Subtract,
    Intersect,
}

impl Selection {
    pub fn from_mask(mask: GrayImage) -> Option<Self> {
        let bounds = tight_bounds(&mask)?;
        Some(Selection { mask: Arc::new(mask), bounds })
    }

    pub fn all(width: u32, height: u32) -> Self {
        Selection { mask: Arc::new(GrayImage::from_pixel(width, height, Luma([255]))), bounds: (0, 0, width, height) }
    }

    pub fn coverage(&self, x: i64, y: i64) -> u8 {
        if x < 0 || y < 0 || x >= self.mask.width() as i64 || y >= self.mask.height() as i64 {
            0
        } else {
            self.mask.get_pixel(x as u32, y as u32)[0]
        }
    }

    pub fn inverted(&self) -> Option<Self> {
        let mut mask = (*self.mask).clone();
        for p in mask.pixels_mut() {
            p[0] = 255 - p[0];
        }
        Selection::from_mask(mask)
    }

    /// Combines `shape` (a canvas-sized coverage) into `current` with `op`.
    pub fn combine(current: Option<&Selection>, shape: GrayImage, op: SelectionOp) -> Option<Selection> {
        let Some(current) = current else {
            return match op {
                SelectionOp::Replace | SelectionOp::Add => Selection::from_mask(shape),
                _ => None,
            };
        };
        let mut result = (*current.mask).clone();
        for (r, s) in result.pixels_mut().zip(shape.pixels()) {
            let a = r[0] as u32;
            let b = s[0] as u32;
            r[0] = match op {
                SelectionOp::Replace => b,
                SelectionOp::Add => a.max(b),
                SelectionOp::Subtract => a * (255 - b) / 255,
                SelectionOp::Intersect => a.min(b),
            } as u8;
        }
        Selection::from_mask(result)
    }
}

pub fn tight_bounds(mask: &GrayImage) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = mask.dimensions();
    let mut b = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in 0..h {
        let row = &mask.as_raw()[(y * w) as usize..((y + 1) * w) as usize];
        if let Some(first) = row.iter().position(|v| *v > 0) {
            let last = row.iter().rposition(|v| *v > 0).unwrap();
            b.0 = b.0.min(first as u32);
            b.2 = b.2.max(last as u32 + 1);
            b.1 = b.1.min(y);
            b.3 = y + 1;
        }
    }
    (b.0 != u32::MAX).then_some(b)
}
