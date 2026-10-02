//! Photoshop type layers (`TySh`) as editable text. The block holds a version, a 2×3 transform,
//! a text descriptor and a warp descriptor; the descriptor's `EngineData` (a PostScript-like
//! dictionary) supplies the font, size, color, tracking, leading and alignment. Anything the text
//! model can't represent (vertical text, shear, uneven scale, a frame too large) stays pixels.

use std::collections::HashMap;

use crate::text::{Alignment, TextStyle, PADDING};

pub struct Source {
    pub style: TextStyle,
    /// Document point the text's anchor lands on: the paragraph frame's top-left, or for point
    /// text where its first baseline meets the alignment edge.
    pub anchor: (f64, f64),
    pub anchor_is_frame: bool,
    pub rotation: f64,
    pub flip_y: bool,
}

pub fn parse(extra: &HashMap<String, Vec<u8>>) -> Option<Source> {
    let data = extra.get("TySh").or_else(|| extra.get("tySh"))?;
    if data.len() > 8_000_000 {
        return None;
    }
    let mut r = Reader { data, offset: 0 };
    if r.u16()? != 1 {
        return None;
    }
    let m: Vec<f64> = (0..6).map(|_| r.f64()).collect::<Option<_>>()?;
    if !m.iter().all(|v| v.is_finite()) {
        return None;
    }
    if r.u16()? != 50 {
        return None;
    }
    let text = r.descriptor(true)?;
    if enumeration(&text, "Ornt").as_deref() == Some("Vrtc") {
        return None;
    }
    let placed = Placement::new(m[0], m[1], m[2], m[3], m[4], m[5])?;
    let engine = match text.get("EngineData") {
        Some(Descriptor::Data(bytes)) => engine_value(bytes),
        _ => None,
    };
    let raw = match text.get("Txt ").or_else(|| text.get("Txt")) {
        Some(Descriptor::Text(t)) => Some(t.clone()),
        _ => engine.as_ref().and_then(|e| walk(Some(e), &["EngineDict", "Editor", "Text"]).and_then(Engine::string)),
    };
    let content = cleaned(&raw?);
    if content.is_empty() || crate::text::utf16_len(&content) > crate::text::MAX_LENGTH {
        return None;
    }
    let mut style = TextStyle { content, ..TextStyle::default() };
    match &engine {
        Some(engine) => apply_style(&mut style, engine, placed.scale),
        None => style.font_size = (12.0 * placed.scale).clamp(1.0, 2000.0),
    }
    if !style.font_size.is_finite() || style.font_size <= 0.0 {
        return None;
    }
    let mut anchor = (placed.tx, placed.ty);
    let mut anchor_is_frame = false;
    if let (Some(bounds), Some(glyphs)) = (rect(&text, "bounds"), rect(&text, "boundingBox")) {
        let (bw, bh) = (bounds.2 - bounds.0, bounds.3 - bounds.1);
        if bw > (glyphs.2 - glyphs.0) + 4.0 && bh > (glyphs.3 - glyphs.1) + 4.0 && bw > 1.0 && bh > 1.0 {
            let mut boxed = style.clone();
            boxed.box_size = Some([(bw * placed.scale + PADDING * 2.0).round(), (bh * placed.scale + PADDING * 2.0).round()]);
            // A paragraph frame the model can't store is dropped entirely: importing it as point
            // text would lose the wrap without saying so.
            if !boxed.is_valid() {
                return None;
            }
            style = boxed;
            anchor = placed.map(bounds.0, bounds.1);
            anchor_is_frame = true;
        }
    }
    if !style.is_valid() {
        return None;
    }
    Some(Source { style, anchor, anchor_is_frame, rotation: placed.rotation, flip_y: placed.flip_y })
}

/// Uniform scale, rotation and an optional vertical flip; shear and uneven scale don't fit.
struct Placement {
    scale: f64,
    rotation: f64,
    flip_y: bool,
    tx: f64,
    ty: f64,
    m: [f64; 4],
}

