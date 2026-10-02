//! Adjustment layers: settings that serialize exactly as the macOS app's manifest does
//! (`LayerAdjustment.swift`), the math that renders them, and their Properties editors. The same
//! math runs destructively from Image > Adjustments and the Filter menu (`crate::filters`).

pub mod curves;
pub mod hsv;
pub mod levels;
pub mod spatial;
pub mod tone;
pub mod ui;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use curves::CurvesSettings;
pub use hsv::HueSaturationSettings;
pub use levels::{Histogram, LevelsSettings};
pub use tone::{AdjustmentColor, BlackWhiteSettings, ColorBalanceSettings, ExposureSettings, GradientMapSettings, GrainSettings};
pub use ui::properties_ui;

use crate::render::{Buffer, Region};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AdjustmentKind {
    #[serde(rename = "Hue/Saturation")]
    HueSaturation,
    Levels,
    Curves,
    Exposure,
    #[serde(rename = "Gradient Map")]
    GradientMap,
    Grain,
    #[serde(rename = "Add Noise")]
    AddNoise,
    #[serde(rename = "Gaussian Blur")]
    GaussianBlur,
    #[serde(rename = "Motion Blur")]
    MotionBlur,
    Invert,
    #[serde(rename = "Black & White")]
    BlackWhite,
    #[serde(rename = "Color Balance")]
    ColorBalance,
}

/// The order adjustment kinds appear in menus.
pub const MENU_KINDS: &[AdjustmentKind] = &[
    AdjustmentKind::Levels,
    AdjustmentKind::Curves,
    AdjustmentKind::Exposure,
    AdjustmentKind::HueSaturation,
    AdjustmentKind::ColorBalance,
    AdjustmentKind::BlackWhite,
    AdjustmentKind::Invert,
    AdjustmentKind::GradientMap,
    AdjustmentKind::Grain,
    AdjustmentKind::GaussianBlur,
    AdjustmentKind::MotionBlur,
    AdjustmentKind::AddNoise,
];

pub fn kind_name(kind: AdjustmentKind) -> &'static str {
    match kind {
        AdjustmentKind::HueSaturation => "Hue/Saturation",
        AdjustmentKind::Levels => "Levels",
        AdjustmentKind::Curves => "Curves",
        AdjustmentKind::Exposure => "Exposure",
        AdjustmentKind::GradientMap => "Gradient Map",
        AdjustmentKind::Grain => "Grain",
        AdjustmentKind::AddNoise => "Add Noise",
        AdjustmentKind::GaussianBlur => "Gaussian Blur",
        AdjustmentKind::MotionBlur => "Motion Blur",
        AdjustmentKind::Invert => "Invert",
        AdjustmentKind::BlackWhite => "Black & White",
        AdjustmentKind::ColorBalance => "Color Balance",
    }
}

/// A glyph for the Layers panel.
pub fn symbol(kind: AdjustmentKind) -> &'static str {
    use crate::ui::icons;
    match kind {
        AdjustmentKind::HueSaturation => icons::CIRCLE_HALF,
        AdjustmentKind::Levels => icons::CHART_BAR,
        AdjustmentKind::Curves => icons::WAVE_SINE,
        AdjustmentKind::Exposure => icons::PLUS_MINUS,
        AdjustmentKind::GradientMap => icons::GRADIENT,
        AdjustmentKind::Grain => icons::DOTS_NINE,
        AdjustmentKind::AddNoise => icons::DOTS_SIX,
        AdjustmentKind::GaussianBlur => icons::DROP,
        AdjustmentKind::MotionBlur => icons::WIND,
        AdjustmentKind::Invert => icons::CIRCLE_HALF_TILT,
        AdjustmentKind::BlackWhite => icons::SUN_DIM,
        AdjustmentKind::ColorBalance => icons::SCALES,
    }
}

