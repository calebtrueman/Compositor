//! Image Size, Canvas Size and Trim, as in the macOS app's `ImageResizer`, `CanvasResizer` and
//! `ImageTrim`.

use std::sync::Arc;

use image::imageops::FilterType;
use image::{GrayImage, Luma, Rgba, RgbaImage};
use rayon::prelude::*;

use crate::doc::{Affine, Document, Layer, LayerTransform, MaskPixels, Sampling};
use crate::ui::dialogs::{ok_cancel, Dialog, DialogCtx, DialogState};

/// Largest canvas side, and area, the app allows.
pub const MAX_SIDE: u32 = 30_000;
pub const MAX_PIXELS: u64 = 100_000_000;

fn filter_for(sampling: Sampling) -> FilterType {
    match sampling {
        Sampling::High => FilterType::Lanczos3,
        Sampling::Smooth => FilterType::Triangle,
        Sampling::Nearest => FilterType::Nearest,
    }
}

/// Pixel-aligned bounds of `t` once the canvas scales by `sx`×`sy`.
fn scaled_bounds(t: &LayerTransform, sx: f64, sy: f64) -> (f64, f64, u32, u32) {
    let corners = t.corners().map(|(x, y)| (x * sx, y * sy));
    let left = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min).floor();
    let top = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min).floor();
    let right = corners.iter().map(|c| c.0).fold(f64::MIN, f64::max).ceil();
    let bottom = corners.iter().map(|c| c.1).fold(f64::MIN, f64::max).ceil();
    (left, top, ((right - left) as u32).max(1), ((bottom - top) as u32).max(1))
}