impl Placement {
    fn new(xx: f64, xy: f64, yx: f64, yy: f64, tx: f64, ty: f64) -> Option<Placement> {
        let scale_x = xx.hypot(yx);
        if scale_x <= 1e-6 {
            return None;
        }
        let (cos, sin) = (xx / scale_x, yx / scale_x);
        let local_x = cos * xy + sin * yy;
        let local_y = -sin * xy + cos * yy;
        let scale_y = local_y.abs();
        if scale_y <= 1e-6 {
            return None;
        }
        let largest = scale_x.max(scale_y);
        if local_x.abs() > 0.02 * largest || (scale_x - scale_y).abs() > 0.02 * largest {
            return None;
        }
        let sign = if local_y < 0.0 { -1.0 } else { 1.0 };
        Some(Placement {
            scale: scale_x,
            rotation: sin.atan2(cos).to_degrees(),
            flip_y: local_y < 0.0,
            tx,
            ty,
            m: [cos * scale_x, sin * scale_x, -sin * scale_x * sign, cos * scale_x * sign],
        })
    }

    fn map(&self, x: f64, y: f64) -> (f64, f64) {
        (self.m[0] * x + self.m[2] * y + self.tx, self.m[1] * x + self.m[3] * y + self.ty)
    }
}

/// The first style run's settings; Photoshop's other runs, faux styles and full justification
/// aren't kept, as in the macOS app.
fn apply_style(style: &mut TextStyle, engine: &Engine, scale: f64) {
    let runs = walk(Some(engine), &["EngineDict", "StyleRun", "RunArray"]).map(Engine::array).unwrap_or_default();
    let first = runs.first().unwrap_or(engine);
    let data = walk(Some(first), &["StyleSheet", "StyleSheetData"]).unwrap_or(first);
    let points = number(walk(Some(data), &["FontSize"])).unwrap_or(12.0);
    if !points.is_finite() || points <= 0.0 {
        return;
    }
    style.font_size = (points * scale).clamp(1.0, 2000.0);
    let fonts = walk(Some(engine), &["ResourceDict", "FontSet"]).map(Engine::array).unwrap_or_default();
    let index = number(walk(Some(data), &["Font"])).unwrap_or(0.0).round();
    if index >= 0.0 {
        if let Some(name) = fonts.get(index as usize).and_then(|f| walk(Some(f), &["Name"])).and_then(Engine::string) {
            if !name.is_empty() {
                style.font_name = name;
            }
        }
    }
    let values: Vec<f64> = walk(Some(data), &["FillColor", "Values"]).map(Engine::array).unwrap_or_default().iter().filter_map(|v| number(Some(v))).collect();
    if !values.is_empty() {
        [style.red, style.green, style.blue] = color(&values);
    }
    let tracking = number(walk(Some(data), &["Tracking"])).unwrap_or(0.0);
    if tracking.is_finite() {
        style.tracking = (tracking * style.font_size / 1000.0).clamp(-100.0, 1000.0);
    }
    let auto = boolean(walk(Some(data), &["AutoLeading"])).unwrap_or(true);
    if !auto {
        if let Some(leading) = number(walk(Some(data), &["Leading"])).filter(|l| l.is_finite() && *l > 0.0) {
            style.leading = (leading * scale).clamp(0.0, 5000.0);
        }
    }
    let paragraphs = walk(Some(engine), &["EngineDict", "ParagraphRun", "RunArray"]).map(Engine::array).unwrap_or_default();
    let justification = number(walk(Some(paragraphs.first().unwrap_or(engine)), &["ParagraphSheet", "Properties", "Justification"]));
    style.alignment = match justification.unwrap_or(0.0).round() as i64 {
        1 => Alignment::Right,
        2 => Alignment::Center,
        _ => Alignment::Left,
    };
}

fn color(values: &[f64]) -> [f64; 3] {
    let unit = |v: f64| if v > 1.0 { v.min(255.0) / 255.0 } else { v.clamp(0.0, 1.0) };
    match values.len() {
        0 => [0.0; 3],
        1 | 2 => [unit(values[0]); 3],
        3 => [unit(values[0]), unit(values[1]), unit(values[2])],
        _ => [unit(values[1]), unit(values[2]), unit(values[3])],
    }
}

