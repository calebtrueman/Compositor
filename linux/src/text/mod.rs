//! Editable text. A text layer keeps its `TextStyle` (the manifest's `text` record) beside its
//! pixels, which are the rendered text; editing the style redraws them. Until then the saved PNG is
//! shown as it is, so text set in a font that isn't installed keeps its look.

pub mod fonts;
mod kerning;
pub mod layout;

use std::ops::Range;
use std::sync::Arc;

use image::RgbaImage;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::doc::Layer;
use crate::project::Project;
use crate::render::RenderCache;
use crate::tools::Colors;

/// The gap between the text and its layer's edges, in layer pixels, for point text and boxes alike.
pub const PADDING: f64 = 12.0;
/// Longest text, in UTF-16 units.
pub const MAX_LENGTH: usize = 100_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
}

impl<'de> Deserialize<'de> for Alignment {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Anything newer than this build reads as left aligned rather than failing the project.
        Ok(match String::deserialize(deserializer)?.as_str() {
            "Center" => Alignment::Center,
            "Right" => Alignment::Right,
            _ => Alignment::Left,
        })
    }
}

impl Alignment {
    pub const ALL: [Alignment; 3] = [Alignment::Left, Alignment::Center, Alignment::Right];

    pub fn name(self) -> &'static str {
        match self {
            Alignment::Left => "Left",
            Alignment::Center => "Center",
            Alignment::Right => "Right",
        }
    }

    pub fn icon(self) -> &'static str {
        use crate::ui::icons;
        match self {
            Alignment::Left => icons::TEXT_ALIGN_LEFT,
            Alignment::Center => icons::TEXT_ALIGN_CENTER,
            Alignment::Right => icons::TEXT_ALIGN_RIGHT,
        }
    }
}

/// Letters painted in a color other than the text's own, in UTF-16 units of the content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorRun {
    pub location: usize,
    pub length: usize,
    pub red: f64,
    pub green: f64,
    pub blue: f64,
}

/// Letters set in a face other than the text's own, in UTF-16 units of the content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FontRun {
    pub location: usize,
    pub length: usize,
    pub font_name: String,
}

/// A text layer's source, spelled as the macOS app's `LayerTextStyle` so projects round-trip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextStyle {
    #[serde(default)]
    pub content: String,
    /// PostScript name, such as "HelveticaNeue-Bold".
    #[serde(default = "default_font_name")]
    pub font_name: String,
    /// Pixels per em.
    #[serde(default = "default_font_size")]
    pub font_size: f64,
    #[serde(default)]
    pub red: f64,
    #[serde(default)]
    pub green: f64,
    #[serde(default)]
    pub blue: f64,
    #[serde(default)]
    pub alignment: Alignment,
    /// Extra space after every letter, in layer pixels.
    #[serde(default)]
    pub tracking: f64,
    /// Baseline to baseline in layer pixels, as Photoshop's Leading; 0 is Auto (120% of the size).
    #[serde(default)]
    pub leading: f64,
    /// Paragraph bounds in layer pixels; `None` is point text, as big as what's typed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_size: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_runs: Option<Vec<ColorRun>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_runs: Option<Vec<FontRun>>,
    /// Fields this build doesn't understand, written back unchanged.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn default_font_name() -> String {
    "Helvetica".into()
}

fn default_font_size() -> f64 {
    72.0
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            content: "Text".into(),
            font_name: default_font_name(),
            font_size: default_font_size(),
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            alignment: Alignment::Left,
            tracking: 0.0,
            leading: 0.0,
            box_size: None,
            color_runs: None,
            font_runs: None,
            extra: Map::new(),
        }
    }
}

/// UTF-16 length of `s`.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Byte offset of the letter boundary at (or just before) UTF-16 offset `unit`.
pub fn byte_index(s: &str, unit: usize) -> usize {
    let mut units = 0;
    for (byte, c) in s.char_indices() {
        if units + c.len_utf16() > unit {
            return byte;
        }
        units += c.len_utf16();
    }
    s.len()
}

/// A text layer's name: its first words on one line.
pub fn layer_name(content: &str) -> String {
    let flattened = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.is_empty() {
        "Text".into()
    } else {
        flattened.chars().take(40).collect()
    }
}

