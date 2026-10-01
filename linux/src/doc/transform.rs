use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Sampling {
    Nearest,
    Smooth,
    #[default]
    #[serde(rename = "High quality")]
    High,
}

/// Unrotated bounds in document pixels; rotation is clockwise (y down) around their center, in
/// degrees. Flips mirror the layer within those bounds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerTransform {
    pub origin: [f64; 2],
    pub size: [f64; 2],
    #[serde(default)]
    pub rotation: f64,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,
    #[serde(default)]
    pub sampling: Sampling,
}

/// A 2×3 affine map: `x' = a·x + c·y + tx`, `y' = b·x + d·y + ty` (Core Graphics' layout).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y + self.tx, self.b * x + self.d * y + self.ty)
    }

    pub fn apply_vector(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y, self.b * x + self.d * y)
    }

    pub fn inverse(&self) -> Option<Affine> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let a = self.d / det;
        let b = -self.b / det;
        let c = -self.c / det;
        let d = self.a / det;
        Some(Affine { a, b, c, d, tx: -(a * self.tx + c * self.ty), ty: -(b * self.tx + d * self.ty) })
    }

    /// `self` after `other`: apply `other` first.
    pub fn then(&self, other: &Affine) -> Affine {
        Affine {
            a: other.a * self.a + other.c * self.b,
            b: other.b * self.a + other.d * self.b,
            c: other.a * self.c + other.c * self.d,
            d: other.b * self.c + other.d * self.d,
            tx: other.a * self.tx + other.c * self.ty + other.tx,
            ty: other.b * self.tx + other.d * self.ty + other.ty,
        }
    }

    pub fn translate(x: f64, y: f64) -> Affine {
        Affine { tx: x, ty: y, ..Affine::IDENTITY }
    }

    pub fn scale(x: f64, y: f64) -> Affine {
        Affine { a: x, d: y, ..Affine::IDENTITY }
    }
}

impl LayerTransform {
    pub fn rect(x: f64, y: f64, w: f64, h: f64) -> Self {
        LayerTransform {
            origin: [x, y],
            size: [w.max(1.0), h.max(1.0)],
            rotation: 0.0,
            flip_x: false,
            flip_y: false,
            sampling: Sampling::High,
        }
    }

    pub fn center(&self) -> (f64, f64) {
        (self.origin[0] + self.size[0] / 2.0, self.origin[1] + self.size[1] / 2.0)
    }

    pub fn radians(&self) -> f64 {
        (self.rotation % 360.0).to_radians()
    }

    pub fn is_valid(&self) -> bool {
        [self.origin[0], self.origin[1], self.size[0], self.size[1], self.rotation].iter().all(|v| v.is_finite())
            && (1.0..=300_000.0).contains(&self.size[0])
            && (1.0..=300_000.0).contains(&self.size[1])
            && self.origin[0].abs() <= 1_000_000.0
            && self.origin[1].abs() <= 1_000_000.0
    }

    /// Maps source pixel coordinates of a `width`×`height` image to document coordinates.
    pub fn pixel_to_document(&self, width: u32, height: u32) -> Affine {
        let (cx, cy) = self.center();
        let (sin, cos) = self.radians().sin_cos();
        let sx = self.size[0] / width.max(1) as f64 * if self.flip_x { -1.0 } else { 1.0 };
        let sy = self.size[1] / height.max(1) as f64 * if self.flip_y { -1.0 } else { 1.0 };
        let hw = width as f64 / 2.0;
        let hh = height as f64 / 2.0;
        // translate(-hw,-hh) then scale then rotate then translate(center)
        Affine::translate(-hw, -hh)
            .then(&Affine::scale(sx, sy))
            .then(&Affine { a: cos, b: sin, c: -sin, d: cos, tx: 0.0, ty: 0.0 })
            .then(&Affine::translate(cx, cy))
    }

    /// The unit square (0…1, y down) mapped onto the document.
    pub fn unit_to_document(&self) -> Affine {
        self.pixel_to_document(1, 1)
    }

    /// Document point of a unit-square point.
    pub fn point(&self, u: f64, v: f64) -> (f64, f64) {
        self.unit_to_document().apply(u, v)
    }