fn cleaned(text: &str) -> String {
    text.trim_start_matches(['\u{feff}', '\0']).trim_end_matches('\0').replace("\r\n", "\n").replace('\r', "\n")
}

/// Where the text layer goes so that `image_anchor` (layer pixels of a `size` image) lands on
/// `anchor`: flipped, then turned clockwise about the layer's center, as layer transforms are.
pub fn layer_transform(size: (f64, f64), image_anchor: (f64, f64), anchor: (f64, f64), rotation: f64, flip_y: bool) -> crate::doc::LayerTransform {
    let mut local = (image_anchor.0 - size.0 / 2.0, image_anchor.1 - size.1 / 2.0);
    if flip_y {
        local.1 = -local.1;
    }
    let (sin, cos) = rotation.to_radians().sin_cos();
    let rotated = (local.0 * cos - local.1 * sin, local.0 * sin + local.1 * cos);
    let center = (anchor.0 - rotated.0, anchor.1 - rotated.1);
    let mut transform = crate::doc::LayerTransform::rect(center.0 - size.0 / 2.0, center.1 - size.1 / 2.0, size.0, size.1);
    transform.rotation = rotation;
    transform.flip_y = flip_y;
    transform
}

// Action descriptors.

#[derive(Clone, Debug)]
enum Descriptor {
    Text(String),
    Number(f64),
    Enum(String),
    Data(Vec<u8>),
    Object(HashMap<String, Descriptor>),
    List,
}

fn enumeration(items: &HashMap<String, Descriptor>, key: &str) -> Option<String> {
    match items.get(key) {
        Some(Descriptor::Enum(e)) => Some(e.clone()),
        _ => None,
    }
}

/// A rectangle descriptor as `(left, top, right, bottom)`.
fn rect(items: &HashMap<String, Descriptor>, key: &str) -> Option<(f64, f64, f64, f64)> {
    let Some(Descriptor::Object(sides)) = items.get(key) else { return None };
    let side = |name: &str| match sides.get(name).or_else(|| sides.get(name.trim())) {
        Some(Descriptor::Number(v)) if v.is_finite() => Some(*v),
        _ => None,
    };
    Some((side("Left")?, side("Top ")?, side("Rght")?, side("Btom")?))
}

struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(count)?;
        let slice = self.data.get(self.offset..end)?;
        self.offset = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.bytes(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.bytes(4).map(|b| u32::from_be_bytes(b.try_into().unwrap()))
    }

    fn f64(&mut self) -> Option<f64> {
        self.bytes(8).map(|b| f64::from_be_bytes(b.try_into().unwrap()))
    }

    fn four_cc(&mut self) -> Option<String> {
        self.bytes(4).map(|b| String::from_utf8_lossy(b).into_owned())
    }

    fn unicode(&mut self) -> Option<String> {
        let count = self.u32()? as usize;
        if count > 1_000_000 {
            return None;
        }
        let raw = self.bytes(count * 2)?;
        let units: Vec<u16> = raw.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        Some(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string())
    }

    /// A class or key: length-prefixed, or a four-character code when the length is 0.
    fn identifier(&mut self) -> Option<String> {
        let length = self.u32()? as usize;
        if length == 0 {
            return self.four_cc();
        }
        if length > 10_000 {
            return None;
        }
        self.bytes(length).map(|b| String::from_utf8_lossy(b).into_owned())
    }

    fn descriptor(&mut self, versioned: bool) -> Option<HashMap<String, Descriptor>> {
        if versioned && self.u32()? != 16 {
            return None;
        }
        self.unicode()?;
        self.identifier()?;
        let count = self.u32()?;
        if count > 10_000 {
            return None;
        }
        let mut items = HashMap::new();
        for _ in 0..count {
            let key = self.identifier()?;
            let kind = self.four_cc()?;
            let value = self.value(&kind)?;
            items.insert(key, value);
        }
        Some(items)
    }

    fn value(&mut self, kind: &str) -> Option<Descriptor> {
        Some(match kind {
            "doub" => Descriptor::Number(self.f64()?),
            "UntF" => {
                self.four_cc()?;
                Descriptor::Number(self.f64()?)
            }
            "UnFl" => {
                self.four_cc()?;
                let count = self.u32()? as usize;
                let mut first = None;
                for _ in 0..count.min(10_000) {
                    let v = self.f64()?;
                    first.get_or_insert(v);
                }
                Descriptor::Number(first.unwrap_or(0.0))
            }
            "long" => Descriptor::Number(self.u32()? as i32 as f64),
            "comp" => Descriptor::Number(i64::from_be_bytes(self.bytes(8)?.try_into().unwrap()) as f64),
            "bool" => Descriptor::Number(self.u8()? as f64),
            "TEXT" => Descriptor::Text(self.unicode()?),
            "enum" => {
                self.identifier()?;
                Descriptor::Enum(self.identifier()?)
            }
            "tdta" => {
                let length = self.u32()? as usize;
                if length > 8_000_000 {
                    return None;
                }
                Descriptor::Data(self.bytes(length)?.to_vec())
            }
            "Objc" | "GlbO" => Descriptor::Object(self.descriptor(false)?),
            "VlLs" => {
                let count = self.u32()?;
                if count > 10_000 {
                    return None;
                }
                for _ in 0..count {
                    let kind = self.four_cc()?;
                    self.value(&kind)?;
                }
                Descriptor::List
            }
            "alis" => {
                let length = self.u32()? as usize;
                self.bytes(length)?;
                Descriptor::Number(0.0)
            }
            "obj " => {
                self.reference()?;
                Descriptor::Number(0.0)
            }
            "type" | "GlbC" => {
                self.unicode()?;
                self.identifier()?;
                Descriptor::Number(0.0)
            }
            _ => return None,
        })
    }

    /// Skips a reference so later items can still be read.
    fn reference(&mut self) -> Option<()> {
        let count = self.u32()?;
        if count > 10_000 {
            return None;
        }
        for _ in 0..count {
            match self.four_cc()?.as_str() {
                "prop" => {
                    self.unicode()?;
                    self.identifier()?;
                    self.identifier()?;
                }
                "Clss" => {
                    self.unicode()?;
                    self.identifier()?;
                }
                "Enmr" => {
                    self.unicode()?;
                    self.identifier()?;
                    self.identifier()?;
                    self.identifier()?;
                }
                "rele" => {
                    self.unicode()?;
                    self.identifier()?;
                    self.u32()?;
                }
                "Idnt" | "indx" => {
                    self.u32()?;
                }
                "name" => {
                    self.unicode()?;
                }
                _ => return None,
            }
        }
        Some(())
    }
}

// The text engine's dictionary.

#[derive(Clone, Debug, PartialEq)]
enum Engine {
    Number(f64),
    Bool(bool),
    String(String),
    Dict(HashMap<String, Engine>),
    Array(Vec<Engine>),
}

impl Engine {
    fn string(&self) -> Option<String> {
        match self {
            Engine::String(s) => Some(s.clone()),
            _ => None,
        }
    }

    fn array(&self) -> Vec<Engine> {
        match self {
            Engine::Array(items) => items.clone(),
            _ => Vec::new(),
        }
    }
}

fn walk<'a>(value: Option<&'a Engine>, keys: &[&str]) -> Option<&'a Engine> {
    let mut current = value?;
    for key in keys {
        match current {
            Engine::Dict(items) => current = items.get(*key)?,
            _ => return None,
        }
    }
    Some(current)
}

fn number(value: Option<&Engine>) -> Option<f64> {
    match value? {
        Engine::Number(n) => Some(*n),
        _ => None,
    }
}

fn boolean(value: Option<&Engine>) -> Option<bool> {
    match value? {
        Engine::Bool(b) => Some(*b),
        _ => None,
    }
}

fn engine_value(data: &[u8]) -> Option<Engine> {
    let mut parser = EngineParser { bytes: data, index: 0 };
    if let Some(dict @ Engine::Dict(_)) = parser.value() {
        return Some(dict);
    }
    let start = data.windows(2).position(|w| w == b"<<")?;
    let mut parser = EngineParser { bytes: data, index: start };
    match parser.value()? {
        dict @ Engine::Dict(_) => Some(dict),
        _ => None,
    }
}