fn clamp_range(range: &Range<usize>, count: usize) -> (usize, usize) {
    let start = range.start.min(count);
    (start, range.end.clamp(start, count))
}

impl TextStyle {
    pub fn auto_leading(&self) -> f64 {
        self.font_size * 1.2
    }

    pub fn line_height(&self) -> f64 {
        if self.leading > 0.0 {
            self.leading
        } else {
            self.auto_leading()
        }
    }

    pub fn color(&self) -> [f64; 3] {
        [self.red, self.green, self.blue]
    }

    pub fn len16(&self) -> usize {
        utf16_len(&self.content)
    }

    pub fn box_is_valid(&self) -> bool {
        self.box_size.is_none_or(|[w, h]| {
            w.is_finite() && h.is_finite() && (16.0..=30_000.0).contains(&w) && (16.0..=30_000.0).contains(&h) && w * h <= 100_000_000.0
        })
    }

    /// The same checks the macOS app makes before it accepts a style.
    pub fn is_valid(&self) -> bool {
        let unit = |v: f64| v.is_finite() && (0.0..=1.0).contains(&v);
        let count = self.len16();
        let colors_ok = self.color_runs.as_ref().is_none_or(|runs| {
            let mut end = 0;
            for run in runs {
                if run.location < end || run.length == 0 || ![run.red, run.green, run.blue].into_iter().all(unit) {
                    return false;
                }
                end = run.location + run.length;
            }
            !runs.is_empty() && end <= count
        });
        let fonts_ok = self.font_runs.as_ref().is_none_or(|runs| {
            let mut end = 0;
            for run in runs {
                if run.location < end || run.length == 0 || run.font_name.is_empty() || run.font_name.chars().count() > 200 || run.font_name.contains('\n') {
                    return false;
                }
                end = run.location + run.length;
            }
            !runs.is_empty() && end <= count
        });
        count <= MAX_LENGTH
            && self.box_is_valid()
            && self.font_size.is_finite()
            && (1.0..=2000.0).contains(&self.font_size)
            && [self.red, self.green, self.blue].into_iter().all(unit)
            && self.tracking.is_finite()
            && (-100.0..=1000.0).contains(&self.tracking)
            && self.leading.is_finite()
            && (0.0..=5000.0).contains(&self.leading)
            && colors_ok
            && fonts_ok
    }

    /// The color of the UTF-16 unit at `index`.
    pub fn color_at(&self, index: usize) -> [f64; 3] {
        self.color_runs
            .iter()
            .flatten()
            .find(|r| r.location <= index && index < r.location + r.length)
            .map_or(self.color(), |r| [r.red, r.green, r.blue])
    }

    /// The face of the UTF-16 unit at `index`.
    pub fn font_name_at(&self, index: usize) -> &str {
        self.font_runs
            .iter()
            .flatten()
            .find(|r| r.location <= index && index < r.location + r.length)
            .map_or(self.font_name.as_str(), |r| r.font_name.as_str())
    }

    /// The one face covering `range`, or `None` when it's empty or uses more than one.
    pub fn uniform_font_name(&self, range: Range<usize>) -> Option<String> {
        let (start, end) = clamp_range(&range, self.len16());
        if start == end {
            return None;
        }
        let fonts = self.unit_fonts();
        let face = &fonts[start];
        fonts[start..end].iter().all(|f| f == face).then(|| face.clone())
    }

    /// Paints `range` in `color`. An empty range, or one covering the whole text, recolors all of it.
    pub fn set_color(&mut self, color: [f64; 3], range: Range<usize>) {
        let count = self.len16();
        let (start, end) = clamp_range(&range, count);
        if start == end || (start == 0 && end == count) {
            [self.red, self.green, self.blue] = color;
            self.color_runs = None;
            return;
        }
        let mut colors = self.unit_colors();
        colors[start..end].fill(color);
        self.set_unit_colors(&colors);
    }

    /// Sets the face of `range`. An empty range, or one covering the whole text, changes all of it.
    pub fn set_font(&mut self, name: &str, range: Range<usize>) {
        if name.is_empty() || name.chars().count() > 200 || name.contains('\n') {
            return;
        }
        let count = self.len16();
        let (start, end) = clamp_range(&range, count);
        if start == end || (start == 0 && end == count) {
            self.font_name = name.to_string();
            self.font_runs = None;
            return;
        }
        let mut fonts = self.unit_fonts();
        for f in &mut fonts[start..end] {
            *f = name.to_string();
        }
        self.set_unit_fonts(fonts);
    }