/// One adjustment layer's settings. Like the macOS app's record it carries every kind's settings:
/// Hue/Saturation's legacy fields, Levels and Curves always, the rest only once set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Adjustment {
    pub kind: AdjustmentKind,
    #[serde(default)]
    pub hue: f64,
    #[serde(default)]
    pub saturation: f64,
    #[serde(default)]
    pub lightness: f64,
    #[serde(default)]
    pub colorize: bool,
    /// Range-aware Hue/Saturation; older projects only have the four fields above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hsv_settings: Option<HueSaturationSettings>,
    #[serde(default)]
    pub levels: LevelsSettings,
    #[serde(default)]
    pub curves: CurvesSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure_settings: Option<ExposureSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gradient_map_settings: Option<GradientMapSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grain_settings: Option<GrainSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub black_white_settings: Option<BlackWhiteSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_balance_settings: Option<ColorBalanceSettings>,
    /// Gaussian Blur's standard deviation in document pixels, 0.1–250 (10 when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur_radius: Option<f64>,
    /// Motion Blur, −90…90 degrees counterclockwise (0 when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion_angle: Option<f64>,
    /// Motion Blur streak length in document pixels, 1–2000 (10 when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion_distance: Option<f64>,
    /// Add Noise percentage, 0.1–400 (10 when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_amount: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_gaussian: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_monochromatic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_seed: Option<u32>,
    /// Fields this build doesn't understand, written back unchanged.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Adjustment {
    pub fn new(kind: AdjustmentKind) -> Self {
        let mut adjustment = Adjustment {
            kind,
            hue: 0.0,
            saturation: 0.0,
            lightness: 0.0,
            colorize: false,
            hsv_settings: None,
            levels: LevelsSettings::default(),
            curves: CurvesSettings::default(),
            exposure_settings: None,
            gradient_map_settings: None,
            grain_settings: None,
            black_white_settings: None,
            color_balance_settings: None,
            blur_radius: None,
            motion_angle: None,
            motion_distance: None,
            noise_amount: None,
            noise_gaussian: None,
            noise_monochromatic: None,
            noise_seed: None,
            extra: Map::new(),
        };
        // Each Grain and Add Noise layer gets a pattern of its own.
        match kind {
            AdjustmentKind::Grain => adjustment.grain_settings = Some(GrainSettings { seed: rand::random(), ..Default::default() }),
            AdjustmentKind::AddNoise => adjustment.noise_seed = Some(rand::random()),
            _ => {}
        }
        adjustment
    }

    /// Hue/Saturation settings, from the legacy fields when the project predates color ranges.
    pub fn resolved_hsv(&self) -> HueSaturationSettings {
        self.hsv_settings.clone().unwrap_or_else(|| HueSaturationSettings::new(self.hue, self.saturation, self.lightness, self.colorize))
    }

    pub fn exposure(&self) -> ExposureSettings {
        self.exposure_settings.unwrap_or_default()
    }

    pub fn gradient_map(&self) -> GradientMapSettings {
        self.gradient_map_settings.unwrap_or_default()
    }

    pub fn grain(&self) -> GrainSettings {
        self.grain_settings.unwrap_or_default()
    }

    pub fn black_white(&self) -> BlackWhiteSettings {
        self.black_white_settings.unwrap_or_default()
    }

    pub fn color_balance(&self) -> ColorBalanceSettings {
        self.color_balance_settings.unwrap_or_default()
    }

    pub fn gaussian_radius(&self) -> f64 {
        self.blur_radius.unwrap_or(10.0)
    }

    pub fn resolved_motion_angle(&self) -> f64 {
        self.motion_angle.unwrap_or(0.0)
    }

    pub fn resolved_motion_distance(&self) -> f64 {
        self.motion_distance.unwrap_or(10.0)
    }

    pub fn resolved_noise_amount(&self) -> f64 {
        self.noise_amount.unwrap_or(10.0)
    }

    /// Whether these settings leave every pixel as it is.
    pub fn is_identity(&self) -> bool {
        match self.kind {
            AdjustmentKind::HueSaturation => self.resolved_hsv().is_identity(),
            AdjustmentKind::Levels => self.levels.is_identity(),
            AdjustmentKind::Curves => self.curves.is_identity(),
            AdjustmentKind::Exposure => self.exposure().normalized() == ExposureSettings::default(),
            AdjustmentKind::ColorBalance => self.color_balance().is_identity(),
            AdjustmentKind::Grain => self.grain().amount <= 0.0,
            _ => false,
        }
    }

    /// Whether the adjustment changes coverage as well as color (a blur spreads past edges).
    pub fn changes_alpha(&self) -> bool {
        matches!(self.kind, AdjustmentKind::GaussianBlur | AdjustmentKind::MotionBlur)
    }
}

