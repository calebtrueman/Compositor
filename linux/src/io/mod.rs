pub mod comp;
pub mod psd;
pub mod watch;

use std::fs;
use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::{ImageEncoder, RgbaImage};

use crate::doc::Document;
use crate::render::{self, Region, RenderCache};

pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "webp", "bmp", "gif"];
pub const PSD_EXTENSIONS: &[&str] = &["psd", "psb"];

pub fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn is_importable_image(path: &Path) -> bool {
    let ext = extension(path);
    IMAGE_EXTENSIONS.contains(&ext.as_str()) || PSD_EXTENSIONS.contains(&ext.as_str())
}

/// Decodes an image file to straight-alpha RGBA, honoring EXIF orientation for JPEGs.
pub fn read_image(path: &Path) -> Result<RgbaImage> {
    let bytes = fs::read(path).with_context(|| format!("Couldn't read {}", path.display()))?;
    let mut decoder = image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format()?.into_decoder()?;
    use image::ImageDecoder;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = image::DynamicImage::from_decoder(decoder).context("This image couldn't be decoded")?;
    image.apply_orientation(orientation);
    Ok(image.to_rgba8())
}

/// Opens any supported file as a document: a project, a Photoshop file or a plain image.
pub fn open_document(path: &Path) -> Result<Document> {
    if comp::is_project(path) {
        return comp::load(path);
    }
    let ext = extension(path);
    if PSD_EXTENSIONS.contains(&ext.as_str()) {
        return psd::load(path);
    }
    let image = read_image(path)?;
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Background".into());
    Ok(Document::from_image(image, &name))
}

pub fn flatten(doc: &Document) -> RgbaImage {
    let cache = RenderCache::default();
    render::composite(doc, Region::full(doc), &cache).to_rgba8()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Png,
    Jpeg,
}

impl ExportFormat {
    pub fn for_path(path: &Path) -> ExportFormat {
        match extension(path).as_str() {
            "jpg" | "jpeg" => ExportFormat::Jpeg,
            _ => ExportFormat::Png,
        }
    }
}

/// Encodes a flattened image. JPEG has no alpha, so it's flattened onto white first.
pub fn encode(image: &RgbaImage, format: ExportFormat, quality: u8) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match format {
        ExportFormat::Png => {
            image::codecs::png::PngEncoder::new(Cursor::new(&mut out)).write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        ExportFormat::Jpeg => {
            let rgb = on_white(image);
            JpegEncoder::new_with_quality(Cursor::new(&mut out), quality.clamp(1, 100)).write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )?;
        }
    }
    Ok(out)
}

pub fn on_white(image: &RgbaImage) -> image::RgbImage {
    let mut rgb = image::RgbImage::new(image.width(), image.height());
    for (o, p) in rgb.pixels_mut().zip(image.pixels()) {
        let a = p[3] as u32;
        for i in 0..3 {
            o[i] = ((p[i] as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        }
    }
    rgb
}

pub fn export(doc: &Document, path: &Path, quality: u8) -> Result<()> {
    let flat = flatten(doc);
    let bytes = encode(&flat, ExportFormat::for_path(path), quality)?;
    fs::write(path, bytes).with_context(|| format!("Couldn't write {}", path.display()))
}
