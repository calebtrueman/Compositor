//! Photoshop import. Layered 8-bit RGB `.psd` and `.psb` files open as layers: folders with their
//! opacity and visibility, opacity (with fill opacity folded in), blend modes, user masks,
//! clipping masks and positions. Simple horizontal type stays editable. Effects, smart objects
//! and vector shapes come in as their pixels; adjustment layers other than Invert are skipped.
//! When the layers can't be read, the merged image Photoshop saves alongside them opens instead.

mod reader;
mod text;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use image::{GrayImage, RgbaImage};

use crate::doc::{BlendMode, Document, Id, Layer, LayerMask, LayerTransform, MaskPixels};
use reader::{Header, RawLayer};

pub fn load(path: &Path) -> Result<Document> {
    let bytes = std::fs::read(path).with_context(|| format!("Couldn't read {}", path.display()))?;
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Background".into());
    read(&bytes, &name)
}

/// Opens a Photoshop file's bytes; `name` names the layer when only the merged image is used.
pub fn read(data: &[u8], name: &str) -> Result<Document> {
    let header = reader::header(data)?;
    let layered = reader::layers(data, &header).and_then(|raw| build(&header, raw));
    match layered {
        Ok(Some(doc)) => Ok(doc),
        Ok(None) | Err(_) => {
            let alpha = reader::composite_has_alpha(data, &header);
            let image = match reader::composite(data, &header, alpha) {
                Ok(image) => image,
                // Without a merged image, the reason the layers failed is the one to give.
                Err(error) => return Err(layered.err().unwrap_or(error)),
            };
            let mut doc = Document::from_image(image, name);
            doc.resolution = header.resolution;
            Ok(doc)
        }
    }
}

/// Photoshop's blend mode keys. Dissolve, Darker Color and Lighter Color have no equivalent and
/// come in as Normal, as in the macOS app.
pub fn blend_mode(key: &str) -> BlendMode {
    match key {
        "mul " => BlendMode::Multiply,
        "scrn" => BlendMode::Screen,
        "over" => BlendMode::Overlay,
        "sLit" => BlendMode::SoftLight,
        "hLit" => BlendMode::HardLight,
        "vLit" => BlendMode::VividLight,
        "lLit" => BlendMode::LinearLight,
        "pLit" => BlendMode::PinLight,
        "hMix" => BlendMode::HardMix,
        "dark" => BlendMode::Darken,
        "lite" => BlendMode::Lighten,
        "diff" => BlendMode::Difference,
        "smud" => BlendMode::Exclusion,
        "fsub" => BlendMode::Subtract,
        "fdiv" => BlendMode::Divide,
        "div " => BlendMode::ColorDodge,
        "idiv" => BlendMode::ColorBurn,
        "lbrn" => BlendMode::LinearBurn,
        "lddg" => BlendMode::LinearDodge,
        "hue " => BlendMode::Hue,
        "sat " => BlendMode::Saturation,
        "colr" => BlendMode::Color,
        "lum " => BlendMode::Luminosity,
        _ => BlendMode::Normal,
    }
}

const ADJUSTMENT_KEYS: &[&str] =
    &["levl", "curv", "hue2", "hue ", "expA", "grdm", "brit", "blnc", "nvrt", "thrs", "post", "mixr", "selc", "blwh", "phfl", "vibA"];
const EFFECT_KEYS: &[&str] = &["lfx2", "lrFX", "lmfx"];