    /// Replaces `range` of the content with `text`. New letters take the color and face of the
    /// letter before them, as typing does.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let count = self.len16();
        let (start, end) = clamp_range(&range, count);
        let length = utf16_len(text);
        if self.color_runs.is_some() {
            let mut colors = self.unit_colors();
            let inherited = if start > 0 { colors[start - 1] } else { colors.get(start).copied().unwrap_or(self.color()) };
            colors.splice(start..end, std::iter::repeat_n(inherited, length));
            self.color_runs = None;
            let (b0, b1) = (byte_index(&self.content, start), byte_index(&self.content, end));
            self.content.replace_range(b0..b1, text);
            self.set_unit_colors(&colors);
        } else {
            let (b0, b1) = (byte_index(&self.content, start), byte_index(&self.content, end));
            self.content.replace_range(b0..b1, text);
        }
        if self.font_runs.is_some() {
            // Faces are per unit of the content as it was; rebuild them for the new content.
            let mut fonts: Vec<String> = Vec::new();
            let before = self.font_runs.take();
            let old_count = count;
            let mut old = vec![self.font_name.clone(); old_count];
            for run in before.iter().flatten() {
                for f in old.iter_mut().skip(run.location).take(run.length) {
                    *f = run.font_name.clone();
                }
            }
            let inherited = if start > 0 { old[start - 1].clone() } else { old.get(start).cloned().unwrap_or(self.font_name.clone()) };
            fonts.extend_from_slice(&old[..start]);
            fonts.extend(std::iter::repeat_n(inherited, length));
            fonts.extend_from_slice(&old[end..]);
            self.set_unit_fonts(fonts);
        }
    }

    fn unit_colors(&self) -> Vec<[f64; 3]> {
        let mut colors = vec![self.color(); self.len16()];
        for run in self.color_runs.iter().flatten() {
            for c in colors.iter_mut().skip(run.location).take(run.length) {
                *c = [run.red, run.green, run.blue];
            }
        }
        colors
    }

    fn set_unit_colors(&mut self, colors: &[[f64; 3]]) {
        let base = self.color();
        let mut runs: Vec<ColorRun> = Vec::new();
        for (index, color) in colors.iter().enumerate() {
            if *color == base {
                continue;
            }
            match runs.last_mut() {
                Some(last) if last.location + last.length == index && [last.red, last.green, last.blue] == *color => last.length += 1,
                _ => runs.push(ColorRun { location: index, length: 1, red: color[0], green: color[1], blue: color[2] }),
            }
        }
        self.color_runs = (!runs.is_empty()).then_some(runs);
    }

    fn unit_fonts(&self) -> Vec<String> {
        let mut fonts = vec![self.font_name.clone(); self.len16()];
        for run in self.font_runs.iter().flatten() {
            for f in fonts.iter_mut().skip(run.location).take(run.length) {
                *f = run.font_name.clone();
            }
        }
        fonts
    }

    fn set_unit_fonts(&mut self, fonts: Vec<String>) {
        if let Some(first) = fonts.first() {
            if fonts.iter().all(|f| f == first) {
                self.font_name = first.clone();
                self.font_runs = None;
                return;
            }
        }
        let mut runs: Vec<FontRun> = Vec::new();
        for (index, name) in fonts.into_iter().enumerate() {
            if name == self.font_name {
                continue;
            }
            match runs.last_mut() {
                Some(last) if last.location + last.length == index && last.font_name == name => last.length += 1,
                _ => runs.push(FontRun { location: index, length: 1, font_name: name }),
            }
        }
        self.font_runs = (!runs.is_empty()).then_some(runs);
    }

    /// The style new text starts from: this one's settings without its letters or box.
    pub fn as_defaults(&self) -> TextStyle {
        TextStyle { content: String::new(), box_size: None, color_runs: None, font_runs: None, ..self.clone() }
    }
}

/// Draws `style` as a text layer's pixels, or `None` when no font is installed or it's too large.
pub fn render(style: &TextStyle) -> Option<RgbaImage> {
    layout::layout(style).map(|l| l.draw())
}