/// Document pixels around each pixel the adjustment reads.
pub fn sampling_margin(adjustment: &Adjustment) -> f64 {
    match adjustment.kind {
        AdjustmentKind::GaussianBlur => adjustment.gaussian_radius().clamp(0.1, 250.0) * 3.0 + 2.0,
        AdjustmentKind::MotionBlur => adjustment.resolved_motion_distance().clamp(1.0, 2000.0) / 2.0 + 2.0,
        _ => 0.0,
    }
}

/// A per-color function, for adjustments that change each color on its own.
pub type ColorFn = Box<dyn Fn([f32; 3]) -> [f32; 3] + Send + Sync>;

/// The adjustment as a function of color alone, or `None` for the kinds that depend on position
/// or neighborhood (Grain, Add Noise, the blurs) or that change nothing.
pub fn color_fn(adjustment: &Adjustment) -> Option<ColorFn> {
    match adjustment.kind {
        AdjustmentKind::HueSaturation => {
            let settings = adjustment.resolved_hsv();
            if settings.is_identity() || !settings.is_valid() {
                return None;
            }
            let cube = settings.cube();
            Some(Box::new(move |c| cube.lookup(c)))
        }
        AdjustmentKind::Levels => {
            if adjustment.levels.is_identity() {
                return None;
            }
            let lut = adjustment.levels.tables();
            Some(Box::new(move |c| lut.apply(c)))
        }
        AdjustmentKind::Curves => {
            if adjustment.curves.is_identity() || !adjustment.curves.is_valid() {
                return None;
            }
            let lut = adjustment.curves.tables();
            Some(Box::new(move |c| lut.apply(c)))
        }
        AdjustmentKind::Exposure => {
            let lut = adjustment.exposure().tables();
            Some(Box::new(move |c| lut.apply(c)))
        }
        AdjustmentKind::GradientMap => {
            let settings = adjustment.gradient_map();
            Some(Box::new(move |c| settings.apply(c)))
        }
        AdjustmentKind::BlackWhite => {
            let settings = adjustment.black_white();
            Some(Box::new(move |c| settings.apply(c)))
        }
        AdjustmentKind::ColorBalance => {
            let settings = adjustment.color_balance();
            if settings.is_identity() {
                return None;
            }
            Some(Box::new(move |c| settings.apply(c)))
        }
        AdjustmentKind::Invert => Some(Box::new(|c| [1.0 - c[0], 1.0 - c[1], 1.0 - c[2]])),
        _ => None,
    }
}

/// Adjusts straight-alpha pixels in place. `region` places the buffer on the canvas (its scale
/// is document pixels per buffer pixel), so patterns stay fixed in the document and blurs reach
/// the same distance at any zoom.
pub fn apply(adjustment: &Adjustment, buf: &mut Buffer, region: Region) {
    let width = buf.width.max(1);
    if let Some(f) = color_fn(adjustment) {
        buf.px.par_iter_mut().for_each(|p| {
            let c = f([p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0), p[2].clamp(0.0, 1.0)]);
            p[0] = c[0];
            p[1] = c[1];
            p[2] = c[2];
        });
        return;
    }
    let scale = if region.scale > 0.0 { region.scale } else { 1.0 };
    match adjustment.kind {
        AdjustmentKind::Grain => {
            let grain = adjustment.grain();
            buf.px.par_chunks_mut(width).enumerate().for_each(|(j, row)| {
                let v = (region.y as f64 + j as f64 + 0.5) * scale;
                grain.apply_row(row, region.x as f64 * scale, v, scale);
            });
        }
        AdjustmentKind::AddNoise => {
            let amount = adjustment.resolved_noise_amount().clamp(0.1, 400.0) as f32;
            let gaussian = adjustment.noise_gaussian.unwrap_or(false);
            let mono = adjustment.noise_monochromatic.unwrap_or(false);
            let seed = adjustment.noise_seed.unwrap_or(0);
            buf.px.par_chunks_mut(width).enumerate().for_each(|(j, row)| {
                spatial::add_noise_row(row, amount, gaussian, mono, seed, region.x, region.y + j as i64);
            });
        }
        AdjustmentKind::GaussianBlur => {
            spatial::premultiply(&mut buf.px);
            spatial::gaussian_blur(&mut buf.px, buf.width, buf.height, adjustment.gaussian_radius().clamp(0.1, 250.0) / scale);
            spatial::unpremultiply(&mut buf.px);
        }
        AdjustmentKind::MotionBlur => {
            spatial::premultiply(&mut buf.px);
            spatial::motion_blur(
                &mut buf.px,
                buf.width,
                buf.height,
                adjustment.resolved_motion_angle().clamp(-90.0, 90.0),
                adjustment.resolved_motion_distance().clamp(1.0, 2000.0) / scale,
            );
            spatial::unpremultiply(&mut buf.px);
        }
        _ => {}
    }
}

