//! System fonts by PostScript name. Projects from the macOS app name faces such as
//! "HelveticaNeue-Bold"; when a face isn't installed, the nearest installed family and weight
//! stand in for it (a sans-serif such as Liberation Sans or DejaVu Sans when nothing is close),
//! while the layer keeps the original name until the user picks another.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use ab_glyph::{Font, FontArc, FontVec};
use fontdb::{Database, Family as QueryFamily, Query, Stretch, Style, Weight, ID};

/// One installed face, for the font picker.
#[derive(Clone, Debug)]
pub struct Face {
    pub post_script_name: String,
    /// "Bold", "Light Italic", …
    pub style_name: String,
    pub weight: u16,
    pub italic: bool,
}

/// An installed family and its faces, lightest first.
#[derive(Clone, Debug)]
pub struct Family {
    pub name: String,
    pub faces: Vec<Face>,
}

pub struct Library {
    db: Database,
    families: Vec<Family>,
    by_post_script: HashMap<String, ID>,
    loaded: Mutex<HashMap<ID, Option<FontArc>>>,
    /// Face index within its file (for collections), by font data address.
    indices: Mutex<HashMap<usize, u32>>,
    resolved: Mutex<HashMap<String, Option<ID>>>,
    fallbacks: Vec<ID>,
    default_sans: Option<ID>,
}

/// Families that stand in for common macOS faces, in order of preference.
const SANS: &[&str] = &[
    "Helvetica",
    "Helvetica Neue",
    "Arial",
    "Liberation Sans",
    "Nimbus Sans",
    "DejaVu Sans",
    "Noto Sans",
    "Cantarell",
    "Ubuntu",
];
const SERIF: &[&str] = &["Times New Roman", "Times", "Liberation Serif", "Nimbus Roman", "DejaVu Serif", "Noto Serif"];
const MONO: &[&str] = &["Courier New", "Courier", "Liberation Mono", "Nimbus Mono PS", "DejaVu Sans Mono", "Noto Sans Mono"];
/// Tried, in order, for letters the chosen face doesn't have.
const FALLBACK: &[&str] = &[
    "DejaVu Sans",
    "Noto Sans",
    "Noto Sans CJK SC",
    "Noto Sans CJK JP",
    "Noto Sans Symbols",
    "Noto Sans Symbols2",
    "Arial Unicode MS",
    "PingFang SC",
    "Hiragino Sans",
    "Apple Symbols",
];

static LIBRARY: OnceLock<Library> = OnceLock::new();

/// The installed fonts, discovered on first use.
pub fn library() -> &'static Library {
    LIBRARY.get_or_init(|| {
        let mut db = Database::new();
        db.load_system_fonts();
        Library::new(db)
    })
}

/// Starts discovering fonts in the background so the first text edit doesn't wait for it.
pub fn warm_up() {
    if LIBRARY.get().is_none() {
        std::thread::spawn(|| {
            library();
        });
    }
}

