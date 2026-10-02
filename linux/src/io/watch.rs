//! Noticing when an open project changes on disk (another program or an AI agent writing it), as
//! the macOS app does. Changes are detected from the manifest's contents and each image's name and
//! size, not from modification times.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;

/// A fingerprint of the project on disk, or `None` if it can't be read right now.
pub fn digest(root: &Path) -> Option<u64> {
    let manifest = fs::read(root.join("manifest.json")).ok()?;
    let mut hasher = DefaultHasher::new();
    manifest.hash(&mut hasher);
    let mut entries: Vec<(String, u64)> = fs::read_dir(root.join("images"))
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| Some((e.file_name().to_string_lossy().to_string(), e.metadata().ok()?.len())))
        .collect();
    entries.sort();
    entries.hash(&mut hasher);
    Some(hasher.finish())
}