/// Gives a text layer a new style and redraws its pixels, keeping its top-left corner where it
/// is along with its scale, rotation and flips. False (and nothing changes) when it can't be drawn.
pub fn restyle(layer: &mut Layer, style: TextStyle) -> bool {
    if !style.is_valid() {
        return false;
    }
    let Some(image) = render(&style) else { return false };
    let (old_w, old_h) = layer.pixel_size();
    let mut transform = layer.transform;
    let anchor = transform.point(0.0, 0.0);
    transform.size = [
        image.width() as f64 * transform.size[0] / old_w.max(1) as f64,
        image.height() as f64 * transform.size[1] / old_h.max(1) as f64,
    ];
    let moved = transform.point(0.0, 0.0);
    transform.origin[0] += anchor.0 - moved.0;
    transform.origin[1] += anchor.1 - moved.1;
    if !transform.is_valid() {
        return false;
    }
    if image.dimensions() != (old_w, old_h) {
        // A linked mask would stretch with the new size; it stays where it was instead.
        if let Some(mask) = layer.mask.as_mut().filter(|m| m.linked) {
            mask.linked = false;
            mask.placement = Some(layer.transform);
        }
    }
    layer.image = Some(Arc::new(image));
    layer.text = Some(style);
    layer.transform = transform;
    true
}

/// A change made with the type controls.
#[derive(Clone, Debug, PartialEq)]
pub enum StyleChange {
    Font(String),
    Size(f64),
    Color([f64; 3]),
    Alignment(Alignment),
    Tracking(f64),
    Leading(f64),
}

impl StyleChange {
    /// Applies the change; face and color go to the letters in `range` (all of them when it's
    /// empty), the rest to the whole text.
    pub fn apply(&self, style: &mut TextStyle, range: Range<usize>) {
        match self {
            StyleChange::Font(name) => style.set_font(name, range),
            StyleChange::Color(color) => style.set_color(*color, range),
            StyleChange::Size(size) => style.font_size = size.clamp(1.0, 2000.0),
            StyleChange::Alignment(alignment) => style.alignment = *alignment,
            StyleChange::Tracking(tracking) => style.tracking = tracking.clamp(-100.0, 1000.0),
            StyleChange::Leading(leading) => style.leading = leading.clamp(0.0, 5000.0),
        }
    }
}

/// Font family and style pickers. `current` is the face shown (`None` when the letters use several).
pub fn font_picker(ui: &mut egui::Ui, id_salt: &str, current: Option<&str>) -> Option<String> {
    let library = fonts::library();
    let found = current.and_then(|name| library.find(name));
    let mut chosen = None;
    let family_label = match (found, current) {
        (Some((family, _)), _) => family.name.clone(),
        (None, Some(name)) => format!("{name} (missing)"),
        (None, None) => "Multiple fonts".into(),
    };
    let search_id = egui::Id::new(("font-search", id_salt));
    // Clicks inside (in the search field) leave the list open; picking a family closes it.
    let config = egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let (response, _) = egui::containers::menu::MenuButton::new(family_label).config(config).ui(ui, |ui| {
        let mut query: String = ui.data_mut(|d| d.get_temp(search_id)).unwrap_or_default();
        let search = ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search fonts").desired_width(220.0));
        if !search.has_focus() && query.is_empty() {
            search.request_focus();
        }
        let needle = query.to_lowercase();
        let matches: Vec<&fonts::Family> =
            library.families().iter().filter(|f| needle.is_empty() || f.name.to_lowercase().contains(&needle)).collect();
        let row = ui.spacing().interact_size.y;
        egui::ScrollArea::vertical().max_height(320.0).show_rows(ui, row, matches.len(), |ui, rows| {
            for family in &matches[rows] {
                let selected = found.is_some_and(|(f, _)| f.name == family.name);
                if ui.selectable_label(selected, &family.name).clicked() {
                    // Keep the weight and slant when switching family, as far as the family has them.
                    let (weight, italic) = found.map_or((400, false), |(_, face)| (face.weight, face.italic));
                    let face = family
                        .faces
                        .iter()
                        .min_by_key(|f| (f.italic != italic, (f.weight as i32 - weight as i32).abs()))
                        .map(|f| f.post_script_name.clone());
                    chosen = face;
                    ui.close();
                }
            }
        });
        if matches.is_empty() {
            ui.weak("No matching fonts");
        }
        ui.data_mut(|d| d.insert_temp(search_id, query));
    });
    response.on_hover_text(current.unwrap_or("Several fonts"));
    if let Some((family, face)) = found {
        egui::ComboBox::from_id_salt(("font-style", id_salt)).selected_text(&face.style_name).show_ui(ui, |ui| {
            for f in &family.faces {
                if ui.selectable_label(f.post_script_name == face.post_script_name, &f.style_name).clicked() {
                    chosen = Some(f.post_script_name.clone());
                }
            }
        });
    }
    chosen
}

