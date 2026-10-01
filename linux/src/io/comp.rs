//! Reading and writing `.comp` projects: a folder holding `manifest.json` and `images/<UUID>.png`
//! (plus `<UUID>.mask.png` masks). The format is shared with the macOS app; see
//! docs/project-format.md.

use std::fs;
use std::io::{BufWriter, Cursor};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{DynamicImage, GrayImage, ImageEncoder, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::adjust::Adjustment;
use crate::doc::{BlendMode, Document, Guide, Id, Layer, LayerMask, LayerTransform, MaskPixels, RawRecord};

pub const FORMAT: &str = "com.compositor.project";
pub const CURRENT_VERSION: i64 = 11;

const MAX_SIDE: u32 = 30_000;
const MAX_PIXELS: u64 = 100_000_000;
const MAX_LAYERS: usize = 10_000;
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format: String,
    version: i64,
    #[serde(default = "srgb")]
    color_space: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolution: Option<f64>,
    #[serde(rename = "documentID")]
    document_id: Id,
    width: u32,
    height: u32,
    #[serde(rename = "activeLayerID", default)]
    active_layer_id: Option<Id>,
    layers: Vec<LayerRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guides: Option<Vec<Guide>>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

fn srgb() -> String {
    "sRGB".into()
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayerRecord {
    id: Id,
    name: String,
    is_visible: bool,
    transform: LayerTransform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image_file: Option<String>,
    #[serde(rename = "parentID", default, skip_serializing_if = "Option::is_none")]
    parent_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    is_group: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blend_mode: Option<BlendMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mask_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mask_enabled: Option<bool>,
    #[serde(rename = "maskSourceID", default, skip_serializing_if = "Option::is_none")]
    mask_source_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adjustment: Option<Adjustment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mask_placement: Option<LayerTransform>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mask_linked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shape: Option<RawRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effects: Option<crate::effects::LayerEffects>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<crate::text::TextStyle>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// The package folder for `path`: the `.comp` folder itself, or the one holding a chosen
/// `manifest.json`.
pub fn package_root(path: &Path) -> PathBuf {
    if path.file_name().is_some_and(|n| n == "manifest.json") {
        path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

pub fn is_project(path: &Path) -> bool {
    let root = package_root(path);
    root.join("manifest.json").is_file()
}

pub fn load(path: &Path) -> Result<Document> {
    let root = package_root(path);
    let manifest_path = root.join("manifest.json");
    let size = fs::metadata(&manifest_path).context("This folder is not a Compositor project")?.len();
    if size > MAX_MANIFEST_BYTES {
        bail!("The project manifest is too large.");
    }
    let bytes = fs::read(&manifest_path)?;
    let header: Value = serde_json::from_slice(&bytes).context("The project manifest is damaged.")?;
    if header.get("format").and_then(Value::as_str) != Some(FORMAT) {
        bail!("This is not a valid Compositor project, or its metadata is damaged.");
    }
    let version = header.get("version").and_then(Value::as_i64).unwrap_or(0);
    if !(1..=CURRENT_VERSION).contains(&version) {
        bail!("This project uses format version {version}. This app supports versions 1–{CURRENT_VERSION}.");
    }
    let manifest: Manifest =
        serde_json::from_value(header).context("This is not a valid Compositor project, or its metadata is damaged.")?;
    validate(&manifest)?;

    let images_dir = root.join("images");
    let canonical_images = images_dir.canonicalize().ok();
    let load_asset = |file: &str| -> Result<DynamicImage> {
        if file.contains('/') || file.contains('\\') || file.starts_with('.') {
            bail!("Unsafe image path in project.");
        }
        let path = images_dir.join(file);
        let meta = fs::symlink_metadata(&path).with_context(|| format!("An image inside the project is missing: {file}"))?;
        if !meta.is_file() || meta.len() > MAX_ASSET_BYTES {
            bail!("An image inside the project is missing or damaged.");
        }
        if let (Some(dir), Ok(real)) = (&canonical_images, path.canonicalize()) {
            if !real.starts_with(dir) {
                bail!("Unsafe image path in project.");
            }
        }
        let bytes = fs::read(&path)?;
        image::load_from_memory(&bytes).with_context(|| format!("An image inside the project is damaged: {file}"))
    };

    let decoded: Vec<Result<(Option<RgbaImage>, Option<GrayImage>)>> = manifest
        .layers
        .par_iter()
        .map(|record| {
            let image = match &record.image_file {
                Some(file) => Some(load_asset(file)?.to_rgba8()),
                None => None,
            };
            let mask = match &record.mask_file {
                Some(file) => Some(load_asset(file)?.to_luma8()),
                None => None,
            };
            Ok((image, mask))
        })
        .collect();

    let mut total_pixels = 0u64;
    let mut layers = Vec::with_capacity(manifest.layers.len());
    for (record, assets) in manifest.layers.into_iter().zip(decoded) {
        let (image, mask) = assets?;
        if let Some(image) = &image {
            check_size(image.width(), image.height())?;
            total_pixels += image.width() as u64 * image.height() as u64;
        }
        let mask = mask.map(|gray| {
            let pixels = if gray.width() == 1 && gray.height() == 1 {
                MaskPixels::Uniform(gray.get_pixel(0, 0)[0])
            } else {
                MaskPixels::Pixels(Arc::new(gray))
            };
            LayerMask {
                pixels,
                enabled: record.mask_enabled.unwrap_or(true),
                linked: record.mask_linked.unwrap_or(true),
                placement: record.mask_placement,
            }
        });
        layers.push(Layer {
            id: record.id,
            name: record.name,
            visible: record.is_visible,
            transform: record.transform,
            image: image.map(Arc::new),
            parent: record.parent_id,
            is_group: record.is_group.unwrap_or(false),
            opacity: record.opacity.unwrap_or(1.0).clamp(0.0, 1.0) as f32,
            blend: record.blend_mode.unwrap_or_default(),
            mask,
            clip_source: record.mask_source_id,
            adjustment: record.adjustment,
            effects: record.effects,
            text: record.text,
            shape: record.shape,
            expanded: true,
            extra: record.extra,
        });
    }
    if total_pixels > MAX_PIXELS * 4 {
        bail!("This project exceeds the supported document size.");
    }

    let mut doc = Document {
        id: manifest.document_id,
        width: manifest.width,
        height: manifest.height,
        resolution: manifest.resolution.unwrap_or(72.0),
        layers,
        active: manifest.active_layer_id,
        selected: Vec::new(),
        guides: manifest.guides.unwrap_or_default(),
        selection: None,
        extra: manifest.extra,
    };
    if doc.active.is_none() {
        doc.active = doc.layers.last().map(|l| l.id);
    }
    doc.normalize_order();
    Ok(doc)
}

fn check_size(w: u32, h: u32) -> Result<()> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || w as u64 * h as u64 > MAX_PIXELS {
        bail!("This project exceeds the supported canvas or image size.");
    }
    Ok(())
}

fn validate(m: &Manifest) -> Result<()> {
    check_size(m.width, m.height)?;
    if m.layers.len() > MAX_LAYERS {
        bail!("This project has too many layers.");
    }
    if let Some(r) = m.resolution {
        if !(1.0..=9600.0).contains(&r) {
            bail!("Invalid resolution.");
        }
    }
    let mut ids = std::collections::HashSet::new();
    for layer in &m.layers {
        if !ids.insert(layer.id) {
            bail!("Duplicate layer ID {}.", layer.id);
        }
    }
    for layer in &m.layers {
        if !layer.transform.is_valid() {
            bail!("Layer “{}” has an invalid transform.", layer.name);
        }
        let upper = layer.id.upper();
        if let Some(file) = &layer.image_file {
            if !file.eq_ignore_ascii_case(&format!("{upper}.png")) {
                bail!("Layer “{}” names an image that isn't its own.", layer.name);
            }
            if layer.is_group == Some(true) {
                bail!("A folder can't have an image.");
            }
        }
        if let Some(file) = &layer.mask_file {
            if !file.eq_ignore_ascii_case(&format!("{upper}.mask.png")) {
                bail!("Layer “{}” names a mask that isn't its own.", layer.name);
            }
        }
        if let Some(parent) = layer.parent_id {
            let Some(p) = m.layers.iter().find(|l| l.id == parent) else {
                bail!("Layer “{}” is in a folder that doesn't exist.", layer.name);
            };
            if p.is_group != Some(true) {
                bail!("Layer “{}” is inside something that isn't a folder.", layer.name);
            }
        }
        if let Some(opacity) = layer.opacity {
            if !(0.0..=1.0).contains(&opacity) {
                bail!("Layer “{}” has an invalid opacity.", layer.name);
            }
        }
        if let Some(source) = layer.mask_source_id {
            if source == layer.id || !ids.contains(&source) {
                bail!("Layer “{}” has an invalid clipping mask.", layer.name);
            }
        }
    }
    // Cycles in folders.
    for layer in &m.layers {
        let mut current = layer.parent_id;
        let mut depth = 0;
        while let Some(p) = current {
            depth += 1;
            if depth > 64 || p == layer.id {
                bail!("Folders are nested too deeply or in a loop.");
            }
            current = m.layers.iter().find(|l| l.id == p).and_then(|l| l.parent_id);
        }
    }
    Ok(())
}

fn encode_png_rgba(image: &RgbaImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(Cursor::new(&mut out), CompressionType::Fast, FilterType::Adaptive).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(out)
}

fn encode_png_gray(image: &GrayImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(Cursor::new(&mut out), CompressionType::Fast, FilterType::Adaptive).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::L8,
    )?;
    Ok(out)
}

/// Writes the project to `path` atomically: the new package is written beside the old one and
/// swapped in only once everything has been encoded and written.
pub fn save(doc: &Document, path: &Path) -> Result<()> {
    let root = package_root(path);
    let parent = root.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = root.file_name().context("Invalid project path")?.to_string_lossy().to_string();
    let staging = parent.join(format!(".{name}.saving-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(staging.join("images"))?;
    let result = write_package(doc, &staging);
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if root.exists() {
        let backup = parent.join(format!(".{name}.previous-{}", std::process::id()));
        if backup.exists() {
            fs::remove_dir_all(&backup)?;
        }
        fs::rename(&root, &backup).context("Couldn't replace the existing project")?;
        if let Err(error) = fs::rename(&staging, &root) {
            let _ = fs::rename(&backup, &root);
            return Err(error.into());
        }
        let _ = fs::remove_dir_all(&backup);
    } else {
        fs::rename(&staging, &root)?;
    }
    Ok(())
}

fn write_package(doc: &Document, dir: &Path) -> Result<()> {
    let images = dir.join("images");
    let records: Vec<LayerRecord> = doc.layers.iter().map(record_for).collect();

    let jobs: Vec<(String, Asset)> = doc
        .layers
        .iter()
        .flat_map(|layer| {
            let mut jobs = Vec::new();
            if let Some(image) = &layer.image {
                jobs.push((format!("{}.png", layer.id.upper()), Asset::Rgba(image.clone())));
            }
            if let Some(mask) = &layer.mask {
                jobs.push((format!("{}.mask.png", layer.id.upper()), Asset::Mask(mask.pixels.clone())));
            }
            jobs
        })
        .collect();
    jobs.par_iter().try_for_each(|(file, asset)| -> Result<()> {
        let bytes = match asset {
            Asset::Rgba(image) => encode_png_rgba(image)?,
            Asset::Mask(MaskPixels::Pixels(gray)) => encode_png_gray(gray)?,
            Asset::Mask(MaskPixels::Uniform(v)) => encode_png_gray(&GrayImage::from_pixel(1, 1, image::Luma([*v])))?,
        };
        fs::write(images.join(file), bytes).with_context(|| format!("An image could not be saved: {file}"))
    })?;

    let manifest = Manifest {
        format: FORMAT.into(),
        version: CURRENT_VERSION,
        color_space: "sRGB".into(),
        resolution: Some(doc.resolution),
        document_id: doc.id,
        width: doc.width,
        height: doc.height,
        active_layer_id: doc.active,
        layers: records,
        guides: (!doc.guides.is_empty()).then(|| doc.guides.clone()),
        extra: doc.extra.clone(),
    };
    let file = fs::File::create(dir.join("manifest.json"))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &manifest)?;
    use std::io::Write;
    writer.flush()?;
    Ok(())
}

enum Asset {
    Rgba(Arc<RgbaImage>),
    Mask(MaskPixels),
}

fn record_for(layer: &Layer) -> LayerRecord {
    let upper = layer.id.upper();
    LayerRecord {
        id: layer.id,
        name: layer.name.clone(),
        is_visible: layer.visible,
        transform: layer.transform,
        image_file: layer.image.as_ref().map(|_| format!("{upper}.png")),
        parent_id: layer.parent,
        is_group: Some(layer.is_group),
        opacity: Some(layer.opacity as f64),
        blend_mode: Some(if layer.is_group { BlendMode::Normal } else { layer.blend }),
        mask_file: layer.mask.as_ref().map(|_| format!("{upper}.mask.png")),
        mask_enabled: layer.mask.as_ref().map(|m| m.enabled),
        mask_source_id: layer.clip_source,
        adjustment: layer.adjustment.clone(),
        mask_placement: layer.mask.as_ref().filter(|m| !m.linked).and_then(|m| m.placement),
        mask_linked: layer.mask.as_ref().filter(|m| !m.linked).map(|_| false),
        shape: layer.shape.clone(),
        effects: layer.effects.clone(),
        text: layer.text.clone(),
        extra: layer.extra.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("comp-test-{}", Id::new()));
        let path = dir.join("Test.comp");
        let mut doc = Document::new(64, 32, Some([10, 20, 30, 255]));
        let mut group = Layer::new_group("Folder", doc.full_canvas_transform());
        group.opacity = 0.5;
        let group_id = group.id;
        doc.layers.push(group);
        let mut child = Layer::new_pixel("Child", RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 128])), LayerTransform::rect(4.0, 4.0, 8.0, 8.0));
        child.parent = Some(group_id);
        child.blend = BlendMode::LinearDodge;
        child.mask = Some(LayerMask { pixels: MaskPixels::Uniform(200), enabled: true, linked: true, placement: None });
        child.extra.insert("futureField".into(), Value::from(42));
        doc.layers.push(child);
        doc.normalize_order();
        save(&doc, &path).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.layers.len(), 3);
        let child = loaded.layers.iter().find(|l| l.name == "Child").unwrap();
        assert_eq!(child.blend, BlendMode::LinearDodge);
        assert_eq!(child.parent, Some(group_id));
        assert_eq!(child.extra.get("futureField"), Some(&Value::from(42)));
        assert!(matches!(child.mask.as_ref().unwrap().pixels, MaskPixels::Uniform(200)));
        let text = fs::read_to_string(path.join("manifest.json")).unwrap();
        assert!(text.contains(&format!("\"{}.png\"", child.id.upper())));
        save(&loaded, &path).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_documented_example() {
        let dir = std::env::temp_dir().join(format!("comp-example-{}", Id::new()));
        let root = dir.join("Example.comp");
        fs::create_dir_all(root.join("images")).unwrap();
        let id = "6F1D3C2A-0B7E-4E8A-9C4D-2A1B3C4D5E6F";
        RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255])).save(root.join(format!("images/{id}.png"))).unwrap();
        let manifest = format!(
            r#"{{"format":"com.compositor.project","version":11,"colorSpace":"sRGB","documentID":"0C5E7A91-3B2D-4F6A-8E1C-9D0B7A6F5E4D","width":4,"height":4,"resolution":72,"activeLayerID":"{id}","layers":[{{"id":"{id}","name":"Background","imageFile":"{id}.png","isVisible":true,"isGroup":false,"opacity":1,"blendMode":"Normal","transform":{{"origin":[0,0],"size":[4,4],"rotation":0,"flipX":false,"flipY":false,"sampling":"High quality"}},"adjustment":null}}]}}"#
        );
        fs::write(root.join("manifest.json"), manifest).unwrap();
        let doc = load(&root).unwrap();
        assert_eq!(doc.layers[0].image.as_ref().unwrap().get_pixel(0, 0).0, [1, 2, 3, 255]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