fn normalized(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

pub fn style_name(weight: u16, italic: bool, stretch: Stretch) -> String {
    let mut parts: Vec<&str> = Vec::new();
    match stretch {
        Stretch::UltraCondensed | Stretch::ExtraCondensed | Stretch::Condensed => parts.push("Condensed"),
        Stretch::SemiCondensed => parts.push("Semi Condensed"),
        Stretch::SemiExpanded => parts.push("Semi Expanded"),
        Stretch::Expanded | Stretch::ExtraExpanded | Stretch::UltraExpanded => parts.push("Expanded"),
        Stretch::Normal => {}
    }
    parts.push(match weight {
        0..=149 => "Thin",
        150..=249 => "Extra Light",
        250..=349 => "Light",
        350..=449 => "Regular",
        450..=549 => "Medium",
        550..=649 => "Semibold",
        650..=749 => "Bold",
        750..=849 => "Extra Bold",
        _ => "Black",
    });
    if italic {
        if parts.last() == Some(&"Regular") {
            parts.pop();
        }
        parts.push("Italic");
    }
    parts.join(" ")
}

/// The weight and slant a PostScript style suffix such as "BoldItalic" or "LightOblique" asks for.
fn requested_style(style: &str) -> (u16, bool) {
    let s = style.to_lowercase();
    let italic = s.contains("italic") || s.contains("oblique") || s.ends_with("it");
    let weight = if s.contains("hairline") || s.contains("thin") {
        100
    } else if s.contains("ultralight") || s.contains("extralight") {
        200
    } else if s.contains("light") {
        300
    } else if s.contains("medium") {
        500
    } else if s.contains("semibold") || s.contains("demibold") || s.contains("demi") {
        600
    } else if s.contains("extrabold") || s.contains("ultrabold") {
        800
    } else if s.contains("black") || s.contains("heavy") {
        900
    } else if s.contains("bold") {
        700
    } else {
        400
    };
    (weight, italic)
}

/// The family part of a PostScript name, without vendor suffixes ("ArialMT" → "arial").
fn requested_family(post_script: &str) -> String {
    let family = post_script.split('-').next().unwrap_or(post_script);
    let mut key = normalized(family);
    for suffix in ["psmt", "mt", "ps", "std", "pro"] {
        if key.len() > suffix.len() + 2 && key.ends_with(suffix) {
            key.truncate(key.len() - suffix.len());
            break;
        }
    }
    key
}

impl Library {
    fn new(db: Database) -> Self {
        let mut grouped: HashMap<String, Family> = HashMap::new();
        let mut by_post_script = HashMap::new();
        for info in db.faces() {
            let Some((family, _)) = info.families.first() else { continue };
            if family.starts_with('.') || info.post_script_name.is_empty() {
                continue;
            }
            by_post_script.entry(info.post_script_name.clone()).or_insert(info.id);
            let italic = info.style != Style::Normal;
            let face = Face {
                post_script_name: info.post_script_name.clone(),
                style_name: style_name(info.weight.0, italic, info.stretch),
                weight: info.weight.0,
                italic,
            };
            let entry = grouped.entry(family.clone()).or_insert_with(|| Family { name: family.clone(), faces: Vec::new() });
            if !entry.faces.iter().any(|f| f.post_script_name == face.post_script_name) {
                entry.faces.push(face);
            }
        }
        let mut families: Vec<Family> = grouped.into_values().collect();
        for family in &mut families {
            family.faces.sort_by_key(|f| (f.weight, f.italic));
            // Two faces with the same description (widths the style name doesn't cover) are told
            // apart by their PostScript suffix.
            let names: Vec<String> = family.faces.iter().map(|f| f.style_name.clone()).collect();
            for (index, face) in family.faces.iter_mut().enumerate() {
                if names.iter().filter(|n| **n == names[index]).count() > 1 {
                    if let Some(suffix) = face.post_script_name.split('-').nth(1) {
                        face.style_name = format!("{} ({suffix})", face.style_name);
                    }
                }
            }
        }
        families.sort_by_key(|f| f.name.to_lowercase());
        let mut library = Library {
            db,
            families,
            by_post_script,
            loaded: Mutex::new(HashMap::new()),
            indices: Mutex::new(HashMap::new()),
            resolved: Mutex::new(HashMap::new()),
            fallbacks: Vec::new(),
            default_sans: None,
        };
        library.default_sans = library.query(SANS, 400, false).or_else(|| library.db.faces().next().map(|f| f.id));
        let mut fallbacks: Vec<ID> = library.default_sans.into_iter().collect();
        for name in FALLBACK {
            if let Some(id) = library.query(&[name], 400, false) {
                if !fallbacks.contains(&id) {
                    fallbacks.push(id);
                }
            }
        }
        library.fallbacks = fallbacks;
        library
    }

    pub fn is_empty(&self) -> bool {
        self.families.is_empty()
    }

    pub fn families(&self) -> &[Family] {
        &self.families
    }

    /// The family and face a PostScript name belongs to, when it is installed.
    pub fn find(&self, post_script: &str) -> Option<(&Family, &Face)> {
        self.families.iter().find_map(|family| {
            family.faces.iter().find(|f| f.post_script_name == post_script).map(|face| (family, face))
        })
    }

    pub fn is_installed(&self, post_script: &str) -> bool {
        self.by_post_script.contains_key(post_script)
    }

    /// The PostScript name new text uses: Helvetica when it's installed, otherwise the first
    /// common sans-serif.
    pub fn default_post_script_name(&self) -> String {
        self.default_sans
            .and_then(|id| self.db.face(id))
            .map(|f| f.post_script_name.clone())
            .unwrap_or_else(|| "Helvetica".into())
    }

    fn query(&self, names: &[&str], weight: u16, italic: bool) -> Option<ID> {
        let families: Vec<QueryFamily> = names.iter().map(|n| QueryFamily::Name(n)).collect();
        let query = Query {
            families: &families,
            weight: Weight(weight),
            stretch: Stretch::Normal,
            style: if italic { Style::Italic } else { Style::Normal },
        };
        // fontdb falls back to the first family that exists at all, so check it's one we named.
        let id = self.db.query(&query)?;
        let face = self.db.face(id)?;
        names
            .iter()
            .any(|n| face.families.iter().any(|(f, _)| f.eq_ignore_ascii_case(n)))
            .then_some(id)
    }

    fn resolve_id(&self, post_script: &str) -> Option<ID> {
        if let Some(found) = self.resolved.lock().unwrap().get(post_script) {
            return *found;
        }
        let result = self.lookup(post_script);
        self.resolved.lock().unwrap().insert(post_script.to_string(), result);
        result
    }

    fn lookup(&self, post_script: &str) -> Option<ID> {
        if let Some(id) = self.by_post_script.get(post_script) {
            return Some(*id);
        }
        if let Some((_, id)) = self.by_post_script.iter().find(|(name, _)| name.eq_ignore_ascii_case(post_script)) {
            return Some(*id);
        }
        let style = post_script.split_once('-').map(|(_, s)| s).unwrap_or("");
        let (weight, italic) = requested_style(style);
        // The same family under its display name ("HelveticaNeue" → "Helvetica Neue").
        let family = requested_family(post_script);
        if !family.is_empty() {
            if let Some(found) = self.families.iter().find(|f| normalized(&f.name) == family) {
                let names = [found.name.as_str()];
                if let Some(id) = self.query(&names, weight, italic) {
                    return Some(id);
                }
            }
        }
        let lower = post_script.to_lowercase();
        let substitutes = if lower.starts_with("times") || lower.contains("georgia") || lower.contains("serif") && !lower.contains("sans") {
            SERIF
        } else if ["courier", "menlo", "monaco", "mono", "sfmono"].iter().any(|m| lower.contains(m)) {
            MONO
        } else {
            SANS
        };
        self.query(substitutes, weight, italic)
            .or_else(|| self.query(SANS, weight, italic))
            .or(self.default_sans)
    }

    fn load(&self, id: ID) -> Option<FontArc> {
        if let Some(font) = self.loaded.lock().unwrap().get(&id) {
            return font.clone();
        }
        let font = self
            .db
            .with_face_data(id, |data, index| {
                let font = FontArc::new(FontVec::try_from_vec_and_index(data.to_vec(), index).ok()?);
                self.indices.lock().unwrap().insert(font.font_data().as_ptr() as usize, index);
                Some(font)
            })
            .flatten();
        self.loaded.lock().unwrap().insert(id, font.clone());
        font
    }

    /// The face for a PostScript name: the face itself, or the closest installed stand-in.
    pub fn resolve(&self, post_script: &str) -> Option<FontArc> {
        let id = self.resolve_id(post_script)?;
        self.load(id).or_else(|| self.default_sans.and_then(|d| self.load(d)))
    }

    /// Which face of its file `font` is.
    pub fn face_index(&self, font: &FontArc) -> u32 {
        self.indices.lock().unwrap().get(&(font.font_data().as_ptr() as usize)).copied().unwrap_or(0)
    }

    /// A face with a glyph for `c`, for letters the chosen face lacks.
    pub fn fallback_for(&self, c: char) -> Option<FontArc> {
        self.fallbacks.iter().filter_map(|id| self.load(*id)).find(|font| font.glyph_id(c).0 != 0)
    }
}

/// Shared handle to a loaded face, compared by identity.
pub fn same_font(a: &FontArc, b: &FontArc) -> bool {
    std::ptr::eq(a.font_data().as_ptr(), b.font_data().as_ptr())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_post_script_styles() {
        assert_eq!(requested_style("Bold"), (700, false));
        assert_eq!(requested_style("BoldItalic"), (700, true));
        assert_eq!(requested_style("LightOblique"), (300, true));
        assert_eq!(requested_style("SemiboldIt"), (600, true));
        assert_eq!(requested_family("HelveticaNeue-Bold"), "helveticaneue");
        assert_eq!(requested_family("ArialMT"), "arial");
        assert_eq!(style_name(700, true, Stretch::Normal), "Bold Italic");
        assert_eq!(style_name(400, true, Stretch::Normal), "Italic");
    }
}