/// The type settings: face, size, color, alignment, tracking and leading, for `style` with the
/// letters in `range` selected. Returns what the user changed.
pub fn controls(ui: &mut egui::Ui, id_salt: &str, style: &TextStyle, range: Range<usize>) -> Option<StyleChange> {
    let mut change = None;
    let caret = if range.is_empty() { range.start.saturating_sub(1) } else { range.start };
    let face = if range.is_empty() { Some(style.font_name_at(caret).to_string()) } else { style.uniform_font_name(range.clone()) };
    if let Some(name) = font_picker(ui, id_salt, face.as_deref()) {
        change = Some(StyleChange::Font(name));
    }
    let mut size = style.font_size;
    if ui
        .add(egui::DragValue::new(&mut size).range(1.0..=2000.0).speed(0.5).max_decimals(1).suffix(" px"))
        .on_hover_text("Font size")
        .changed()
    {
        change = Some(StyleChange::Size(size));
    }
    let color = style.color_at(caret);
    let mut srgb = color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    if egui::color_picker::color_edit_button_srgb(ui, &mut srgb).on_hover_text("Text color").changed() {
        change = Some(StyleChange::Color(srgb.map(|c| c as f64 / 255.0)));
    }
    for alignment in Alignment::ALL {
        if ui
            .selectable_label(style.alignment == alignment, alignment.icon())
            .on_hover_text(format!("Align {}", alignment.name()))
            .clicked()
        {
            change = Some(StyleChange::Alignment(alignment));
        }
    }
    let mut tracking = style.tracking;
    ui.label("Tracking");
    if ui.add(egui::DragValue::new(&mut tracking).range(-100.0..=1000.0).speed(0.5).max_decimals(1)).changed() {
        change = Some(StyleChange::Tracking(tracking));
    }
    let mut leading = style.leading;
    ui.label("Leading");
    let auto = style.auto_leading();
    if ui
        .add(
            egui::DragValue::new(&mut leading)
                .range(0.0..=5000.0)
                .speed(0.5)
                .max_decimals(1)
                .custom_formatter(move |v, _| if v <= 0.0 { format!("Auto ({auto:.0})") } else { format!("{v:.1}") })
                .custom_parser(|s| if s.trim().eq_ignore_ascii_case("auto") { Some(0.0) } else { s.trim().parse().ok() }),
        )
        .on_hover_text("Baseline to baseline; 0 is Auto")
        .changed()
    {
        change = Some(StyleChange::Leading(leading));
    }
    change
}

/// A note for text whose face isn't installed.
pub fn missing_font_note(style: &TextStyle) -> Option<String> {
    let library = fonts::library();
    let mut names: Vec<&str> = vec![style.font_name.as_str()];
    names.extend(style.font_runs.iter().flatten().map(|r| r.font_name.as_str()));
    let missing: Vec<&str> = names.into_iter().filter(|n| !library.is_installed(n)).collect();
    let first = missing.first()?;
    Some(format!("“{first}” isn’t installed. The text keeps its look until it’s edited, then uses the closest installed font."))
}