/// The document for the layer records, or `None` when nothing usable is in them.
fn build(header: &Header, raw: Vec<RawLayer>) -> Result<Option<Document>> {
    let (width, height) = (header.width, header.height);
    let canvas = LayerTransform::rect(0.0, 0.0, width as f64, height as f64);
    let mut layers: Vec<(Layer, bool)> = Vec::new();
    // Folders open at their end marker (type 3) and close at their own record, which follows
    // their contents.
    let mut open: Vec<Id> = Vec::new();
    for record in raw {
        if record.section == Some(3) {
            open.push(Id::new());
            continue;
        }
        let is_group = matches!(record.section, Some(1 | 2));
        let id = if is_group { open.pop().unwrap_or_default() } else { Id::new() };
        let name = if record.name.is_empty() { "Layer".to_string() } else { record.name.clone() };
        let effects = record.extra.keys().any(|k| EFFECT_KEYS.contains(&k.as_str()));
        let opacity = if effects && record.fill != 255 {
            record.opacity as f32 / 255.0
        } else {
            record.opacity as f32 / 255.0 * record.fill as f32 / 255.0
        };
        let adjustment = !is_group && record.extra.keys().any(|k| ADJUSTMENT_KEYS.contains(&k.as_str()));
        let mut layer = if is_group {
            Layer::new_group(name, canvas)
        } else if adjustment {
            // Only Invert has no settings to carry over; the others would change the look
            // without matching Photoshop, so they're left out as in the macOS app.
            if !record.extra.contains_key("nvrt") {
                continue;
            }
            let mut layer = Layer::blank(name, canvas);
            layer.adjustment = Some(crate::adjust::Adjustment::new(crate::adjust::AdjustmentKind::Invert));
            layer
        } else if let Some(layer) = text_layer(&name, &record) {
            layer
        } else if let Some(image) = record.image.clone() {
            let (w, h) = image.dimensions();
            Layer::new_pixel(name, image, LayerTransform::rect(record.left as f64, record.top as f64, w as f64, h as f64))
        } else {
            Layer::blank(name, canvas)
        };
        layer.id = id;
        layer.parent = open.last().copied();
        layer.visible = !record.hidden;
        layer.opacity = opacity.clamp(0.0, 1.0);
        layer.blend = if is_group { BlendMode::Normal } else { blend_mode(&record.blend_key) };
        if let Some(mask) = record.mask.as_ref().filter(|m| !m.from_render) {
            let pixels = match &mask.image {
                Some(patch) => MaskPixels::Pixels(Arc::new(mask_on_layer_grid(patch, mask, &layer))),
                None => MaskPixels::Uniform(mask.default),
            };
            layer.mask = Some(LayerMask {
                pixels,
                enabled: !mask.disabled,
                linked: mask.linked,
                placement: (!mask.linked).then_some(layer.transform),
            });
        }
        layers.push((layer, record.clipping));
    }
    if !open.is_empty() {
        bail!(reader::TRUNCATED);
    }
    if layers.iter().all(|(l, _)| l.is_group) {
        return Ok(None);
    }
    // Clipped layers clip to the nearest unclipped layer below them in the same folder.
    let mut base: HashMap<Option<Id>, Option<Id>> = HashMap::new();
    for (layer, clipped) in &mut layers {
        if *clipped {
            if let Some(Some(source)) = base.get(&layer.parent) {
                layer.clip_source = Some(*source);
            }
        } else if layer.is_group || layer.adjustment.is_some() {
            base.insert(layer.parent, None);
        } else {
            base.insert(layer.parent, Some(layer.id));
        }
    }
    let mut doc = Document::new(width, height, None);
    doc.resolution = header.resolution;
    doc.layers = layers.into_iter().map(|(l, _)| l).collect();
    doc.normalize_order();
    doc.active = doc.layers.iter().rev().find(|l| !l.is_group).or(doc.layers.last()).map(|l| l.id);
    Ok(Some(doc))
}