    pub fn corners(&self) -> [(f64, f64); 4] {
        [self.point(0.0, 0.0), self.point(1.0, 0.0), self.point(1.0, 1.0), self.point(0.0, 1.0)]
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        let (cx, cy) = self.center();
        let (sin, cos) = self.radians().sin_cos();
        let dx = x - cx;
        let dy = y - cy;
        (dx * cos + dy * sin).abs() <= self.size[0] / 2.0 && (-dx * sin + dy * cos).abs() <= self.size[1] / 2.0
    }

    /// Axis-aligned document bounds `(x0, y0, x1, y1)`.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        let corners = self.corners();
        let mut b = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (x, y) in corners {
            b.0 = b.0.min(x);
            b.1 = b.1.min(y);
            b.2 = b.2.max(x);
            b.3 = b.3.max(y);
        }
        b
    }

    /// Whole pixels and whole degrees.
    pub fn rounded(&self) -> Self {
        let mut r = *self;
        r.origin = [self.origin[0].round(), self.origin[1].round()];
        r.size = [self.size[0].round().max(1.0), self.size[1].round().max(1.0)];
        r.rotation = self.rotation.round();
        r
    }

    /// Whether `width`×`height` pixels land 1:1 on whole document pixels.
    pub fn is_pixel_aligned(&self, width: u32, height: u32) -> bool {
        self.rotation % 360.0 == 0.0
            && !self.flip_x
            && !self.flip_y
            && self.size[0] == width as f64
            && self.size[1] == height as f64
            && self.origin[0].fract() == 0.0
            && self.origin[1].fract() == 0.0
    }

    /// This placement carried along as its layer moves from `old` to `new` (for unlinked masks and
    /// layers transformed together).
    pub fn following(&self, old: &LayerTransform, new: &LayerTransform) -> LayerTransform {
        if old == new {
            return *self;
        }
        let Some(inverse) = old.unit_to_document().inverse() else { return *self };
        let map = self.unit_to_document().then(&inverse).then(&new.unit_to_document());
        let mut result = LayerTransform::from_affine(&map, self.rotation, self.flip_x);
        result.sampling = self.sampling;
        result
    }

    /// A transform placing the unit square as `map` does (shear dropped), keeping a rotation near
    /// `near_rotation` and the given horizontal flip.
    pub fn from_affine(map: &Affine, near_rotation: f64, flip_x: bool) -> LayerTransform {
        let sign = if flip_x { -1.0 } else { 1.0 };
        let angle = (map.b * sign).atan2(map.a * sign);
        let along = -map.c * angle.sin() + map.d * angle.cos();
        let (mx, my) = map.apply(0.5, 0.5);
        let width = map.a.hypot(map.b);
        let height = along.abs();
        let degrees = angle.to_degrees();
        let rotation = degrees + ((near_rotation - degrees) / 360.0).round() * 360.0;
        LayerTransform {
            origin: [mx - width / 2.0, my - height / 2.0],
            size: [width.max(1e-6), height.max(1e-6)],
            rotation,
            flip_x,
            flip_y: along < 0.0,
            sampling: Sampling::High,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_mapping_round_trips() {
        let t = LayerTransform { rotation: 30.0, flip_x: true, ..LayerTransform::rect(10.0, 20.0, 200.0, 100.0) };
        let m = t.pixel_to_document(50, 25);
        let inv = m.inverse().unwrap();
        let (x, y) = m.apply(12.0, 7.0);
        let (u, v) = inv.apply(x, y);
        assert!((u - 12.0).abs() < 1e-9 && (v - 7.0).abs() < 1e-9);
        let center = m.apply(25.0, 12.5);
        assert!((center.0 - 110.0).abs() < 1e-9 && (center.1 - 70.0).abs() < 1e-9);
    }

    #[test]
    fn from_affine_recovers_transform() {
        let t = LayerTransform { rotation: 45.0, ..LayerTransform::rect(5.0, 5.0, 40.0, 20.0) };
        let back = LayerTransform::from_affine(&t.unit_to_document(), 45.0, false);
        assert!((back.size[0] - 40.0).abs() < 1e-9 && (back.size[1] - 20.0).abs() < 1e-9);
        assert!((back.rotation - 45.0).abs() < 1e-9);
        assert!((back.origin[0] - 5.0).abs() < 1e-9);
    }
}
