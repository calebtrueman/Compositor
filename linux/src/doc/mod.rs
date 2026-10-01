//! The in-memory document: a canvas and a bottom-to-top list of layers, mirroring the `.comp`
//! manifest closely so projects round-trip with the macOS app.

pub mod history;
pub mod ids;
pub mod ops;
pub mod selection;
pub mod transform;

use std::sync::Arc;

use image::{GrayImage, RgbaImage};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use ids::Id;
pub use selection::Selection;
pub use transform::{Affine, LayerTransform, Sampling};

use crate::adjust::Adjustment;

/// Blend modes, spelled exactly as the manifest stores them, in Photoshop's menu order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Darken,
    Multiply,
    #[serde(rename = "Color Burn")]
    ColorBurn,
    #[serde(rename = "Linear Burn")]
    LinearBurn,
    Lighten,
    Screen,
    #[serde(rename = "Color Dodge")]
    ColorDodge,
    #[serde(rename = "Linear Dodge (Add)")]
    LinearDodge,
    Overlay,
    #[serde(rename = "Soft Light")]
    SoftLight,
    #[serde(rename = "Hard Light")]
    HardLight,
    #[serde(rename = "Vivid Light")]
    VividLight,
    #[serde(rename = "Linear Light")]
    LinearLight,
    #[serde(rename = "Pin Light")]
    PinLight,
    #[serde(rename = "Hard Mix")]
    HardMix,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    /// Photoshop's grouping; menus draw a separator between groups.
    pub const GROUPS: &'static [&'static [BlendMode]] = &[
        &[BlendMode::Normal],
        &[BlendMode::Darken, BlendMode::Multiply, BlendMode::ColorBurn, BlendMode::LinearBurn],
        &[BlendMode::Lighten, BlendMode::Screen, BlendMode::ColorDodge, BlendMode::LinearDodge],
        &[
            BlendMode::Overlay,
            BlendMode::SoftLight,
            BlendMode::HardLight,
            BlendMode::VividLight,
            BlendMode::LinearLight,
            BlendMode::PinLight,
            BlendMode::HardMix,
        ],
        &[BlendMode::Difference, BlendMode::Exclusion, BlendMode::Subtract, BlendMode::Divide],
        &[BlendMode::Hue, BlendMode::Saturation, BlendMode::Color, BlendMode::Luminosity],
    ];

    pub fn name(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Darken => "Darken",
            BlendMode::Multiply => "Multiply",
            BlendMode::ColorBurn => "Color Burn",
            BlendMode::LinearBurn => "Linear Burn",
            BlendMode::Lighten => "Lighten",
            BlendMode::Screen => "Screen",
            BlendMode::ColorDodge => "Color Dodge",
            BlendMode::LinearDodge => "Linear Dodge (Add)",
            BlendMode::Overlay => "Overlay",
            BlendMode::SoftLight => "Soft Light",
            BlendMode::HardLight => "Hard Light",
            BlendMode::VividLight => "Vivid Light",
            BlendMode::LinearLight => "Linear Light",
            BlendMode::PinLight => "Pin Light",
            BlendMode::HardMix => "Hard Mix",
            BlendMode::Difference => "Difference",
            BlendMode::Exclusion => "Exclusion",
            BlendMode::Subtract => "Subtract",
            BlendMode::Divide => "Divide",
            BlendMode::Hue => "Hue",
            BlendMode::Saturation => "Saturation",
            BlendMode::Color => "Color",
            BlendMode::Luminosity => "Luminosity",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GuideAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub id: Id,
    pub axis: GuideAxis,
    /// Document pixels: Y for a horizontal guide, X for a vertical one.
    pub position: f64,
}

/// Opaque JSON kept as written so fields this build doesn't edit survive a save.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RawRecord(pub Map<String, Value>);

/// What a layer's raster mask covers when it isn't uniform.
#[derive(Clone, Debug)]
pub enum MaskPixels {
    /// A 1×1 mask: one coverage everywhere, with no pixels allocated.
    Uniform(u8),
    Pixels(Arc<GrayImage>),
}