/// Draws `image`, placed by `t` on a canvas scaled by `sx`×`sy`, into a `w`×`h` grid at
/// (`left`, `top`) of the scaled canvas: the layer's transform baked into its pixels, as the
/// macOS app does (a scaled rotation can shear, which a transform can't hold).
fn rasterize_rgba(image: &RgbaImage, t: &LayerTransform, sx: f64, sy: f64, left: f64, top: f64, w: u32, h: u32, sampling: Sampling) -> RgbaImage {
    // First resample to about the output's density with a good filter, then place it.
    let tw = ((t.size[0] * sx).abs().round() as u32).clamp(1, MAX_SIDE);
    let th = ((t.size[1] * sy).abs().round() as u32).clamp(1, MAX_SIDE);
    let source = if (tw, th) == image.dimensions() { image.clone() } else { premultiplied_resize(image, tw, th, filter_for(sampling)) };
    let to_doc = t.pixel_to_document(source.width(), source.height());
    let Some(to_pixels) = to_doc.then(&Affine::scale(sx, sy)).inverse() else { return RgbaImage::new(w, h) };
    let mut out = RgbaImage::new(w, h);
    let (sw, sh) = (source.width() as i64, source.height() as i64);
    let nearest = sampling == Sampling::Nearest;
    out.as_mut().par_chunks_mut(w as usize * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w as usize {
            let (px, py) = to_pixels.apply(left + x as f64 + 0.5, top + y as f64 + 0.5);
            let mut acc = [0f64; 4];
            if nearest {
                let (ix, iy) = (px.floor() as i64, py.floor() as i64);
                if ix >= 0 && iy >= 0 && ix < sw && iy < sh {
                    let p = source.get_pixel(ix as u32, iy as u32);
                    row[x * 4..x * 4 + 4].copy_from_slice(&p.0);
                }
                continue;
            }
            let (fx, fy) = (px - 0.5, py - 0.5);
            let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
            let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
            for (j, wy) in [(0, 1.0 - ty), (1, ty)] {
                for (i, wx) in [(0, 1.0 - tx), (1, tx)] {
                    let (xx, yy) = (x0 + i, y0 + j);
                    let weight = wx * wy;
                    if weight <= 0.0 || xx < 0 || yy < 0 || xx >= sw || yy >= sh {
                        continue;
                    }
                    let p = source.get_pixel(xx as u32, yy as u32);
                    let a = p[3] as f64 / 255.0 * weight;
                    acc[0] += p[0] as f64 * a;
                    acc[1] += p[1] as f64 * a;
                    acc[2] += p[2] as f64 * a;
                    acc[3] += a;
                }
            }
            if acc[3] > 1e-6 {
                let o = &mut row[x * 4..x * 4 + 4];
                for c in 0..3 {
                    o[c] = (acc[c] / acc[3]).round().clamp(0.0, 255.0) as u8;
                }
                o[3] = (acc[3] * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    });
    out
}

/// Resizes straight-alpha pixels without dark fringes at soft edges.
fn premultiplied_resize(image: &RgbaImage, w: u32, h: u32, filter: FilterType) -> RgbaImage {
    let mut premultiplied = image.clone();
    premultiplied.pixels_mut().for_each(|p| {
        let a = p[3] as u32;
        for c in 0..3 {
            p[c] = ((p[c] as u32 * a + 127) / 255) as u8;
        }
    });
    let mut out = image::imageops::resize(&premultiplied, w, h, filter);
    out.pixels_mut().for_each(|p| {
        let a = p[3] as u32;
        if a == 0 {
            *p = Rgba([0, 0, 0, 0]);
        } else {
            for c in 0..3 {
                p[c] = ((p[c] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    });
    out
}

/// Image Size: resamples every layer and mask so the document is `width`×`height`, keeping
/// everything where it was relative to the canvas.
pub fn resize_document(doc: &mut Document, width: u32, height: u32, resolution: f64, sampling: Sampling) {
    doc.resolution = resolution;
    if (width, height) == (doc.width, doc.height) {
        return;
    }
    let sx = width as f64 / doc.width as f64;
    let sy = height as f64 / doc.height as f64;
    let scale_map = Affine::scale(sx, sy);
    doc.layers.par_iter_mut().for_each(|layer| resize_layer(layer, sx, sy, &scale_map, sampling));
    for guide in &mut doc.guides {
        guide.position *= match guide.axis {
            crate::doc::GuideAxis::Horizontal => sy,
            crate::doc::GuideAxis::Vertical => sx,
        };
    }
    doc.selection = doc.selection.take().and_then(|s| {
        let mask = image::imageops::resize(&*s.mask, width, height, FilterType::Triangle);
        crate::doc::Selection::from_mask(mask)
    });
    doc.width = width;
    doc.height = height;
}

fn resize_layer(layer: &mut Layer, sx: f64, sy: f64, scale_map: &Affine, sampling: Sampling) {
    let old = layer.transform;
    let (left, top, w, h) = scaled_bounds(&old, sx, sy);
    let mut placed = LayerTransform::rect(left, top, w as f64, h as f64);
    placed.sampling = old.sampling;
    if let Some(image) = &layer.image {
        layer.image = Some(Arc::new(rasterize_rgba(image, &old, sx, sy, left, top, w, h, sampling)));
        layer.rasterized();
    }
    if let Some(mask) = layer.mask.as_mut() {
        match (&mask.pixels, mask.linked) {
            (MaskPixels::Pixels(gray), true) => {
                // Through the same placement as the layer; the edge tone carries past the old edge.
                let rgba = RgbaImage::from_fn(gray.width(), gray.height(), |x, y| {
                    let v = gray.get_pixel(x, y)[0];
                    Rgba([v, v, v, 255])
                });
                let grown = pad_edge(&rgba);
                let mut t = old;
                t.size = [old.size[0] * grown.width() as f64 / rgba.width() as f64, old.size[1] * grown.height() as f64 / rgba.height() as f64];
                let (cx, cy) = old.center();
                t.origin = [cx - t.size[0] / 2.0, cy - t.size[1] / 2.0];
                let placed_mask = rasterize_rgba(&grown, &t, sx, sy, left, top, w, h, sampling);
                let out = GrayImage::from_fn(w, h, |x, y| Luma([placed_mask.get_pixel(x, y)[0]]));
                mask.pixels = MaskPixels::Pixels(Arc::new(out));
            }
            (_, false) => {
                if let Some(p) = mask.placement.as_mut() {
                    let mut moved = LayerTransform::from_affine(&p.unit_to_document().then(scale_map), p.rotation, p.flip_x);
                    moved.sampling = p.sampling;
                    *p = moved;
                }
            }
            _ => {}
        }
    }
    layer.transform = placed;
}

/// `image` with one more pixel on every side copying its edge.
fn pad_edge(image: &RgbaImage) -> RgbaImage {
    let (w, h) = image.dimensions();
    RgbaImage::from_fn(w + 2, h + 2, |x, y| *image.get_pixel(x.saturating_sub(1).min(w - 1), y.saturating_sub(1).min(h - 1)))
}

/// Canvas Size: the canvas grows or shrinks around `anchor` (0…8, row-major from top left);
/// layers keep their pixels. A `fill` color paints the new area on a layer of its own.
pub fn resize_canvas(doc: &mut Document, width: u32, height: u32, anchor: usize, fill: Option<[u8; 4]>) {
    let (ow, oh) = (doc.width, doc.height);
    let ox = ((width as f64 - ow as f64) * (anchor % 3) as f64 / 2.0).floor() as i64;
    let oy = ((height as f64 - oh as f64) * (anchor / 3) as f64 / 2.0).floor() as i64;
    crate::doc::ops::crop_canvas(doc, -ox, -oy, width, height);
    if let Some(color) = fill {
        if width > ow || height > oh {
            let mut image = RgbaImage::from_pixel(width, height, Rgba(color));
            for y in oy.max(0)..(oy + oh as i64).min(height as i64) {
                for x in ox.max(0)..(ox + ow as i64).min(width as i64) {
                    image.put_pixel(x as u32, y as u32, Rgba([0, 0, 0, 0]));
                }
            }
            let layer = Layer::new_pixel("Canvas Extension", image, doc.full_canvas_transform());
            doc.layers.insert(0, layer);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimBasis {
    Transparent,
    TopLeft,
    BottomRight,
}

/// The canvas rectangle Trim keeps, from the merged image, or `None` if nothing would be left.
pub fn trim_rect(image: &RgbaImage, basis: TrimBasis, sides: [bool; 4]) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let reference = match basis {
        TrimBasis::Transparent => None,
        TrimBasis::TopLeft => Some(*image.get_pixel(0, 0)),
        TrimBasis::BottomRight => Some(*image.get_pixel(w - 1, h - 1)),
    };
    let keep = |p: &Rgba<u8>| match reference {
        None => p[3] > 0,
        Some(r) => *p != r,
    };
    let rows: Vec<Option<(u32, u32)>> = (0..h)
        .into_par_iter()
        .map(|y| {
            let first = (0..w).find(|x| keep(image.get_pixel(*x, y)))?;
            let last = (0..w).rev().find(|x| keep(image.get_pixel(*x, y)))?;
            Some((first, last + 1))
        })
        .collect();
    let y0 = rows.iter().position(Option::is_some)? as u32;
    let y1 = rows.iter().rposition(Option::is_some)? as u32 + 1;
    let x0 = rows.iter().flatten().map(|r| r.0).min()?;
    let x1 = rows.iter().flatten().map(|r| r.1).max()?;
    let [top, bottom, left, right] = sides;
    Some((if left { x0 } else { 0 }, if top { y0 } else { 0 }, if right { x1 } else { w }, if bottom { y1 } else { h }))
}

// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Unit {
    Pixels,
    Percent,
    Inches,
    Centimeters,
}

impl Unit {
    const ALL: [Unit; 4] = [Unit::Pixels, Unit::Percent, Unit::Inches, Unit::Centimeters];

    fn name(self) -> &'static str {
        match self {
            Unit::Pixels => "Pixels",
            Unit::Percent => "Percent",
            Unit::Inches => "Inches",
            Unit::Centimeters => "Centimeters",
        }
    }

    fn to_display(self, pixels: f64, original: f64, resolution: f64) -> f64 {
        match self {
            Unit::Pixels => pixels,
            Unit::Percent => pixels / original * 100.0,
            Unit::Inches => pixels / resolution,
            Unit::Centimeters => pixels / resolution * 2.54,
        }
    }

    fn to_pixels(self, value: f64, original: f64, resolution: f64) -> f64 {
        match self {
            Unit::Pixels => value,
            Unit::Percent => value / 100.0 * original,
            Unit::Inches => value * resolution,
            Unit::Centimeters => value / 2.54 * resolution,
        }
    }

    fn speed(self) -> f64 {
        match self {
            Unit::Pixels => 1.0,
            Unit::Percent => 0.5,
            _ => 0.01,
        }
    }
}

fn unit_picker(ui: &mut egui::Ui, unit: &mut Unit, allowed: &[Unit]) {
    egui::ComboBox::from_id_salt("size-unit").selected_text(unit.name()).show_ui(ui, |ui| {
        for u in allowed {
            ui.selectable_value(unit, *u, u.name());
        }
    });
}

fn size_is_valid(w: f64, h: f64) -> bool {
    let (w, h) = (w.round(), h.round());
    w.is_finite() && h.is_finite() && (1.0..=MAX_SIDE as f64).contains(&w) && (1.0..=MAX_SIDE as f64).contains(&h) && w * h <= MAX_PIXELS as f64
}

pub struct ImageSizeDialog {
    original: (u32, u32),
    width: f64,
    height: f64,
    resolution: f64,
    locked: bool,
    resample: bool,
    unit: Unit,
    sampling: Sampling,
}

impl ImageSizeDialog {
    pub fn new(doc: &Document) -> Self {
        ImageSizeDialog {
            original: (doc.width, doc.height),
            width: doc.width as f64,
            height: doc.height as f64,
            resolution: doc.resolution,
            locked: true,
            resample: true,
            unit: Unit::Pixels,
            sampling: Sampling::High,
        }
    }
}

impl Dialog for ImageSizeDialog {
    fn title(&self) -> String {
        "Image Size".into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut DialogCtx) -> DialogState {
        let (ow, oh) = (self.original.0 as f64, self.original.1 as f64);
        ui.label(format!("Current: {} × {} pixels", self.original.0, self.original.1));
        let units: &[Unit] = if self.resample { &Unit::ALL } else { &Unit::ALL[2..] };
        if !units.contains(&self.unit) {
            self.unit = Unit::Inches;
        }
        egui::Grid::new("image-size").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Units");
            unit_picker(ui, &mut self.unit, units);
            ui.end_row();
            for (label, is_width) in [("Width", true), ("Height", false)] {
                ui.label(label);
                let (pixels, original) = if is_width { (self.width, ow) } else { (self.height, oh) };
                let mut shown = self.unit.to_display(pixels, original, self.resolution);
                let decimals = if self.unit == Unit::Pixels { 0 } else { 2 };
                if ui.add(egui::DragValue::new(&mut shown).speed(self.unit.speed()).max_decimals(decimals).range(0.001..=f64::MAX)).changed() && shown > 0.0 {
                    if !self.resample {
                        // Only the print size changes: the resolution follows it.
                        let inches = if self.unit == Unit::Centimeters { shown / 2.54 } else { shown };
                        self.resolution = (pixels / inches).clamp(1.0, 9600.0);
                    } else {
                        let new = self.unit.to_pixels(shown, original, self.resolution);
                        if is_width {
                            if self.locked {
                                self.height = new * self.height / self.width;
                            }
                            self.width = new;
                        } else {
                            if self.locked {
                                self.width = new * self.width / self.height;
                            }
                            self.height = new;
                        }
                    }
                }
                ui.end_row();
            }
            ui.label("");
            ui.add_enabled(self.resample, egui::Checkbox::new(&mut self.locked, "Constrain proportions"));
            ui.end_row();
            ui.label("Resolution");
            let before = self.resolution;
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.resolution).range(1.0..=9600.0).max_decimals(2));
                ui.label("pixels/inch");
            });
            if self.resolution != before && self.resample && matches!(self.unit, Unit::Inches | Unit::Centimeters) {
                // The print size stays; the pixel count follows the resolution.
                self.width *= self.resolution / before;
                self.height *= self.resolution / before;
            }
            ui.end_row();
            ui.label("");
            if ui.checkbox(&mut self.resample, "Resample").changed() && !self.resample {
                self.width = ow;
                self.height = oh;
                self.locked = true;
            }
            ui.end_row();
            if self.resample {
                ui.label("Sampling");
                egui::ComboBox::from_id_salt("image-size-sampling")
                    .selected_text(match self.sampling {
                        Sampling::High => "High quality",
                        Sampling::Smooth => "Smooth",
                        Sampling::Nearest => "Nearest",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.sampling, Sampling::High, "High quality");
                        ui.selectable_value(&mut self.sampling, Sampling::Smooth, "Smooth");
                        ui.selectable_value(&mut self.sampling, Sampling::Nearest, "Nearest");
                    });
                ui.end_row();
            }
        });
        let valid = size_is_valid(self.width, self.height);
        if valid {
            ui.weak(format!(
                "Result: {} × {} pixels ({:.2} × {:.2} in)",
                self.width.round(),
                self.height.round(),
                self.width.round() / self.resolution,
                self.height.round() / self.resolution
            ));
        } else {
            ui.colored_label(ui.visuals().warn_fg_color, "Use 1–30,000 pixels per side, up to 100 megapixels.");
        }
        if self.resample {
            ui.weak("Resamples layer pixels and masks; layers keep their places on the canvas.");
        } else {
            ui.weak("Only the print size and resolution change; pixels stay as they are.");
        }
        match ok_cancel(ui, "Resize") {
            Some(true) if valid => {
                if let Some(project) = ctx.project.as_deref_mut() {
                    let (w, h) = (self.width.round() as u32, self.height.round() as u32);
                    let (resolution, sampling) = (self.resolution, self.sampling);
                    project.edit("Image Size", |doc| resize_document(doc, w, h, resolution, sampling));
                    project.view.request_fit();
                }
                DialogState::Closed
            }
            Some(false) => DialogState::Closed,
            _ => DialogState::Open,
        }
    }
}

