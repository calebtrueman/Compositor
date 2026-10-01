//! Layer effects. Placeholder: effect records round-trip unchanged and don't render yet.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::render::{Buffer, Region};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerEffects(pub Map<String, Value>);

/// Document pixels the effects reach past the layer's own pixels.
pub fn margin(_effects: &LayerEffects) -> f64 {
    0.0
}

/// The layer's placed pixels with its effects drawn, premultiplied.
pub fn apply(_effects: &LayerEffects, content: Buffer, _region: Region) -> Buffer {
    content
}

pub fn is_empty(effects: &LayerEffects) -> bool {
    effects.0.is_empty()
}

pub fn properties_ui(_ui: &mut egui::Ui, _project: &mut crate::project::Project) {}

/// Layer > Layer Style items.
pub fn menu(ui: &mut egui::Ui, _project: &mut crate::project::Project) {
    ui.label("Layer styles are coming soon");
}