/// A type layer as editable text, placed where Photoshop drew it. Its pixels stay Photoshop's own
/// when the text isn't turned, so it looks the same until it's edited.
fn text_layer(name: &str, record: &RawLayer) -> Option<Layer> {
    let source = text::parse(&record.extra)?;
    let layout = crate::text::layout::layout(&source.style)?;
    let size = (layout.width as f64, layout.height as f64);
    let image_anchor = if source.anchor_is_frame {
        (crate::text::PADDING, crate::text::PADDING)
    } else {
        let x = match source.style.alignment {
            crate::text::Alignment::Left => crate::text::PADDING,
            crate::text::Alignment::Center => size.0 / 2.0,
            crate::text::Alignment::Right => size.0 - crate::text::PADDING,
        };
        (x, layout.lines.first().map_or(crate::text::PADDING + source.style.font_size, |l| l.baseline as f64))
    };
    let mut transform = text::layer_transform(size, image_anchor, source.anchor, source.rotation, source.flip_y);
    if !transform.is_valid() {
        return None;
    }
    let image = match &record.image {
        Some(pixels) if source.rotation == 0.0 && !source.flip_y => {
            // Photoshop's pixels on a layer-sized image, grown to the right and down if they
            // reach past it.
            transform.origin = [transform.origin[0].round(), transform.origin[1].round()];
            let dx = record.left - transform.origin[0] as i64;
            let dy = record.top - transform.origin[1] as i64;
            let w = (layout.width as i64).max(dx + pixels.width() as i64).clamp(1, reader::MAX_SIDE as i64) as u32;
            let h = (layout.height as i64).max(dy + pixels.height() as i64).clamp(1, reader::MAX_SIDE as i64) as u32;
            let mut image = RgbaImage::new(w, h);
            image::imageops::replace(&mut image, pixels, dx, dy);
            transform.size = [w as f64, h as f64];
            image
        }
        _ => layout.draw(),
    };
    let mut layer = Layer::new_pixel(name, image, transform);
    layer.text = Some(source.style);
    Some(layer)
}

