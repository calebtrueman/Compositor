//! Editable text. Placeholder: metadata round-trips and the saved PNG is shown.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TextStyle(pub Map<String, Value>);

pub fn properties_ui(_ui: &mut egui::Ui, _project: &mut crate::project::Project, _colors: &mut crate::tools::Colors, _cache: &crate::render::RenderCache) {}