/// The Properties panel's text settings for the active text layer; changes apply to all of it.
pub fn properties_ui(ui: &mut egui::Ui, project: &mut Project, _colors: &mut Colors, _cache: &RenderCache) {
    let Some(style) = project.doc.active_layer().and_then(|l| l.text.clone()) else { return };
    if let Some(note) = missing_font_note(&style) {
        ui.label(egui::RichText::new(note).small().weak());
    }
    let mut change = None;
    ui.horizontal_wrapped(|ui| {
        change = controls(ui, "properties", &style, 0..0);
    });
    ui.horizontal(|ui| {
        let mut boxed = style.box_size.is_some();
        if ui.checkbox(&mut boxed, "Paragraph box").on_hover_text("Wrap the text inside a fixed box").changed() {
            let mut next = style.clone();
            next.box_size = if boxed {
                project.doc.active_layer().map(|l| {
                    let (w, h) = l.pixel_size();
                    [(w as f64).max(16.0), (h as f64).max(16.0)]
                })
            } else {
                None
            };
            crate::ui::properties::change(project, |doc| {
                if let Some(layer) = doc.active_layer_mut() {
                    restyle(layer, next);
                }
            });
        }
    });
    if let Some(change) = change {
        let mut next = style.clone();
        change.apply(&mut next, 0..0);
        if next != style {
            crate::ui::properties::change(project, |doc| {
                if let Some(layer) = doc.active_layer_mut() {
                    restyle(layer, next);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_mac_json() {
        let json = r#"{"content":"Hi there","fontName":"HelveticaNeue-Bold","fontSize":48,"red":1,"green":0.5,"blue":0,"alignment":"Center","tracking":2,"leading":60,"boxSize":[300,120],"colorRuns":[{"location":0,"length":2,"red":0,"green":0,"blue":1}],"fontRuns":[{"location":3,"length":5,"fontName":"Menlo-Regular"}],"futureField":true}"#;
        let style: TextStyle = serde_json::from_str(json).unwrap();
        assert_eq!(style.font_name, "HelveticaNeue-Bold");
        assert_eq!(style.font_size, 48.0);
        assert_eq!(style.alignment, Alignment::Center);
        assert_eq!(style.box_size, Some([300.0, 120.0]));
        assert_eq!(style.color_runs.as_ref().unwrap()[0].length, 2);
        assert_eq!(style.font_runs.as_ref().unwrap()[0].font_name, "Menlo-Regular");
        assert!(style.is_valid());
        let value = serde_json::to_value(&style).unwrap();
        for key in ["content", "fontName", "fontSize", "red", "green", "blue", "alignment", "tracking", "leading", "boxSize", "colorRuns", "fontRuns", "futureField"] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
        assert_eq!(value["fontRuns"][0]["fontName"], "Menlo-Regular");
        assert_eq!(value["boxSize"], serde_json::json!([300.0, 120.0]));
        let back: TextStyle = serde_json::from_value(value).unwrap();
        assert_eq!(back, style);
        // Point text omits the optional fields.
        let plain = serde_json::to_value(TextStyle::default()).unwrap();
        assert!(plain.get("boxSize").is_none() && plain.get("colorRuns").is_none() && plain.get("fontRuns").is_none());
        assert_eq!(plain["alignment"], "Left");
    }

    #[test]
    fn runs_follow_edits() {
        let mut style = TextStyle { content: "abcdef".into(), ..TextStyle::default() };
        style.set_color([1.0, 0.0, 0.0], 1..3);
        assert_eq!(style.color_runs.as_ref().unwrap(), &vec![ColorRun { location: 1, length: 2, red: 1.0, green: 0.0, blue: 0.0 }]);
        // Typing after a red letter continues in red.
        style.replace(3..3, "XY");
        assert_eq!(style.content, "abcXYdef");
        assert_eq!(style.color_runs.as_ref().unwrap()[0].length, 4);
        style.set_font("Menlo-Regular", 0..2);
        assert_eq!(style.uniform_font_name(0..2).as_deref(), Some("Menlo-Regular"));
        assert_eq!(style.uniform_font_name(0..3), None);
        style.replace(0..1, "");
        assert_eq!(style.font_runs.as_ref().unwrap()[0], FontRun { location: 0, length: 1, font_name: "Menlo-Regular".into() });
        // Recoloring everything drops the runs.
        style.set_color([0.0, 0.0, 1.0], 0..0);
        assert!(style.color_runs.is_none());
        assert!(style.is_valid());
        // UTF-16 offsets: an emoji is two units.
        let mut emoji = TextStyle { content: "a😀b".into(), ..TextStyle::default() };
        assert_eq!(emoji.len16(), 4);
        emoji.set_color([1.0, 0.0, 0.0], 3..4);
        assert_eq!(emoji.color_at(3), [1.0, 0.0, 0.0]);
        emoji.replace(1..3, "");
        assert_eq!(emoji.content, "ab");
        assert_eq!(emoji.color_runs.as_ref().unwrap()[0].location, 1);
    }
}