/// A Photoshop mask on the layer's own pixel grid, as Compositor's masks are: the stored patch
/// where it sits on the document, and Photoshop's default value everywhere else.
fn mask_on_layer_grid(patch: &GrayImage, mask: &reader::MaskInfo, layer: &Layer) -> GrayImage {
    let (w, h) = layer.pixel_size();
    let to_doc = layer.transform.pixel_to_document(w, h);
    if to_doc == LayerTransform::rect(mask.left as f64, mask.top as f64, w as f64, h as f64).pixel_to_document(w, h)
        && patch.dimensions() == (w, h)
    {
        return patch.clone();
    }
    let mut out = GrayImage::from_pixel(w, h, image::Luma([mask.default]));
    for (x, y, p) in out.enumerate_pixels_mut() {
        let (dx, dy) = to_doc.apply(x as f64 + 0.5, y as f64 + 0.5);
        let (mx, my) = ((dx - mask.left as f64).floor(), (dy - mask.top as f64).floor());
        if mx >= 0.0 && my >= 0.0 && (mx as u32) < patch.width() && (my as u32) < patch.height() {
            *p = *patch.get_pixel(mx as u32, my as u32);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One layer record for `psd`: rect is (top, left, bottom, right), pixels one RGBA color.
    struct Spec {
        name: &'static str,
        rect: (i32, i32, i32, i32),
        pixels: [u8; 4],
        mask: Option<((i32, i32, i32, i32), u8, Vec<u8>)>,
        section: Option<u32>,
        blend: &'static [u8; 4],
        opacity: u8,
        clipping: bool,
        hidden: bool,
        extra: Vec<(&'static [u8; 4], Vec<u8>)>,
    }

    fn spec(name: &'static str, rect: (i32, i32, i32, i32), pixels: [u8; 4]) -> Spec {
        Spec { name, rect, pixels, mask: None, section: None, blend: b"norm", opacity: 255, clipping: false, hidden: false, extra: Vec::new() }
    }

    /// A small Photoshop file with `specs` as its layers, bottom to top, and a gray merged image.
    fn psd(large: bool, specs: &[Spec], width: u32, height: u32) -> Vec<u8> {
        let length = |out: &mut Vec<u8>, v: usize, wide: bool| {
            if wide {
                out.extend_from_slice(&(v as u64).to_be_bytes());
            } else {
                out.extend_from_slice(&(v as u32).to_be_bytes());
            }
        };
        let mut out = Vec::new();
        out.extend_from_slice(b"8BPS");
        out.extend_from_slice(&(if large { 2u16 } else { 1 }).to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        out.extend_from_slice(&3u16.to_be_bytes());
        out.extend_from_slice(&height.to_be_bytes());
        out.extend_from_slice(&width.to_be_bytes());
        out.extend_from_slice(&8u16.to_be_bytes());
        out.extend_from_slice(&3u16.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        // Layer info: records, then channel data.
        let mut records = Vec::new();
        let mut channels = Vec::new();
        for s in specs {
            let (top, left, bottom, right) = s.rect;
            for v in [top, left, bottom, right] {
                records.extend_from_slice(&v.to_be_bytes());
            }
            let n = ((bottom - top) * (right - left)) as usize;
            let mut ids: Vec<(i16, Vec<u8>)> = vec![(-1, vec![s.pixels[3]; n]), (0, vec![s.pixels[0]; n]), (1, vec![s.pixels[1]; n]), (2, vec![s.pixels[2]; n])];
            if let Some((_, _, gray)) = &s.mask {
                ids.push((-2, gray.clone()));
            }
            records.extend_from_slice(&(ids.len() as u16).to_be_bytes());
            for (id, data) in &ids {
                records.extend_from_slice(&id.to_be_bytes());
                length(&mut records, data.len() + 2, large);
                channels.extend_from_slice(&0u16.to_be_bytes());
                channels.extend_from_slice(data);
            }
            records.extend_from_slice(b"8BIM");
            records.extend_from_slice(s.blend);
            records.push(s.opacity);
            records.push(s.clipping as u8);
            records.push(if s.hidden { 2 } else { 0 });
            records.push(0);
            let mut extra = Vec::new();
            match &s.mask {
                Some(((t, l, b, r), default, _)) => {
                    extra.extend_from_slice(&20u32.to_be_bytes());
                    for v in [*t, *l, *b, *r] {
                        extra.extend_from_slice(&v.to_be_bytes());
                    }
                    extra.push(*default);
                    extra.push(0);
                    extra.extend_from_slice(&[0, 0]);
                }
                None => extra.extend_from_slice(&0u32.to_be_bytes()),
            }
            extra.extend_from_slice(&0u32.to_be_bytes());
            let name = s.name.as_bytes();
            extra.push(name.len() as u8);
            extra.extend_from_slice(name);
            let pad = (4 - (name.len() + 1) % 4) % 4;
            extra.extend(std::iter::repeat_n(0, pad));
            let mut blocks: Vec<(&[u8; 4], Vec<u8>)> = s.extra.iter().map(|(k, v)| (*k, v.clone())).collect();
            if let Some(section) = s.section {
                blocks.push((b"lsct", section.to_be_bytes().to_vec()));
            }
            for (key, data) in blocks {
                extra.extend_from_slice(b"8BIM");
                extra.extend_from_slice(key);
                extra.extend_from_slice(&(data.len() as u32).to_be_bytes());
                extra.extend_from_slice(&data);
                if data.len() % 2 == 1 {
                    extra.push(0);
                }
            }
            records.extend_from_slice(&(extra.len() as u32).to_be_bytes());
            records.extend_from_slice(&extra);
        }
        let mut info = Vec::new();
        info.extend_from_slice(&(specs.len() as i16).to_be_bytes());
        info.extend_from_slice(&records);
        info.extend_from_slice(&channels);
        let mut section = Vec::new();
        length(&mut section, info.len(), large);
        section.extend_from_slice(&info);
        section.extend_from_slice(&0u32.to_be_bytes());
        length(&mut out, section.len(), large);
        out.extend_from_slice(&section);
        // Merged image, raw: gray.
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend(std::iter::repeat_n(128u8, (width * height * 3) as usize));
        out
    }

    fn sample(large: bool) -> Vec<u8> {
        let background = spec("Background", (0, 0, 4, 4), [0, 0, 255, 255]);
        let clipped = Spec { blend: b"mul ", clipping: true, ..spec("Clipped", (0, 0, 2, 2), [255, 0, 0, 255]) };
        let end = Spec { section: Some(3), ..spec("</Layer group>", (0, 0, 0, 0), [0; 4]) };
        let child = Spec {
            mask: Some(((0, 0, 2, 2), 0, vec![10, 20, 30, 40])),
            blend: b"scrn",
            opacity: 128,
            ..spec("Child", (1, 1, 3, 3), [0, 255, 0, 255])
        };
        let group = Spec { section: Some(1), opacity: 64, hidden: true, ..spec("Folder", (0, 0, 0, 0), [0; 4]) };
        psd(large, &[background, clipped, end, child, group], 4, 4)
    }

    #[test]
    fn imports_layers_groups_masks_and_clipping() {
        for large in [false, true] {
            let doc = read(&sample(large), "Sample").unwrap();
            assert_eq!(doc.layers.len(), 4, "large: {large}");
            let find = |name: &str| doc.layers.iter().find(|l| l.name == name).unwrap();
            let background = find("Background");
            assert_eq!(background.image.as_ref().unwrap().get_pixel(0, 0).0, [0, 0, 255, 255]);
            let clipped = find("Clipped");
            assert_eq!(clipped.clip_source, Some(background.id));
            assert_eq!(clipped.blend, BlendMode::Multiply);
            let folder = find("Folder");
            assert!(folder.is_group && !folder.visible);
            assert!((folder.opacity - 64.0 / 255.0).abs() < 1e-6);
            let child = find("Child");
            assert_eq!(child.parent, Some(folder.id));
            assert_eq!(child.blend, BlendMode::Screen);
            assert_eq!(child.transform, LayerTransform::rect(1.0, 1.0, 2.0, 2.0));
            assert!((child.opacity - 128.0 / 255.0).abs() < 1e-6);
            // The mask covers document (0,0)–(2,2); the layer starts at (1,1), so only its first
            // pixel is inside, taking the patch's last value. The rest is the default, 0.
            let mask = child.mask.as_ref().unwrap();
            let MaskPixels::Pixels(gray) = &mask.pixels else { panic!("expected mask pixels") };
            assert_eq!(gray.as_raw(), &vec![40, 0, 0, 0]);
            // Children are listed before their folder.
            assert!(doc.index_of(child.id).unwrap() < doc.index_of(folder.id).unwrap());
        }
    }

    #[test]
    fn falls_back_to_the_merged_image() {
        let mut bytes = sample(false);
        // Break the first layer's blend signature: the layers can't be read any more.
        let at = bytes.windows(8).position(|w| w == b"8BIMnorm").unwrap();
        bytes[at] = b'X';
        let doc = read(&bytes, "Merged").unwrap();
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].name, "Merged");
        assert_eq!(doc.layers[0].image.as_ref().unwrap().get_pixel(3, 3).0, [128, 128, 128, 255]);
    }

    #[test]
    fn rejects_cmyk_and_deep_files() {
        let mut bytes = sample(false);
        bytes[24..26].copy_from_slice(&4u16.to_be_bytes());
        assert!(read(&bytes, "x").unwrap_err().to_string().contains("CMYK"));
        let mut bytes = sample(false);
        bytes[22..24].copy_from_slice(&16u16.to_be_bytes());
        assert!(read(&bytes, "x").unwrap_err().to_string().contains("16-bit"));
    }

    #[test]
    fn keeps_type_editable() {
        if crate::text::fonts::library().is_empty() {
            return;
        }
        let text = Spec { extra: vec![(b"TySh", text::tests::type_block("Hello"))], ..spec("Hello", (80, 40, 110, 100), [255, 0, 0, 255]) };
        let doc = read(&psd(false, &[spec("Background", (0, 0, 200, 200), [255; 4]), text], 200, 200), "x").unwrap();
        let layer = doc.layers.iter().find(|l| l.name == "Hello").unwrap();
        let style = layer.text.as_ref().unwrap();
        assert_eq!(style.content, "Hello");
        assert_eq!(style.font_size, 24.0);
        // Photoshop's own pixels are kept, at the same place on the document.
        let image = layer.image.as_ref().unwrap();
        let (x, y) = (40 - layer.transform.origin[0] as i64, 80 - layer.transform.origin[1] as i64);
        assert_eq!(image.get_pixel(x as u32, y as u32).0, [255, 0, 0, 255]);
        assert_eq!(layer.transform.size, [image.width() as f64, image.height() as f64]);
    }
}
