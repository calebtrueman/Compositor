//! Adjustment layers. Placeholder: kinds and raw settings are kept so projects round-trip; only
//! Invert renders until the full set lands.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Adjustment {
    pub kind: AdjustmentKind,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl Adjustment {
    pub fn new(kind: AdjustmentKind) -> Self {
        Adjustment { kind, rest: Map::new() }
    }
}

/// Document pixels around each pixel the adjustment reads.
pub fn sampling_margin(_adjustment: &Adjustment) -> f64 {
    0.0
}

/// Adjusts straight-alpha pixels in place.
pub fn apply(adjustment: &Adjustment, buf: &mut Buffer, _region: Region) {
    if adjustment.kind == AdjustmentKind::Invert {
        for p in &mut buf.px {
            p[0] = 1.0 - p[0];
            p[1] = 1.0 - p[1];
            p[2] = 1.0 - p[2];
        }
    }
}

/// The settings editor for the active adjustment layer, in the Properties panel. Returns true
/// when the settings changed.
pub fn properties_ui(_ui: &mut egui::Ui, _adjustment: &mut Adjustment) -> bool {
    false
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
