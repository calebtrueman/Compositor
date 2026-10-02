//! Reads the parts of a Photoshop file Compositor uses, following Adobe's *Photoshop File Formats
//! Specification*: the header, the resolution resource, layer records with their channels and
//! additional information, and the merged image. Both `.psd` (version 1) and `.psb` (version 2,
//! the large document format, with wider lengths) are read; only 8-bit RGB is accepted.

use std::collections::HashMap;
use std::io::Read;

use anyhow::{bail, Context, Result};
use image::{GrayImage, RgbaImage};

pub const TRUNCATED: &str = "The Photoshop file could not be read. It may be damaged or incomplete.";
const UNSUPPORTED_MODE: &str = "Only 8-bit RGB Photoshop files can be imported. Convert this file to 8 bits per channel RGB in Photoshop first.";

/// Largest canvas and layer, as for projects.
pub const MAX_SIDE: u32 = 30_000;
pub const MAX_PIXELS: u64 = 100_000_000;

pub struct Cursor<'a> {
    data: &'a [u8],
    pub offset: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Cursor { data, offset: 0 }
    }

    pub fn bytes(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self.offset.checked_add(count).filter(|e| *e <= self.data.len()).context(TRUNCATED)?;
        let slice = &self.data[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    pub fn skip(&mut self, count: usize) -> Result<()> {
        self.bytes(count).map(|_| ())
    }

    pub fn seek(&mut self, offset: usize) -> Result<()> {
        if offset > self.data.len() {
            bail!(TRUNCATED);
        }
        self.offset = offset;
        Ok(())
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn u64(&mut self) -> Result<u64> {
        let b = self.bytes(8)?;
        Ok(u64::from_be_bytes(b.try_into().unwrap()))
    }

    /// A length field: 4 bytes, or 8 in a large document where the format widens it.
    pub fn length(&mut self, wide: bool) -> Result<usize> {
        let value = if wide { self.u64()? } else { self.u32()? as u64 };
        usize::try_from(value).ok().filter(|v| *v <= self.data.len()).context(TRUNCATED)
    }

    pub fn tag(&mut self) -> Result<String> {
        Ok(String::from_utf8_lossy(self.bytes(4)?).into_owned())
    }
}

pub struct Header {
    pub width: u32,
    pub height: u32,
    pub channels: u16,
    pub large: bool,
    pub resolution: f64,
    /// Where the layer and mask information section starts.
    pub layer_section: usize,
}

pub fn header(data: &[u8]) -> Result<Header> {
    let mut c = Cursor::new(data);
    if data.len() < 26 || c.tag()? != "8BPS" {
        bail!("This isn't a Photoshop file.");
    }
    let version = c.u16()?;
    if version != 1 && version != 2 {
        bail!("This Photoshop file uses a format version Compositor can’t read.");
    }
    c.skip(6)?;
    let channels = c.u16()?;
    let height = c.u32()?;
    let width = c.u32()?;
    let depth = c.u16()?;
    let mode = c.u16()?;
    if mode != 3 || depth != 8 {
        let what = match (mode, depth) {
            (4, _) => "CMYK".to_string(),
            (_, 16 | 32) => format!("{depth}-bit"),
            (1, _) => "grayscale".into(),
            (2, _) => "indexed color".into(),
            (0, _) => "bitmap".into(),
            (9, _) => "Lab".into(),
            _ => "this color mode".into(),
        };
        bail!("This Photoshop file is {what}. {UNSUPPORTED_MODE}");
    }
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || width as u64 * height as u64 > MAX_PIXELS {
        bail!("This Photoshop file exceeds the supported canvas size.");
    }
    let color_data = c.u32()? as usize;
    c.skip(color_data)?;
    let resources = c.u32()? as usize;
    let resources_end = c.offset.checked_add(resources).context(TRUNCATED)?;
    let mut resolution = 72.0;
    while c.offset + 12 <= resources_end {
        if c.tag()? != "8BIM" {
            break;
        }
        let id = c.u16()?;
        let name = c.u8()? as usize;
        c.skip(name + (name + 1) % 2)?;
        let length = c.u32()? as usize;
        let start = c.offset;
        if id == 1005 && length >= 4 {
            let value = c.u32()? as f64 / 65536.0;
            if value.is_finite() && value >= 1.0 {
                resolution = value.min(9600.0);
            }
        }
        c.seek(start + length + length % 2)?;
    }
    c.seek(resources_end)?;
    Ok(Header { width, height, channels, large: version == 2, resolution, layer_section: c.offset })
}

/// A layer's user mask: a rectangle of the document and the value everywhere outside it.
#[derive(Clone, Debug, Default)]
pub struct MaskInfo {
    pub top: i64,
    pub left: i64,
    pub bottom: i64,
    pub right: i64,
    pub default: u8,
    pub disabled: bool,
    pub linked: bool,
    /// Photoshop made the mask from other data (a vector mask), so it isn't the user's.
    pub from_render: bool,
    pub image: Option<GrayImage>,
}

impl MaskInfo {
    pub fn width(&self) -> u32 {
        (self.right - self.left).max(0) as u32
    }
    pub fn height(&self) -> u32 {
        (self.bottom - self.top).max(0) as u32
    }
}

#[derive(Debug, Default)]
pub struct RawLayer {
    pub name: String,
    pub top: i64,
    pub left: i64,
    pub bottom: i64,
    pub right: i64,
    pub opacity: u8,
    pub fill: u8,
    pub clipping: bool,
    pub hidden: bool,
    pub blend_key: String,
    pub channels: Vec<(i16, usize)>,
    /// Additional layer information by key.
    pub extra: HashMap<String, Vec<u8>>,
    pub mask: Option<MaskInfo>,
    /// Section divider type: 1 or 2 opens a folder (its record comes after its children), 3 ends one.
    pub section: Option<u32>,
    pub image: Option<RgbaImage>,
}

impl RawLayer {
    pub fn width(&self) -> u32 {
        (self.right - self.left).max(0) as u32
    }
    pub fn height(&self) -> u32 {
        (self.bottom - self.top).max(0) as u32
    }
}

/// Additional-information keys whose length is 8 bytes in a large document.
const WIDE_KEYS: &[&str] = &["LMsk", "Lr16", "Lr32", "Layr", "Mt16", "Mt32", "Mtrn", "Alph", "FMsk", "lnk2", "FEid", "FXid", "PxSD"];

/// Mac OS Roman for the legacy layer name (the Unicode name usually replaces it).
fn mac_roman(bytes: &[u8]) -> String {
    const HIGH: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø¿¡¬√ƒ≈∆«»…\u{a0}ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄€‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\u{f8ff}ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";
    let high: Vec<char> = HIGH.chars().collect();
    bytes.iter().map(|b| if *b < 128 { *b as char } else { high.get(*b as usize - 128).copied().unwrap_or('?') }).collect()
}

fn unicode_name(data: &[u8]) -> Option<String> {
    let count = u32::from_be_bytes(data.get(..4)?.try_into().ok()?) as usize;
    let units: Vec<u16> = data.get(4..4 + count.checked_mul(2)?)?.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    let name = String::from_utf16_lossy(&units).trim_end_matches('\0').to_string();
    (!name.is_empty()).then_some(name)
}

fn record(c: &mut Cursor, large: bool) -> Result<RawLayer> {
    let mut layer = RawLayer {
        top: c.i32()? as i64,
        left: c.i32()? as i64,
        bottom: c.i32()? as i64,
        right: c.i32()? as i64,
        ..RawLayer::default()
    };
    let count = c.u16()?;
    if count > 56 {
        bail!(TRUNCATED);
    }
    for _ in 0..count {
        let id = c.i16()?;
        let length = c.length(large)?;
        layer.channels.push((id, length));
    }
    if c.tag()? != "8BIM" {
        bail!(TRUNCATED);
    }
    layer.blend_key = c.tag()?;
    layer.opacity = c.u8()?;
    layer.fill = 255;
    layer.clipping = c.u8()? != 0;
    let flags = c.u8()?;
    layer.hidden = flags & 2 != 0;
    c.skip(1)?;
    let extra_length = c.u32()? as usize;
    let extra_end = c.offset.checked_add(extra_length).context(TRUNCATED)?;
    let mask_length = c.u32()? as usize;
    let mask_end = c.offset + mask_length;
    if mask_length >= 20 {
        let top = c.i32()? as i64;
        let left = c.i32()? as i64;
        let bottom = c.i32()? as i64;
        let right = c.i32()? as i64;
        let default = c.u8()?;
        let flags = c.u8()?;
        layer.mask = Some(MaskInfo {
            top,
            left,
            bottom,
            right,
            default,
            disabled: flags & 2 != 0,
            linked: flags & 1 == 0,
            from_render: flags & 8 != 0,
            image: None,
        });
    }
    c.seek(mask_end)?;
    let ranges = c.u32()? as usize;
    c.skip(ranges)?;
    let name_length = c.u8()? as usize;
    layer.name = mac_roman(c.bytes(name_length)?);
    c.skip((4 - (name_length + 1) % 4) % 4)?;
    while c.offset + 12 <= extra_end {
        let signature = c.tag()?;
        if signature != "8BIM" && signature != "8B64" {
            break;
        }
        let key = c.tag()?;
        let length = if signature == "8B64" || (large && WIDE_KEYS.contains(&key.as_str())) { c.length(true)? } else { c.u32()? as usize };
        let payload = c.bytes(length.min(extra_end.saturating_sub(c.offset)))?.to_vec();
        if length % 2 == 1 && c.offset < extra_end {
            c.skip(1)?;
        }
        match key.as_str() {
            "luni" => {
                if let Some(name) = unicode_name(&payload) {
                    layer.name = name;
                }
            }
            "iOpa" => layer.fill = payload.first().copied().unwrap_or(255),
            "lsct" | "lsdk" if payload.len() >= 4 => {
                layer.section = Some(u32::from_be_bytes(payload[..4].try_into().unwrap()));
            }
            _ => {}
        }
        layer.extra.insert(key, payload);
    }
    c.seek(extra_end)?;
    Ok(layer)
}

/// Unpacks one channel of `width`×`height` pixels.
fn channel(compression: u16, payload: &[u8], width: u32, height: u32, large: bool) -> Result<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    let size = w * h;
    match compression {
        0 => payload.get(..size).map(<[u8]>::to_vec).context(TRUNCATED),
        1 => {
            let count_size = if large { 4 } else { 2 };
            let counts = payload.get(..h * count_size).context(TRUNCATED)?;
            let mut offset = h * count_size;
            let mut plane = vec![0u8; size];
            for row in 0..h {
                let count = if large {
                    u32::from_be_bytes(counts[row * 4..row * 4 + 4].try_into().unwrap()) as usize
                } else {
                    u16::from_be_bytes([counts[row * 2], counts[row * 2 + 1]]) as usize
                };
                let end = offset + count;
                let data = payload.get(offset..end).context(TRUNCATED)?;
                unpack_bits(data, &mut plane[row * w..(row + 1) * w])?;
                offset = end;
            }
            Ok(plane)
        }
        2 | 3 => {
            let mut plane = Vec::with_capacity(size);
            flate2::read::ZlibDecoder::new(payload).take(size as u64).read_to_end(&mut plane).context(TRUNCATED)?;
            if plane.len() < size {
                bail!(TRUNCATED);
            }
            if compression == 3 {
                // Each row is stored as differences from the pixel before.
                for row in plane.chunks_mut(w.max(1)) {
                    for i in 1..row.len() {
                        row[i] = row[i].wrapping_add(row[i - 1]);
                    }
                }
            }
            Ok(plane)
        }
        _ => bail!("This Photoshop file uses a layer compression method that isn’t supported."),
    }
}

/// PackBits.
fn unpack_bits(data: &[u8], row: &mut [u8]) -> Result<()> {
    let (mut i, mut written) = (0, 0);
    while written < row.len() {
        let n = *data.get(i).context(TRUNCATED)? as i8;
        i += 1;
        if n >= 0 {
            let count = n as usize + 1;
            let bytes = data.get(i..i + count).context(TRUNCATED)?;
            row.get_mut(written..written + count).context(TRUNCATED)?.copy_from_slice(bytes);
            i += count;
            written += count;
        } else if n != -128 {
            let count = 1 - n as isize;
            let value = *data.get(i).context(TRUNCATED)?;
            i += 1;
            row.get_mut(written..written + count as usize).context(TRUNCATED)?.fill(value);
            written += count as usize;
        }
    }
    Ok(())
}

fn check_layer_size(w: u32, h: u32) -> Result<()> {
    if w > MAX_SIDE || h > MAX_SIDE || w as u64 * h as u64 > MAX_PIXELS {
        bail!("A layer in this Photoshop file exceeds the supported size.");
    }
    Ok(())
}

/// Decodes a layer's channels, which follow the records in the file in record order.
fn decode(c: &mut Cursor, layer: &mut RawLayer, large: bool) -> Result<()> {
    let (w, h) = (layer.width(), layer.height());
    check_layer_size(w, h)?;
    let mut planes: HashMap<i16, Vec<u8>> = HashMap::new();
    for (id, length) in layer.channels.clone() {
        let start = c.offset;
        let data = c.bytes(length)?;
        let is_mask = id == -2;
        let (cw, ch) = match (&layer.mask, is_mask) {
            (Some(mask), true) => (mask.width(), mask.height()),
            (None, true) => continue,
            _ => (w, h),
        };
        if ![-2, -1, 0, 1, 2].contains(&id) || length < 2 || cw == 0 || ch == 0 {
            continue;
        }
        check_layer_size(cw, ch)?;
        let compression = u16::from_be_bytes([data[0], data[1]]);
        planes.insert(id, channel(compression, &data[2..], cw, ch, large)?);
        c.seek(start + length)?;
    }
    if let (Some(mask), Some(gray)) = (layer.mask.as_mut(), planes.remove(&-2)) {
        mask.image = GrayImage::from_raw(mask.width(), mask.height(), gray);
    }
    if w == 0 || h == 0 {
        return Ok(());
    }
    let n = (w * h) as usize;
    let get = |id: i16, fill: u8| planes.get(&id).map_or_else(|| vec![fill; n], Clone::clone);
    let (r, g, b, a) = (get(0, 0), get(1, 0), get(2, 0), get(-1, 255));
    let mut rgba = Vec::with_capacity(n * 4);
    for i in 0..n {
        rgba.extend_from_slice(&[r[i], g[i], b[i], a[i]]);
    }
    layer.image = RgbaImage::from_raw(w, h, rgba);
    Ok(())
}

fn layer_info(c: &mut Cursor, end: usize, large: bool) -> Result<Vec<RawLayer>> {
    let count = c.i16()?.unsigned_abs() as usize;
    if count > 10_000 {
        bail!("This Photoshop file has too many layers.");
    }
    let mut layers = Vec::with_capacity(count);
    for _ in 0..count {
        layers.push(record(c, large)?);
    }
    let mut total = 0u64;
    for layer in &mut layers {
        total += layer.width() as u64 * layer.height() as u64;
        if total > MAX_PIXELS * 4 {
            bail!("This Photoshop file exceeds the supported document size.");
        }
        decode(c, layer, large)?;
    }
    if c.offset > end {
        bail!(TRUNCATED);
    }
    Ok(layers)
}

/// The layer records, bottom to top, with their pixels.
pub fn layers(data: &[u8], header: &Header) -> Result<Vec<RawLayer>> {
    let mut c = Cursor::new(data);
    c.seek(header.layer_section)?;
    let length = c.length(header.large)?;
    let end = c.offset + length;
    if length < 4 {
        return Ok(Vec::new());
    }
    let info = c.length(header.large)?;
    if info > 0 {
        let info_end = c.offset + info;
        return layer_info(&mut c, info_end, header.large);
    }
    // No layer info here: Photoshop may keep it in a `Layr` block after the global mask.
    let global = c.u32()? as usize;
    c.skip(global)?;
    while c.offset + 12 <= end {
        let signature = c.tag()?;
        if signature != "8BIM" && signature != "8B64" {
            break;
        }
        let key = c.tag()?;
        let length = if header.large && WIDE_KEYS.contains(&key.as_str()) { c.length(true)? } else { c.u32()? as usize };
        let block_end = c.offset + length;
        if key == "Layr" {
            return layer_info(&mut c, block_end, header.large);
        }
        c.seek(block_end + length % 2)?;
    }
    Ok(Vec::new())
}

/// The merged image Photoshop saves after the layers ("Maximize Compatibility").
pub fn composite(data: &[u8], header: &Header, transparent: bool) -> Result<RgbaImage> {
    let mut c = Cursor::new(data);
    c.seek(header.layer_section)?;
    let length = c.length(header.large)?;
    c.skip(length)?;
    let compression = c.u16()?;
    let (w, h) = (header.width as usize, header.height as usize);
    let n = w * h;
    let channels = (header.channels as usize).min(if transparent { 4 } else { 3 });
    let mut planes: Vec<Vec<u8>> = Vec::new();
    match compression {
        0 => {
            for _ in 0..channels {
                planes.push(c.bytes(n)?.to_vec());
            }
        }
        1 => {
            let count_size = if header.large { 4 } else { 2 };
            let total_rows = header.channels as usize * h;
            let counts = c.bytes(total_rows * count_size)?;
            let count = |row: usize| -> usize {
                if header.large {
                    u32::from_be_bytes(counts[row * 4..row * 4 + 4].try_into().unwrap()) as usize
                } else {
                    u16::from_be_bytes([counts[row * 2], counts[row * 2 + 1]]) as usize
                }
            };
            let mut row_index = 0;
            for _ in 0..channels {
                let mut plane = vec![0u8; n];
                for y in 0..h {
                    let bytes = c.bytes(count(row_index))?;
                    unpack_bits(bytes, &mut plane[y * w..(y + 1) * w])?;
                    row_index += 1;
                }
                planes.push(plane);
            }
        }
        _ => bail!("This Photoshop file’s merged image uses a compression method that isn’t supported."),
    }
    while planes.len() < 3 {
        planes.push(planes.first().cloned().unwrap_or_else(|| vec![0; n]));
    }
    let mut rgba = Vec::with_capacity(n * 4);
    for i in 0..n {
        let a = planes.get(3).map_or(255, |p| p[i]);
        rgba.extend_from_slice(&[planes[0][i], planes[1][i], planes[2][i], a]);
    }
    RgbaImage::from_raw(header.width, header.height, rgba).context(TRUNCATED)
}

/// Whether the merged image carries transparency: the file says so with a negative layer count.
pub fn composite_has_alpha(data: &[u8], header: &Header) -> bool {
    fn negative_count(data: &[u8], header: &Header) -> Result<bool> {
        let mut c = Cursor::new(data);
        c.seek(header.layer_section)?;
        let length = c.length(header.large)?;
        if length < 4 {
            return Ok(false);
        }
        let info = c.length(header.large)?;
        Ok(info >= 2 && c.i16()? < 0)
    }
    negative_count(data, header).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpacks_bits() {
        let mut row = [0u8; 6];
        unpack_bits(&[2, 1, 2, 3, 0xFE, 9], &mut row).unwrap();
        assert_eq!(row, [1, 2, 3, 9, 9, 9]);
        assert!(unpack_bits(&[5, 1], &mut row).is_err());
        assert_eq!(mac_roman(&[b'A', 0x8E]), "Aé");
    }
}