pub struct CanvasSizeDialog {
    original: (u32, u32),
    resolution: f64,
    width: f64,
    height: f64,
    relative: bool,
    locked: bool,
    unit: Unit,
    anchor: usize,
    extension: usize,
}

const EXTENSIONS: [&str; 6] = ["Transparent", "Foreground Color", "Background Color", "White", "Black", "Gray"];

impl CanvasSizeDialog {
    pub fn new(doc: &Document) -> Self {
        CanvasSizeDialog {
            original: (doc.width, doc.height),
            resolution: doc.resolution,
            width: doc.width as f64,
            height: doc.height as f64,
            relative: false,
            locked: false,
            unit: Unit::Pixels,
            anchor: 4,
            extension: 0,
        }
    }
}

impl Dialog for CanvasSizeDialog {
    fn title(&self) -> String {
        "Canvas Size".into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut DialogCtx) -> DialogState {
        let (ow, oh) = (self.original.0 as f64, self.original.1 as f64);
        ui.label(format!("Current: {} × {} pixels", self.original.0, self.original.1));
        egui::Grid::new("canvas-size").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Units");
            unit_picker(ui, &mut self.unit, &Unit::ALL);
            ui.end_row();
            for (label, is_width) in [("Width", true), ("Height", false)] {
                ui.label(label);
                let (pixels, original) = if is_width { (self.width, ow) } else { (self.height, oh) };
                let base = if self.relative { original } else { 0.0 };
                let mut shown = self.unit.to_display(pixels - base, original, self.resolution);
                let decimals = if self.unit == Unit::Pixels { 0 } else { 2 };
                if ui.add(egui::DragValue::new(&mut shown).speed(self.unit.speed()).max_decimals(decimals)).changed() {
                    let new = self.unit.to_pixels(shown, original, self.resolution) + base;
                    if is_width {
                        self.width = new;
                        if self.locked {
                            self.height = new * oh / ow;
                        }
                    } else {
                        self.height = new;
                        if self.locked {
                            self.width = new * ow / oh;
                        }
                    }
                }
                ui.end_row();
            }
            ui.label("");
            ui.checkbox(&mut self.relative, "Relative");
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut self.locked, "Keep aspect ratio");
            ui.end_row();
            ui.label("Anchor");
            egui::Grid::new("anchor").spacing([2.0, 2.0]).show(ui, |ui| {
                let arrows = [
                    crate::ui::icons::ARROW_UP_LEFT,
                    crate::ui::icons::ARROW_UP,
                    crate::ui::icons::ARROW_UP_RIGHT,
                    crate::ui::icons::ARROW_LEFT,
                    crate::ui::icons::CIRCLE,
                    crate::ui::icons::ARROW_RIGHT,
                    crate::ui::icons::ARROW_DOWN_LEFT,
                    crate::ui::icons::ARROW_DOWN,
                    crate::ui::icons::ARROW_DOWN_RIGHT,
                ];
                for i in 0..9 {
                    // Arrows point away from the anchor, toward where the canvas grows.
                    let (ar, ac) = (self.anchor / 3, self.anchor % 3);
                    let (r, c) = (i / 3, i % 3);
                    let (dr, dc) = (r as i64 - ar as i64, c as i64 - ac as i64);
                    let label = if i == self.anchor {
                        crate::ui::icons::CIRCLE.to_string()
                    } else if dr.abs() <= 1 && dc.abs() <= 1 {
                        arrows[((dr + 1) * 3 + dc + 1) as usize].to_string()
                    } else {
                        String::new()
                    };
                    let button = egui::Button::new(label).min_size(egui::vec2(26.0, 26.0)).selected(i == self.anchor);
                    if ui.add(button).clicked() {
                        self.anchor = i;
                    }
                    if c == 2 {
                        ui.end_row();
                    }
                }
            });
            ui.end_row();
            ui.label("Extension");
            egui::ComboBox::from_id_salt("canvas-extension").selected_text(EXTENSIONS[self.extension]).show_ui(ui, |ui| {
                for (i, name) in EXTENSIONS.iter().enumerate() {
                    ui.selectable_value(&mut self.extension, i, *name);
                }
            });
            ui.end_row();
        });
        let valid = size_is_valid(self.width, self.height);
        if valid {
            ui.weak(format!("New: {} × {} pixels. Layers keep their pixels; what falls outside stays outside the canvas.", self.width.round(), self.height.round()));
        } else {
            ui.colored_label(ui.visuals().warn_fg_color, "Use 1–30,000 pixels per side, up to 100 megapixels.");
        }
        match ok_cancel(ui, "OK") {
            Some(true) if valid => {
                let fill = match self.extension {
                    1 => Some(ctx.colors.foreground),
                    2 => Some(ctx.colors.background),
                    3 => Some([255, 255, 255, 255]),
                    4 => Some([0, 0, 0, 255]),
                    5 => Some([128, 128, 128, 255]),
                    _ => None,
                }
                .map(|mut c| {
                    c[3] = 255;
                    c
                });
                if let Some(project) = ctx.project.as_deref_mut() {
                    let (w, h, anchor) = (self.width.round() as u32, self.height.round() as u32, self.anchor);
                    if (w, h) != self.original {
                        project.edit("Canvas Size", |doc| resize_canvas(doc, w, h, anchor, fill));
                        project.view.request_fit();
                    }
                }
                DialogState::Closed
            }
            Some(false) => DialogState::Closed,
            _ => DialogState::Open,
        }
    }
}