impl MaskPixels {
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            MaskPixels::Uniform(_) => (1, 1),
            MaskPixels::Pixels(image) => image.dimensions(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LayerMask {
    pub pixels: MaskPixels,
    pub enabled: bool,
    /// Unlinked masks keep their own placement and stop following the layer.
    pub linked: bool,
    pub placement: Option<LayerTransform>,
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub id: Id,
    pub name: String,
    pub visible: bool,
    pub transform: LayerTransform,
    /// Straight-alpha sRGB pixels. `None` for groups, adjustment layers and blank layers.
    pub image: Option<Arc<RgbaImage>>,
    pub parent: Option<Id>,
    pub is_group: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    pub mask: Option<LayerMask>,
    /// Clipping mask: the layer whose coverage this one is clipped to (`maskSourceID`).
    pub clip_source: Option<Id>,
    pub adjustment: Option<Adjustment>,
    pub effects: Option<crate::effects::LayerEffects>,
    pub text: Option<crate::text::TextStyle>,
    pub shape: Option<RawRecord>,
    /// Session-only: whether the folder is expanded in the Layers panel.
    pub expanded: bool,
    /// Manifest fields this build doesn't understand, written back unchanged.
    pub extra: Map<String, Value>,
}

impl Layer {
    pub fn new_pixel(name: impl Into<String>, image: RgbaImage, transform: LayerTransform) -> Self {
        Layer { image: Some(Arc::new(image)), ..Layer::blank(name, transform) }
    }

    pub fn blank(name: impl Into<String>, transform: LayerTransform) -> Self {
        Layer {
            id: Id::new(),
            name: name.into(),
            visible: true,
            transform,
            image: None,
            parent: None,
            is_group: false,
            opacity: 1.0,
            blend: BlendMode::Normal,
            mask: None,
            clip_source: None,
            adjustment: None,
            effects: None,
            text: None,
            shape: None,
            expanded: true,
            extra: Map::new(),
        }
    }

    pub fn new_group(name: impl Into<String>, transform: LayerTransform) -> Self {
        Layer { is_group: true, ..Layer::blank(name, transform) }
    }

    pub fn is_adjustment(&self) -> bool {
        self.adjustment.is_some()
    }

    pub fn is_text(&self) -> bool {
        self.text.is_some()
    }

    /// Pixel size of the layer's source image (the canvas size for blank and adjustment layers).
    pub fn pixel_size(&self) -> (u32, u32) {
        match &self.image {
            Some(image) => image.dimensions(),
            None => (
                self.transform.size[0].round().max(1.0) as u32,
                self.transform.size[1].round().max(1.0) as u32,
            ),
        }
    }

    /// Pixels changed destructively: the vector/text metadata no longer describes them.
    pub fn rasterized(&mut self) {
        self.text = None;
        self.shape = None;
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    pub id: Id,
    pub width: u32,
    pub height: u32,
    /// Pixels per inch, 72 when the manifest omits it.
    pub resolution: f64,
    /// Bottom to top; groups are followed by nothing in particular — children name their parent.
    pub layers: Vec<Layer>,
    pub active: Option<Id>,
    /// Extra layers selected alongside `active` in the Layers panel.
    pub selected: Vec<Id>,
    pub guides: Vec<Guide>,
    pub selection: Option<Selection>,
    /// Top-level manifest fields this build doesn't understand.
    pub extra: Map<String, Value>,
}

impl Document {
    pub fn new(width: u32, height: u32, background: Option<[u8; 4]>) -> Self {
        let mut doc = Document {
            id: Id::new(),
            width,
            height,
            resolution: 72.0,
            layers: Vec::new(),
            active: None,
            selected: Vec::new(),
            guides: Vec::new(),
            selection: None,
            extra: Map::new(),
        };
        let transform = doc.full_canvas_transform();
        let layer = match background {
            Some(color) => {
                Layer::new_pixel("Background", RgbaImage::from_pixel(width, height, image::Rgba(color)), transform)
            }
            None => Layer::blank("Layer 1", transform),
        };
        doc.active = Some(layer.id);
        doc.layers.push(layer);
        doc
    }

    pub fn from_image(image: RgbaImage, name: &str) -> Self {
        let (width, height) = image.dimensions();
        let mut doc = Document::new(width, height, None);
        doc.layers.clear();
        let layer = Layer::new_pixel(name, image, doc.full_canvas_transform());
        doc.active = Some(layer.id);
        doc.layers.push(layer);
        doc
    }

    pub fn full_canvas_transform(&self) -> LayerTransform {
        LayerTransform::rect(0.0, 0.0, self.width as f64, self.height as f64)
    }

    pub fn index_of(&self, id: Id) -> Option<usize> {
        self.layers.iter().position(|layer| layer.id == id)
    }

    pub fn layer(&self, id: Id) -> Option<&Layer> {
        self.layers.iter().find(|layer| layer.id == id)
    }

    pub fn layer_mut(&mut self, id: Id) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|layer| layer.id == id)
    }

    pub fn active_layer(&self) -> Option<&Layer> {
        self.active.and_then(|id| self.layer(id))
    }

    pub fn active_layer_mut(&mut self) -> Option<&mut Layer> {
        let id = self.active?;
        self.layer_mut(id)
    }

    pub fn active_index(&self) -> Option<usize> {
        self.active.and_then(|id| self.index_of(id))
    }

    /// The active layer plus any extra selection, without duplicates, bottom to top.
    pub fn selected_ids(&self) -> Vec<Id> {
        let mut ids: Vec<Id> = self.selected.clone();
        if let Some(active) = self.active {
            if !ids.contains(&active) {
                ids.push(active);
            }
        }
        ids.retain(|id| self.index_of(*id).is_some());
        ids.sort_by_key(|id| self.index_of(*id));
        ids
    }

    pub fn children(&self, parent: Option<Id>) -> Vec<Id> {
        self.layers.iter().filter(|layer| layer.parent == parent).map(|layer| layer.id).collect()
    }

    /// The layer and all its descendants.
    pub fn subtree(&self, id: Id) -> Vec<Id> {
        let mut result = vec![id];
        let mut index = 0;
        while index < result.len() {
            let current = result[index];
            for layer in &self.layers {
                if layer.parent == Some(current) {
                    result.push(layer.id);
                }
            }
            index += 1;
        }
        result
    }

    pub fn ancestors(&self, id: Id) -> Vec<Id> {
        let mut result = Vec::new();
        let mut current = self.layer(id).and_then(|layer| layer.parent);
        while let Some(parent) = current {
            if result.contains(&parent) || result.len() > 64 {
                break;
            }
            result.push(parent);
            current = self.layer(parent).and_then(|layer| layer.parent);
        }
        result
    }

    pub fn is_effectively_visible(&self, id: Id) -> bool {
        let Some(layer) = self.layer(id) else { return false };
        layer.visible && self.ancestors(id).iter().all(|a| self.layer(*a).is_some_and(|l| l.visible))
    }

    pub fn depth(&self, id: Id) -> usize {
        self.ancestors(id).len()
    }

    /// Reorders `layers` so every group's subtree is contiguous and sits directly below the group
    /// record, which is how the Layers panel and the macOS app lay them out.
    pub fn normalize_order(&mut self) {
        fn visit(doc: &Document, parent: Option<Id>, out: &mut Vec<Id>) {
            for layer in doc.layers.iter().filter(|l| l.parent == parent) {
                if layer.is_group {
                    visit(doc, Some(layer.id), out);
                }
                out.push(layer.id);
            }
        }
        let mut order = Vec::with_capacity(self.layers.len());
        visit(self, None, &mut order);
        if order.len() != self.layers.len() {
            return;
        }
        let mut layers = std::mem::take(&mut self.layers);
        let mut sorted = Vec::with_capacity(layers.len());
        for id in order {
            let index = layers.iter().position(|l| l.id == id).unwrap();
            sorted.push(layers.swap_remove(index));
        }
        self.layers = sorted;
    }

    /// Inserts a layer above the active one (in its folder), and makes it active.
    pub fn insert_above_active(&mut self, mut layer: Layer) -> Id {
        let id = layer.id;
        match self.active_index() {
            Some(index) => {
                let active = &self.layers[index];
                layer.parent = if active.is_group && active.expanded && !layer.is_group {
                    Some(active.id)
                } else {
                    active.parent
                };
                let position = if layer.parent == Some(active.id) { index } else { index + 1 };
                self.layers.insert(position, layer);
            }
            None => self.layers.push(layer),
        }
        self.active = Some(id);
        self.selected.clear();
        self.normalize_order();
        id
    }

    /// Removes layers (and their subtrees), releasing any clipping links to them.
    pub fn remove_layers(&mut self, ids: &[Id]) {
        let mut doomed: Vec<Id> = Vec::new();
        for id in ids {
            for child in self.subtree(*id) {
                if !doomed.contains(&child) {
                    doomed.push(child);
                }
            }
        }
        let below = self.active_index().and_then(|i| {
            self.layers[..i].iter().rev().find(|l| !doomed.contains(&l.id)).map(|l| l.id)
        });
        self.layers.retain(|layer| !doomed.contains(&layer.id));
        for layer in &mut self.layers {
            if layer.clip_source.is_some_and(|source| doomed.contains(&source)) {
                layer.clip_source = None;
            }
        }
        if self.active.is_some_and(|a| doomed.contains(&a)) {
            self.active = below.or_else(|| self.layers.last().map(|l| l.id));
        }
        self.selected.retain(|id| !doomed.contains(id));
    }

    pub fn unique_name(&self, base: &str) -> String {
        let mut n = 1;
        loop {
            let name = format!("{base} {n}");
            if !self.layers.iter().any(|l| l.name == name) {
                return name;
            }
            n += 1;
        }
    }
}
