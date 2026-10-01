//! Photoshop import. Placeholder: opens the flattened composite as one layer.

use std::path::Path;

use anyhow::{Context, Result};
use image::RgbaImage;

use crate::doc::Document;

pub fn load(path: &Path) -> Result<Document> {
    let bytes = std::fs::read(path)?;
    let psd = psd::Psd::from_bytes(&bytes).map_err(|e| anyhow::anyhow!("{e}")).context("This Photoshop file couldn't be read")?;
    let image = RgbaImage::from_raw(psd.width(), psd.height(), psd.rgba()).context("Unsupported Photoshop file")?;
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Background".into());
    Ok(Document::from_image(image, &name))
}