struct EngineParser<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl EngineParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn skip_whitespace(&mut self) {
        while let Some(b) = self.peek() {
            if b == b'%' {
                while self.peek().is_some_and(|b| b != b'\n' && b != b'\r') {
                    self.index += 1;
                }
            } else if b <= 0x20 {
                self.index += 1;
            } else {
                break;
            }
        }
    }

    fn take(&mut self, token: &[u8]) -> bool {
        if self.bytes[self.index.min(self.bytes.len())..].starts_with(token) {
            self.index += token.len();
            true
        } else {
            false
        }
    }

    fn is_delimiter(b: u8) -> bool {
        b <= 0x20 || b"/<>[]()".contains(&b)
    }

    fn token(&mut self) -> String {
        let start = self.index;
        while self.peek().is_some_and(|b| !Self::is_delimiter(b)) {
            self.index += 1;
        }
        String::from_utf8_lossy(&self.bytes[start..self.index]).into_owned()
    }

    fn take_word(&mut self, word: &[u8]) -> bool {
        let rest = &self.bytes[self.index.min(self.bytes.len())..];
        if rest.starts_with(word) && rest.get(word.len()).is_none_or(|b| Self::is_delimiter(*b)) {
            self.index += word.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Option<Engine> {
        self.skip_whitespace();
        let b = self.peek()?;
        match b {
            b'<' if self.bytes.get(self.index + 1) == Some(&b'<') => self.dictionary(),
            b'<' => self.hex(),
            b'[' => self.array(),
            b'(' => self.string(),
            b'/' => {
                self.index += 1;
                Some(Engine::String(self.token()))
            }
            b'-' | b'+' | b'.' | b'0'..=b'9' => self.number(),
            _ if self.take_word(b"true") => Some(Engine::Bool(true)),
            _ if self.take_word(b"false") => Some(Engine::Bool(false)),
            _ if self.take_word(b"null") => Some(Engine::String(String::new())),
            _ => None,
        }
    }

    fn dictionary(&mut self) -> Option<Engine> {
        if !self.take(b"<<") {
            return None;
        }
        let mut items = HashMap::new();
        loop {
            self.skip_whitespace();
            match self.peek() {
                None | Some(b'>') => break,
                Some(b'/') => {
                    self.index += 1;
                    let key = self.token();
                    let value = self.value()?;
                    items.insert(key, value);
                }
                _ => return None,
            }
        }
        self.take(b">>").then_some(Engine::Dict(items))
    }

    fn array(&mut self) -> Option<Engine> {
        self.take(b"[");
        let mut items = Vec::new();
        loop {
            self.skip_whitespace();
            match self.peek() {
                None | Some(b']') => break,
                _ => items.push(self.value()?),
            }
        }
        self.take(b"]").then_some(Engine::Array(items))
    }

    fn number(&mut self) -> Option<Engine> {
        let start = self.index;
        let digits = |p: &mut Self| {
            while p.peek().is_some_and(|b| b.is_ascii_digit()) {
                p.index += 1;
            }
        };
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.index += 1;
        }
        digits(self);
        if self.peek() == Some(b'.') {
            self.index += 1;
            digits(self);
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.index += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.index += 1;
            }
            digits(self);
        }
        let text = std::str::from_utf8(&self.bytes[start..self.index]).ok()?;
        text.parse().ok().map(Engine::Number)
    }

    fn string(&mut self) -> Option<Engine> {
        self.take(b"(");
        let mut raw = Vec::new();
        while let Some(b) = self.peek() {
            self.index += 1;
            match b {
                b')' => break,
                b'\\' => {
                    let escaped = self.peek()?;
                    self.index += 1;
                    match escaped {
                        b'n' => raw.push(b'\n'),
                        b'r' => raw.push(b'\r'),
                        b't' => raw.push(b'\t'),
                        b'0'..=b'7' => {
                            let mut value = (escaped - b'0') as u32;
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        self.index += 1;
                                        value = value * 8 + (d - b'0') as u32;
                                    }
                                    _ => break,
                                }
                            }
                            raw.push(value as u8);
                        }
                        b'\n' | b'\r' => {}
                        other => raw.push(other),
                    }
                }
                other => raw.push(other),
            }
        }
        Some(Engine::String(decode_engine(&raw)))
    }

    fn hex(&mut self) -> Option<Engine> {
        self.take(b"<");
        let mut nibbles = Vec::new();
        while let Some(b) = self.peek() {
            if b == b'>' {
                break;
            }
            self.index += 1;
            if let Some(n) = (b as char).to_digit(16) {
                nibbles.push(n as u8);
            }
        }
        if !self.take(b">") {
            return None;
        }
        let raw: Vec<u8> = nibbles.chunks_exact(2).map(|p| p[0] << 4 | p[1]).collect();
        Some(Engine::String(decode_engine(&raw)))
    }
}