/// Adds an adjustment layer of `kind` above the active layer.
pub fn add_adjustment_layer(project: &mut crate::project::Project, kind: AdjustmentKind) {
    let transform = project.doc.full_canvas_transform();
    let mut layer = crate::doc::Layer::blank(kind_name(kind), transform);
    layer.adjustment = Some(Adjustment::new(kind));
    project.edit(&format!("New {} Layer", kind_name(kind)), |doc| doc.insert_above_active(layer));
    project.target = crate::project::EditTarget::Image;
}

/// Image > Adjustments > Invert (Ctrl+I): inverts the active layer's pixels, or its mask.
pub fn invert_active(project: &mut crate::project::Project) {
    use std::sync::Arc;
    use crate::doc::MaskPixels;
    let target = project.target;
    project.edit("Invert", |doc| {
        let selection = doc.selection.clone();
        let Some(layer) = doc.active_layer_mut() else { return };
        if target == crate::project::EditTarget::Mask {
            if let Some(mask) = layer.mask.as_mut() {
                mask.pixels = match &mask.pixels {
                    MaskPixels::Uniform(v) => MaskPixels::Uniform(255 - v),
                    MaskPixels::Pixels(p) => {
                        let mut p = (**p).clone();
                        p.pixels_mut().for_each(|v| v[0] = 255 - v[0]);
                        MaskPixels::Pixels(Arc::new(p))
                    }
                };
            }
            return;
        }
        let transform = layer.transform;
        let Some(image) = layer.image.as_mut() else { return };
        let to_doc = transform.pixel_to_document(image.width(), image.height());
        let image = Arc::make_mut(image);
        for (x, y, p) in image.enumerate_pixels_mut() {
            let c = match &selection {
                Some(s) => {
                    let (dx, dy) = to_doc.apply(x as f64 + 0.5, y as f64 + 0.5);
                    s.coverage(dx.floor() as i64, dy.floor() as i64) as u32
                }
                None => 255,
            };
            for i in 0..3 {
                let inv = 255 - p[i] as u32;
                p[i] = ((p[i] as u32 * (255 - c) + inv * c + 127) / 255) as u8;
            }
        }
        layer.rasterized();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::curves::CurvePoint;
    use super::hsv::ColorRange;
    use super::levels::{LevelRange, LevelsChannel};

    fn sample_buffer() -> Buffer {
        let mut buf = Buffer::new(16, 16);
        for (i, p) in buf.px.iter_mut().enumerate() {
            let x = (i % 16) as f32 / 15.0;
            let y = (i / 16) as f32 / 15.0;
            *p = [x, y, (x * 0.7 + y * 0.3), 1.0];
        }
        buf
    }

    fn region() -> Region {
        Region { x: 0, y: 0, width: 16, height: 16, scale: 1.0 }
    }

    fn max_difference(a: &Buffer, b: &Buffer) -> f32 {
        a.px.iter().zip(&b.px).flat_map(|(p, q)| (0..4).map(move |c| (p[c] - q[c]).abs())).fold(0.0, f32::max)
    }

    #[test]
    fn identity_settings_leave_pixels_unchanged() {
        for kind in [
            AdjustmentKind::HueSaturation,
            AdjustmentKind::Levels,
            AdjustmentKind::Curves,
            AdjustmentKind::Exposure,
            AdjustmentKind::ColorBalance,
        ] {
            let before = sample_buffer();
            let mut after = before.clone();
            let adjustment = Adjustment::new(kind);
            assert!(adjustment.is_identity(), "{kind:?}");
            apply(&adjustment, &mut after, region());
            assert!(max_difference(&before, &after) < 1.0 / 255.0, "{kind:?} changed pixels");
        }
        // Exposure's table and Hue/Saturation's cube are exact at their defaults even when forced.
        let before = sample_buffer();
        let cube = HueSaturationSettings::new(0.0, 0.0, 0.0, false).cube();
        for p in &before.px {
            let c = cube.lookup([p[0], p[1], p[2]]);
            assert!((c[0] - p[0]).abs() < 1e-3 && (c[1] - p[1]).abs() < 1e-3 && (c[2] - p[2]).abs() < 1e-3);
        }
        let lut = ExposureSettings::default().tables();
        for i in 0..=255 {
            let v = i as f32 / 255.0;
            assert!((lut.lookup(0, v) - v).abs() < 1e-4);
        }
    }

    #[test]
    fn invert_twice_is_identity_and_blurs_keep_flat_color() {
        let before = sample_buffer();
        let mut buf = before.clone();
        let invert = Adjustment::new(AdjustmentKind::Invert);
        apply(&invert, &mut buf, region());
        assert!((buf.px[0][0] - 1.0).abs() < 1e-6);
        apply(&invert, &mut buf, region());
        assert!(max_difference(&before, &buf) < 1e-6);

        let mut flat = Buffer::new(32, 32);
        flat.px.iter_mut().for_each(|p| *p = [0.2, 0.4, 0.6, 1.0]);
        let mut blur = Adjustment::new(AdjustmentKind::GaussianBlur);
        blur.blur_radius = Some(2.0);
        apply(&blur, &mut flat, Region { x: 0, y: 0, width: 32, height: 32, scale: 1.0 });
        let middle = flat.px[16 * 32 + 16];
        assert!((middle[0] - 0.2).abs() < 1e-4 && (middle[2] - 0.6).abs() < 1e-4 && (middle[3] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn adjustment_layers_composite() {
        use crate::doc::{Document, Layer, LayerTransform};
        use crate::render::{composite, RenderCache};
        let cache = RenderCache::default();
        // Levels over a gray canvas.
        let mut doc = Document::new(8, 8, Some([100, 100, 100, 255]));
        let mut layer = Layer::blank("Levels", doc.full_canvas_transform());
        let mut levels = Adjustment::new(AdjustmentKind::Levels);
        levels.levels.ranges[0].white = 200.0;
        layer.adjustment = Some(levels);
        doc.layers.push(layer);
        let out = composite(&doc, Region::full(&doc), &cache).to_rgba8();
        assert_eq!(out.get_pixel(3, 3)[0], 128);
        // A blur layer spreads a small square's coverage past its edges.
        let mut doc = Document::new(40, 40, None);
        let square = image::RgbaImage::from_pixel(10, 10, image::Rgba([255, 0, 0, 255]));
        doc.layers.push(Layer::new_pixel("Square", square, LayerTransform::rect(15.0, 15.0, 10.0, 10.0)));
        let mut blur = Layer::blank("Blur", doc.full_canvas_transform());
        let mut settings = Adjustment::new(AdjustmentKind::GaussianBlur);
        settings.blur_radius = Some(3.0);
        blur.adjustment = Some(settings);
        doc.layers.push(blur);
        let out = composite(&doc, Region::full(&doc), &cache).to_rgba8();
        assert!(out.get_pixel(13, 20)[3] > 0 && out.get_pixel(13, 20)[3] < 255);
        assert!(out.get_pixel(20, 20)[0] == 255 && out.get_pixel(20, 20)[3] > 200);
        // A partial redraw matches the full one, thanks to the sampling margin.
        let tile = composite(&doc, Region { x: 10, y: 10, width: 8, height: 8, scale: 1.0 }, &cache).to_rgba8();
        assert_eq!(tile.get_pixel(3, 10 - 10 + 5), out.get_pixel(13, 15));
    }

    #[test]
    fn levels_and_curves_math() {
        let range = LevelRange { black: 50.0, white: 200.0, ..Default::default() };
        assert!((range.apply(50.0 / 255.0)).abs() < 1e-9);
        assert!((range.apply(200.0 / 255.0) - 1.0).abs() < 1e-9);
        let gamma = LevelRange { gamma: 2.0, ..Default::default() };
        assert!((gamma.apply(0.25) - 0.5).abs() < 1e-9);
        let mut curves = CurvesSettings::default();
        curves.channels[0] = vec![CurvePoint { x: 0.0, y: 0.0 }, CurvePoint { x: 128.0, y: 160.0 }, CurvePoint { x: 255.0, y: 255.0 }];
        assert!((curves.value(128.0, 0) - 160.0).abs() < 1e-9);
        // Monotone: no overshoot between handles.
        let mut last = -1.0;
        for x in 0..=255 {
            let y = curves.value(x as f64, 0);
            assert!(y >= last && (0.0..=255.0).contains(&y));
            last = y;
        }
    }

    #[test]
    fn hue_saturation_matches_photoshop_behavior() {
        let settings = HueSaturationSettings::new(0.0, -100.0, 0.0, false);
        let cube = settings.cube();
        let gray = cube.lookup([1.0, 0.0, 0.0]);
        assert!((gray[0] - gray[1]).abs() < 1e-3 && (gray[0] - 0.5).abs() < 1e-3);
        // A hue shift of 120° turns red into green.
        let shifted = HueSaturationSettings::new(120.0, 0.0, 0.0, false).cube().lookup([1.0, 0.0, 0.0]);
        assert!(shifted[1] > 0.99 && shifted[0] < 0.01);
        // Reds affects red but not blue.
        let mut reds = HueSaturationSettings::default();
        reds.range = ColorRange::Reds;
        reds.current_mut().saturation = -100.0;
        let response_cube = reds.cube();
        let blue = response_cube.lookup([0.0, 0.0, 1.0]);
        assert!(blue[2] > 0.99 && blue[0] < 0.01);
    }

    /// The adjustment from `docs/writing-comp-files.md`.
    const MANIFEST_CURVES: &str = r#"{
        "kind": "Curves",
        "hue": 0, "saturation": 0, "lightness": 0, "colorize": false,
        "levels": { "channel": "RGB", "ranges": [
          { "black": 0, "gamma": 1, "white": 255, "outputBlack": 0, "outputWhite": 255 },
          { "black": 0, "gamma": 1, "white": 255, "outputBlack": 0, "outputWhite": 255 },
          { "black": 0, "gamma": 1, "white": 255, "outputBlack": 0, "outputWhite": 255 },
          { "black": 0, "gamma": 1, "white": 255, "outputBlack": 0, "outputWhite": 255 } ] },
        "curves": { "channel": "RGB", "channels": [
          [ { "x": 0, "y": 0 }, { "x": 255, "y": 255 } ],
          [ { "x": 0, "y": 0 }, { "x": 120, "y": 147 }, { "x": 255, "y": 255 } ],
          [ { "x": 0, "y": 0 }, { "x": 100, "y": 114 }, { "x": 255, "y": 255 } ],
          [ { "x": 0, "y": 0 }, { "x": 115, "y": 97 }, { "x": 255, "y": 238 } ] ] }
    }"#;

    fn keys(value: &Value) -> Vec<String> {
        let mut out = Vec::new();
        fn walk(value: &Value, path: String, out: &mut Vec<String>) {
            match value {
                Value::Object(map) => {
                    for (k, v) in map {
                        let p = format!("{path}.{k}");
                        out.push(p.clone());
                        walk(v, p, out);
                    }
                }
                Value::Array(items) => {
                    for (i, v) in items.iter().enumerate() {
                        walk(v, format!("{path}[{i}]"), out);
                    }
                }
                _ => {}
            }
        }
        walk(value, String::new(), &mut out);
        out.sort();
        out
    }

    #[test]
    fn manifest_adjustment_round_trips_with_the_same_keys() {
        let original: Value = serde_json::from_str(MANIFEST_CURVES).unwrap();
        let adjustment: Adjustment = serde_json::from_value(original.clone()).unwrap();
        assert_eq!(adjustment.kind, AdjustmentKind::Curves);
        assert_eq!(adjustment.curves.channels[1][1], CurvePoint { x: 120.0, y: 147.0 });
        let written = serde_json::to_value(&adjustment).unwrap();
        assert_eq!(keys(&original), keys(&written));
        // Values survive too (numbers compare as floats).
        let again: Adjustment = serde_json::from_value(written).unwrap();
        assert_eq!(again, adjustment);
    }

    #[test]
    fn optional_settings_and_unknown_fields_survive() {
        let json = r#"{
            "kind": "Hue/Saturation", "hue": 0, "saturation": 0, "lightness": 0, "colorize": false,
            "levels": { "channel": "RGB", "ranges": [] }, "curves": { "channel": "Red", "channels": [] },
            "hsvSettings": { "range": "Reds", "colorize": false, "invertRange": true,
                "adjustments": ["Master", { "hue": 10, "saturation": 0, "lightness": 0 }, "Reds", { "hue": 0, "saturation": -50, "lightness": 5 }],
                "bands": ["Reds", { "falloffStart": 300, "rangeStart": 340, "rangeEnd": 20, "falloffEnd": 50 }] },
            "blurRadius": 4.5, "noiseSeed": 4000000000, "futureSetting": { "a": [1, 2] }
        }"#;
        let adjustment: Adjustment = serde_json::from_str(json).unwrap();
        let hsv = adjustment.hsv_settings.as_ref().unwrap();
        assert_eq!(hsv.range, ColorRange::Reds);
        assert!(hsv.invert_range);
        assert_eq!(hsv.adjustments[&ColorRange::Reds].saturation, -50.0);
        assert_eq!(hsv.band(ColorRange::Reds).falloff_start, 300.0);
        assert_eq!(adjustment.noise_seed, Some(4_000_000_000));
        assert_eq!(adjustment.curves.channel, LevelsChannel::Red);
        let written = serde_json::to_value(&adjustment).unwrap();
        assert_eq!(written["futureSetting"]["a"][1], 2);
        assert_eq!(written["blurRadius"], 4.5);
        assert!(written.get("exposureSettings").is_none());
        // Swift's encoding of a dictionary keyed by an enum: alternating keys and values.
        let adjustments = written["hsvSettings"]["adjustments"].as_array().unwrap();
        assert_eq!(adjustments[0], "Master");
        assert_eq!(adjustments[3]["saturation"], -50.0);
        let back: Adjustment = serde_json::from_value(written).unwrap();
        assert_eq!(back, adjustment);
    }

    #[test]
    fn new_kinds_write_every_setting_swift_requires() {
        let mut adjustment = Adjustment::new(AdjustmentKind::BlackWhite);
        adjustment.black_white_settings = Some(BlackWhiteSettings::default());
        let written = serde_json::to_value(&adjustment).unwrap();
        for key in ["kind", "hue", "saturation", "lightness", "colorize", "levels", "curves"] {
            assert!(written.get(key).is_some(), "{key}");
        }
        let bw = &written["blackWhiteSettings"];
        for key in ["reds", "yellows", "greens", "cyans", "blues", "magentas", "tint", "tintHue", "tintSaturation"] {
            assert!(bw.get(key).is_some(), "{key}");
        }
        let cb = serde_json::to_value(ColorBalanceSettings::default()).unwrap();
        assert!(cb.get("preserveLuminosity").is_some() && cb.get("highlightYellowBlue").is_some());
        let hsv = serde_json::to_value(HueSaturationSettings::default()).unwrap();
        assert_eq!(hsv["bands"].as_array().unwrap().len(), 14);
    }
}