pub struct TrimDialog {
    basis: TrimBasis,
    /// Top, bottom, left, right.
    sides: [bool; 4],
}

impl Default for TrimDialog {
    fn default() -> Self {
        TrimDialog { basis: TrimBasis::Transparent, sides: [true; 4] }
    }
}

impl Dialog for TrimDialog {
    fn title(&self) -> String {
        "Trim".into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut DialogCtx) -> DialogState {
        ui.strong("Based On");
        ui.radio_value(&mut self.basis, TrimBasis::Transparent, "Transparent Pixels");
        ui.radio_value(&mut self.basis, TrimBasis::TopLeft, "Top Left Pixel Color");
        ui.radio_value(&mut self.basis, TrimBasis::BottomRight, "Bottom Right Pixel Color");
        ui.add_space(6.0);
        ui.strong("Trim Away");
        egui::Grid::new("trim-sides").num_columns(2).show(ui, |ui| {
            ui.checkbox(&mut self.sides[0], "Top");
            ui.checkbox(&mut self.sides[1], "Bottom");
            ui.end_row();
            ui.checkbox(&mut self.sides[2], "Left");
            ui.checkbox(&mut self.sides[3], "Right");
            ui.end_row();
        });
        match ok_cancel(ui, "OK") {
            Some(true) => {
                if let Some(project) = ctx.project.as_deref_mut() {
                    let merged = crate::render::composite(&project.doc, crate::render::Region::full(&project.doc), ctx.cache).to_rgba8();
                    match trim_rect(&merged, self.basis, self.sides) {
                        Some((x0, y0, x1, y1)) if (x0, y0, x1, y1) != (0, 0, project.doc.width, project.doc.height) => {
                            project.edit("Trim", |doc| crate::doc::ops::crop_canvas(doc, x0 as i64, y0 as i64, x1 - x0, y1 - y0));
                            project.view.request_fit();
                        }
                        Some(_) => *ctx.status = Some("Nothing to trim".into()),
                        None => *ctx.status = Some("Trim would leave nothing".into()),
                    }
                }
                DialogState::Closed
            }
            Some(false) => DialogState::Closed,
            None => DialogState::Open,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_size_keeps_layers_in_place() {
        let mut doc = Document::new(100, 50, Some([10, 20, 30, 255]));
        let mut small = Layer::new_pixel("Dot", RgbaImage::from_pixel(10, 10, Rgba([255, 0, 0, 255])), LayerTransform::rect(20.0, 10.0, 10.0, 10.0));
        small.mask = Some(crate::doc::LayerMask {
            pixels: MaskPixels::Pixels(Arc::new(GrayImage::from_pixel(10, 10, Luma([200])))),
            enabled: true,
            linked: true,
            placement: None,
        });
        doc.layers.push(small);
        resize_document(&mut doc, 200, 100, 144.0, Sampling::High);
        assert_eq!((doc.width, doc.height, doc.resolution), (200, 100, 144.0));
        assert_eq!(doc.layers[0].transform.size, [200.0, 100.0]);
        assert_eq!(doc.layers[0].image.as_ref().unwrap().dimensions(), (200, 100));
        let dot = &doc.layers[1];
        assert_eq!(dot.transform.origin, [40.0, 20.0]);
        assert_eq!(dot.image.as_ref().unwrap().dimensions(), (20, 20));
        assert_eq!(dot.image.as_ref().unwrap().get_pixel(10, 10).0, [255, 0, 0, 255]);
        let MaskPixels::Pixels(mask) = &dot.mask.as_ref().unwrap().pixels else { panic!() };
        assert_eq!(mask.dimensions(), (20, 20));
        assert_eq!(mask.get_pixel(0, 0)[0], 200);
    }

    #[test]
    fn canvas_size_anchors_and_fills() {
        let mut doc = Document::new(10, 10, Some([0, 0, 255, 255]));
        resize_canvas(&mut doc, 20, 14, 8, Some([255, 255, 255, 255]));
        assert_eq!((doc.width, doc.height), (20, 14));
        assert_eq!(doc.layers.len(), 2);
        // Bottom-right anchor: the old canvas moved to the bottom right.
        assert_eq!(doc.layers[1].transform.origin, [10.0, 4.0]);
        let extension = doc.layers[0].image.as_ref().unwrap();
        assert_eq!(extension.get_pixel(0, 0)[3], 255);
        assert_eq!(extension.get_pixel(15, 10)[3], 0);
    }

    #[test]
    fn trim_finds_content() {
        let mut image = RgbaImage::new(10, 8);
        image.put_pixel(3, 2, Rgba([1, 2, 3, 255]));
        image.put_pixel(6, 5, Rgba([1, 2, 3, 255]));
        assert_eq!(trim_rect(&image, TrimBasis::Transparent, [true; 4]), Some((3, 2, 7, 6)));
        assert_eq!(trim_rect(&image, TrimBasis::Transparent, [false, true, true, true]), Some((3, 0, 7, 6)));
        assert_eq!(trim_rect(&RgbaImage::new(4, 4), TrimBasis::Transparent, [true; 4]), None);
    }
}