/// Engine strings are UTF-16 with a byte-order mark, or Latin-1.
fn decode_engine(raw: &[u8]) -> String {
    if raw.len() >= 2 && raw[0] == 0xFE && raw[1] == 0xFF {
        let units: Vec<u16> = raw[2..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    raw.iter().map(|b| *b as char).collect()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A `TySh` block: identity-scaled point text at (40, 100) reading "Hello", 24 pt, red.
    pub fn type_block(text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1u16.to_be_bytes());
        for v in [1.0f64, 0.0, 0.0, 1.0, 40.0, 100.0] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(&50u16.to_be_bytes());
        out.extend_from_slice(&16u32.to_be_bytes());
        // Descriptor: name, class, one item.
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(b"TxLr");
        out.extend_from_slice(&2u32.to_be_bytes());
        let key = |out: &mut Vec<u8>, k: &str| {
            out.extend_from_slice(&(k.len() as u32).to_be_bytes());
            out.extend_from_slice(k.as_bytes());
        };
        key(&mut out, "Txt ");
        out.extend_from_slice(b"TEXT");
        let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
        out.extend_from_slice(&(units.len() as u32).to_be_bytes());
        for u in units {
            out.extend_from_slice(&u.to_be_bytes());
        }
        key(&mut out, "EngineData");
        out.extend_from_slice(b"tdta");
        let engine = b"\n\n<<\n\t/EngineDict\n\t<<\n\t\t/StyleRun\n\t\t<<\n\t\t\t/RunArray [\n\t\t\t<<\n\t\t\t\t/StyleSheet\n\t\t\t\t<<\n\t\t\t\t\t/StyleSheetData\n\t\t\t\t\t<<\n\t\t\t\t\t\t/Font 0\n\t\t\t\t\t\t/FontSize 24.0\n\t\t\t\t\t\t/FillColor << /Type 1 /Values [ 1.0 1.0 0.0 0.0 ] >>\n\t\t\t\t\t>>\n\t\t\t\t>>\n\t\t\t>>\n\t\t\t]\n\t\t>>\n\t\t/ParagraphRun << /RunArray [ << /ParagraphSheet << /Properties << /Justification 2 >> >> >> ] >>\n\t>>\n\t/ResourceDict << /FontSet [ << /Name (\xfe\xff\x00A\x00r\x00i\x00a\x00l\x00M\x00T) >> ] >>\n>>";
        out.extend_from_slice(&(engine.len() as u32).to_be_bytes());
        out.extend_from_slice(engine);
        out
    }

    #[test]
    fn reads_type_layers() {
        let mut extra = HashMap::new();
        extra.insert("TySh".to_string(), type_block("Hello\rworld"));
        let source = parse(&extra).unwrap();
        assert_eq!(source.style.content, "Hello\nworld");
        assert_eq!(source.style.font_name, "ArialMT");
        assert_eq!(source.style.font_size, 24.0);
        assert_eq!(source.style.color(), [1.0, 0.0, 0.0]);
        assert_eq!(source.style.alignment, Alignment::Center);
        assert_eq!(source.anchor, (40.0, 100.0));
        assert!(!source.anchor_is_frame);
    }
}
