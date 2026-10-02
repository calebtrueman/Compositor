//! Shared machinery for the painting tools: brush settings and the precomputed tip, the walker
//! that lays dabs along the pointer's path, and the stroke surface. A stroke accumulates the
//! brush's coverage in its own buffer and composites it over the pixels as they were when the
//! stroke began, so overlapping dabs never build past the stroke's opacity, as in Photoshop.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use egui::{Color32, Key, Modifiers, Painter, Stroke as LineStroke};
use image::{GrayImage, Luma, RgbaImage};
use rayon::prelude::*;

use super::{PointerEvent, ToolCtx};
use crate::doc::{Affine, Document, Id, MaskPixels};
use crate::project::{EditTarget, Project};
use crate::ui::canvas::ViewTransform;

/// Layer-pixel rectangle `(x0, y0, x1, y1)`, exclusive.
pub type PixelRect = (u32, u32, u32, u32);

/// Side of the squares a stroke tracks changes in, in layer pixels.
pub const TILE: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushSettings {
    /// Diameter in document pixels.
    pub size: f32,
    /// 0…1: the share of the radius painted at full strength.
    pub hardness: f32,
    /// 0…1: caps the whole stroke.
    pub opacity: f32,
    /// 0…1: how much each dab adds.
    pub flow: f32,
    /// Distance between dabs as a fraction of the diameter.
    pub spacing: f32,
    /// 0…100: the length of the string the brush trails the pointer on, in screen points.
    pub smoothing: f32,
}

impl Default for BrushSettings {
    fn default() -> Self {
        BrushSettings { size: 30.0, hardness: 1.0, opacity: 1.0, flow: 1.0, spacing: 0.1, smoothing: 0.0 }
    }
}

impl BrushSettings {
    /// Photoshop's `[` / `]` (size) and Shift-`[` / `]` (hardness in 25% steps).
    pub fn key(&mut self, key: Key, modifiers: Modifiers) -> bool {
        if modifiers.command || modifiers.alt {
            return false;
        }
        let (bigger, harder) = match key {
            Key::OpenBracket => (false, modifiers.shift),
            Key::CloseBracket => (true, modifiers.shift),
            Key::OpenCurlyBracket => (false, true),
            Key::CloseCurlyBracket => (true, true),
            _ => return false,
        };
        if harder {
            let quarter = self.hardness * 4.0;
            let step = if bigger { (quarter + 0.001).floor() + 1.0 } else { (quarter - 0.001).ceil() - 1.0 };
            self.hardness = step.clamp(0.0, 4.0) / 4.0;
        } else {
            // A fifth at a time, but always at least a pixel so the smallest sizes stay reachable.
            let s = self.size;
            let next = if bigger { (s + 1.0).max((s * 1.2).round()) } else { (s - 1.0).min((s / 1.2).round()) };
            self.size = next.clamp(1.0, 5000.0);
        }
        true
    }
}

/// Which brush settings a tool's options bar shows.
pub struct Fields {
    pub hardness: bool,
    /// The label for opacity ("Opacity", "Strength"), or `None` to hide it.
    pub opacity: Option<&'static str>,
    pub flow: bool,
    pub smoothing: bool,
}

impl Fields {
    pub const ALL: Fields = Fields { hardness: true, opacity: Some("Opacity"), flow: true, smoothing: true };
}

fn percent(ui: &mut egui::Ui, label: &str, value: &mut f32, min: f32) {
    ui.label(label);
    let mut p = *value * 100.0;
    if ui.add(egui::DragValue::new(&mut p).range(min..=100.0).speed(0.5).suffix("%").max_decimals(0)).changed() {
        *value = p / 100.0;
    }
}

/// The brush part of a painting tool's options bar.
pub fn options_ui(ui: &mut egui::Ui, settings: &mut BrushSettings, fields: Fields) {
    ui.label("Size");
    ui.add(egui::DragValue::new(&mut settings.size).range(1.0..=5000.0).speed(0.5).suffix(" px").max_decimals(0))
        .on_hover_text("[ and ] change the size");
    if fields.hardness {
        percent(ui, "Hardness", &mut settings.hardness, 0.0);
    }
    if let Some(label) = fields.opacity {
        percent(ui, label, &mut settings.opacity, 1.0);
    }
    if fields.flow {
        percent(ui, "Flow", &mut settings.flow, 1.0);
    }
    percent(ui, "Spacing", &mut settings.spacing, 1.0);
    if fields.smoothing {
        ui.label("Smoothing");
        ui.add(egui::DragValue::new(&mut settings.smoothing).range(0.0..=100.0).speed(0.5).suffix("%").max_decimals(0))
            .on_hover_text("The brush trails the pointer on a string this long, steadying shaky lines");
    }
}

/// The brush outline at the pointer, as Photoshop draws it. Tiny brushes get the system
/// crosshair instead (see `cursor`).
pub fn draw_outline(painter: &Painter, view: &ViewTransform, pos: (f64, f64), diameter: f32) {
    let center = view.to_screen(pos.0, pos.1);
    let radius = diameter * view.zoom / 2.0;
    if radius >= 3.0 {
        painter.circle_stroke(center, radius + 0.5, LineStroke::new(1.0, Color32::from_black_alpha(160)));
        painter.circle_stroke(center, radius - 0.5, LineStroke::new(1.0, Color32::from_white_alpha(200)));
    }
}

pub fn draw_crosshair(painter: &Painter, center: egui::Pos2, size: f32) {
    for (stroke, w) in [(LineStroke::new(3.0, Color32::from_black_alpha(140)), size + 1.0), (LineStroke::new(1.0, Color32::WHITE), size)] {
        painter.line_segment([center - egui::vec2(w, 0.0), center + egui::vec2(w, 0.0)], stroke);
        painter.line_segment([center - egui::vec2(0.0, w), center + egui::vec2(0.0, w)], stroke);
    }
}

/// Hides the system pointer when the outline is big enough to aim with.
pub fn cursor(diameter: f32, zoom: f32) -> egui::CursorIcon {
    if diameter * zoom >= 6.0 {
        egui::CursorIcon::None
    } else {
        egui::CursorIcon::Crosshair
    }
}

/// A brush tip: coverage by distance from the dab's center, precomputed over squared distance so
/// stamping needs no square roots.
#[derive(Clone, Debug)]
pub struct Tip {
    pub radius: f64,
    lut: Vec<f32>,
    /// Squared distance (document pixels) past which coverage is zero.
    reach2: f64,
    step: f64,
}

impl Tip {
    const STEPS: usize = 2048;

    pub fn new(diameter: f64, hardness: f64) -> Tip {
        let radius = (diameter / 2.0).max(0.5);
        let reach = radius + 0.5;
        let reach2 = reach * reach;
        let step = reach2 / Self::STEPS as f64;
        let lut = (0..=Self::STEPS + 1).map(|i| Self::profile((i as f64 * step).sqrt(), radius, hardness) as f32).collect();
        Tip { radius, lut, reach2, step }
    }

    /// Coverage at distance `d` from the center: full inside `hardness · radius`, then a
    /// Gaussian fade reaching zero at the rim, with a one-pixel anti-aliased edge.
    fn profile(d: f64, radius: f64, hardness: f64) -> f64 {
        let edge = (radius + 0.5 - d).clamp(0.0, 1.0);
        let inner = radius * hardness.clamp(0.0, 1.0);
        if d <= inner || radius - inner < 1e-6 {
            return edge;
        }
        let u = ((d - inner) / (radius - inner)).min(1.0);
        let k = 2.5f64;
        let fall = (((-k * u * u).exp() - (-k).exp()) / (1.0 - (-k).exp())).max(0.0);
        fall * edge
    }

    #[inline]
    pub fn coverage(&self, d2: f64) -> f32 {
        if d2 >= self.reach2 {
            return 0.0;
        }
        let f = d2 / self.step;
        let i = f as usize;
        let t = (f - i as f64) as f32;
        self.lut[i] + (self.lut[i + 1] - self.lut[i]) * t
    }
}

/// Lays dabs at even spacing along the pointer's path, rounding its corners: each new point
/// extends a quadratic curve through the midpoints of the segments between pointer samples.
#[derive(Clone, Debug, Default)]
pub struct Walker {
    /// The last pointer sample (the curve's next control point).
    control: Option<(f64, f64)>,
    /// Where the laid path ends.
    end: (f64, f64),
    /// Distance along the path to the next dab.
    next: f64,
}

impl Walker {
    /// Starts a path with a dab at `p`.
    pub fn start(&mut self, p: (f64, f64)) -> Vec<(f64, f64)> {
        self.control = Some(p);
        self.end = p;
        self.next = 0.0;
        vec![p]
    }

    /// A straight line from `from` to `to` (Shift-click), dabbing both ends.
    pub fn line(&mut self, from: (f64, f64), to: (f64, f64), spacing: f64) -> Vec<(f64, f64)> {
        let mut dabs = self.start(from);
        self.segment(from, to, spacing, &mut dabs);
        self.control = Some(to);
        self.end = to;
        dabs
    }

    /// Continues the path toward `p`.
    pub fn to(&mut self, p: (f64, f64), spacing: f64) -> Vec<(f64, f64)> {
        let mut dabs = Vec::new();
        let Some(control) = self.control else { return self.start(p) };
        let mid = ((control.0 + p.0) / 2.0, (control.1 + p.1) / 2.0);
        let start = self.end;
        let length = (control.0 - start.0).hypot(control.1 - start.1) + (mid.0 - control.0).hypot(mid.1 - control.1);
        let pieces = (length / 2.0).ceil().clamp(1.0, 256.0) as usize;
        let mut last = start;
        for i in 1..=pieces {
            let t = i as f64 / pieces as f64;
            let u = 1.0 - t;
            let q = (
                u * u * start.0 + 2.0 * u * t * control.0 + t * t * mid.0,
                u * u * start.1 + 2.0 * u * t * control.1 + t * t * mid.1,
            );
            self.segment(last, q, spacing, &mut dabs);
            last = q;
        }
        self.end = mid;
        self.control = Some(p);
        dabs
    }

    /// Finishes the path at the last pointer sample.
    pub fn finish(&mut self, spacing: f64) -> Vec<(f64, f64)> {
        let mut dabs = Vec::new();
        if let Some(control) = self.control.take() {
            let end = self.end;
            self.segment(end, control, spacing, &mut dabs);
            self.end = control;
        }
        dabs
    }

    fn segment(&mut self, a: (f64, f64), b: (f64, f64), spacing: f64, dabs: &mut Vec<(f64, f64)>) {
        let length = (b.0 - a.0).hypot(b.1 - a.1);
        if length <= 0.0 {
            return;
        }
        let mut at = if self.next <= 0.0 { spacing } else { self.next };
        while at <= length {
            let t = at / length;
            dabs.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            at += spacing;
        }
        self.next = at - length;
    }
}

/// The pixels a stroke started from, shared with the undo snapshot (no copy is made).
#[derive(Clone)]
pub enum Original {
    Rgba(Arc<RgbaImage>),
    Gray(Arc<GrayImage>),
}

pub enum PixelsMut<'a> {
    Rgba(&'a mut RgbaImage),
    Gray(&'a mut GrayImage),
}

/// What a stroke lays where it covers.
pub enum Paint<'a> {
    /// A straight sRGB color laid over the pixels; masks take its gray.
    Color([u8; 4]),
    /// Clears alpha; masks go black.
    Erase,
    /// Per-pixel straight RGBA (by layer pixel) laid over the pixels.
    Over(&'a (dyn Fn(u32, u32) -> [u8; 4] + Sync)),
    /// Per-pixel straight RGBA the pixels crossfade toward, alpha included.
    Mix(&'a (dyn Fn(u32, u32) -> [u8; 4] + Sync)),
}

/// The layer pixels (or mask) a painting tool edits, and how they sit on the canvas.
pub struct Surface {
    pub layer: Id,
    pub mask: bool,
    pub width: u32,
    pub height: u32,
    pub to_doc: Affine,
    pub to_pixel: Affine,
    pub original: Original,
    canvas: (u32, u32),
    selection: Option<Arc<GrayImage>>,
}

impl Surface {
    /// Checks the active layer can be painted, starts an undo step named `name`, and prepares the
    /// pixels: a blank layer gets a canvas-sized image and a uniform mask real pixels.
    pub fn begin(project: &mut Project, name: &str, allow_mask: bool) -> Result<Surface, String> {
        let doc = &project.doc;
        let Some(layer) = doc.active_layer() else { return Err("Select a layer to paint on.".into()) };
        if layer.is_group || layer.adjustment.is_some() {
            return Err(format!("{name} needs a pixel layer; select one or add a new layer."));
        }
        if !doc.is_effectively_visible(layer.id) {
            return Err(format!("{name} can't paint on a hidden layer."));
        }
        let mask = project.target == EditTarget::Mask && layer.mask.is_some();
        if mask && !allow_mask {
            return Err(format!("{name} works on the layer's pixels; select the layer thumbnail instead of its mask."));
        }
        if !mask && layer.image.is_none() && (layer.text.is_some() || layer.shape.is_some()) {
            return Err("Rasterize the layer before painting on it.".into());
        }
        let id = layer.id;
        let canvas = (doc.width, doc.height);
        let canvas_transform = doc.full_canvas_transform();
        let selection = doc.selection.as_ref().map(|s| s.mask.clone());
        let placement = match (&layer.mask, mask) {
            (Some(m), true) if !m.linked => m.placement.unwrap_or(layer.transform),
            _ if layer.image.is_none() && !mask => canvas_transform,
            _ => layer.transform,
        };
        let (w, h) = match (&layer.mask, mask, &layer.image) {
            (Some(m), true, _) => match &m.pixels {
                MaskPixels::Pixels(p) => p.dimensions(),
                MaskPixels::Uniform(_) => layer.pixel_size(),
            },
            (_, _, Some(image)) => image.dimensions(),
            _ => canvas,
        };
        let to_doc = placement.pixel_to_document(w, h);
        let Some(to_pixel) = to_doc.inverse() else { return Err("The layer is too small to paint on.".into()) };

        project.begin_edit(name);
        let layer = project.doc.layer_mut(id).unwrap();
        let original = if mask {
            let m = layer.mask.as_mut().unwrap();
            if let MaskPixels::Uniform(v) = m.pixels {
                m.pixels = MaskPixels::Pixels(Arc::new(GrayImage::from_pixel(w, h, Luma([v]))));
            }
            let MaskPixels::Pixels(gray) = &m.pixels else { unreachable!() };
            Original::Gray(gray.clone())
        } else {
            if layer.image.is_none() {
                layer.image = Some(Arc::new(RgbaImage::new(w, h)));
                layer.transform = canvas_transform;
            }
            layer.rasterized();
            Original::Rgba(layer.image.clone().unwrap())
        };
        Ok(Surface { layer: id, mask, width: w, height: h, to_doc, to_pixel, original, canvas, selection })
    }

    /// The pixels to write, copied once per stroke off the undo snapshot's.
    pub fn pixels_mut<'a>(&self, doc: &'a mut Document) -> Option<PixelsMut<'a>> {
        let layer = doc.layer_mut(self.layer)?;
        let pixels = if self.mask {
            match &mut layer.mask.as_mut()?.pixels {
                MaskPixels::Pixels(gray) => PixelsMut::Gray(Arc::make_mut(gray)),
                MaskPixels::Uniform(_) => return None,
            }
        } else {
            PixelsMut::Rgba(Arc::make_mut(layer.image.as_mut()?))
        };
        let size = match &pixels {
            PixelsMut::Rgba(i) => i.dimensions(),
            PixelsMut::Gray(i) => i.dimensions(),
        };
        // The layer changed under the stroke (an undo, say): leave it alone.
        (size == (self.width, self.height)).then_some(pixels)
    }

    /// How much of layer pixel `(x, y)` may change: the selection's coverage, and nothing off the canvas.
    #[inline]
    pub fn limit(&self, x: u32, y: u32) -> f32 {
        let (dx, dy) = self.to_doc.apply(x as f64 + 0.5, y as f64 + 0.5);
        if dx < 0.0 || dy < 0.0 || dx >= self.canvas.0 as f64 || dy >= self.canvas.1 as f64 {
            return 0.0;
        }
        match &self.selection {
            Some(s) => s.get_pixel(dx as u32, dy as u32)[0] as f32 / 255.0,
            None => 1.0,
        }
    }

    /// The original pixel, straight RGBA (a mask's gray repeated, opaque).
    #[inline]
    pub fn original_at(&self, x: u32, y: u32) -> [u8; 4] {
        match &self.original {
            Original::Rgba(image) => image.get_pixel(x, y).0,
            Original::Gray(gray) => {
                let g = gray.get_pixel(x, y)[0];
                [g, g, g, 255]
            }
        }
    }

    /// Document pixels a layer-pixel rectangle lands on, grown a pixel for resampling.
    pub fn doc_rect(&self, r: PixelRect) -> crate::project::DocRect {
        let corners = [(r.0, r.1), (r.2, r.1), (r.2, r.3), (r.0, r.3)].map(|(x, y)| self.to_doc.apply(x as f64, y as f64));
        let x0 = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min);
        let y0 = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min);
        let x1 = corners.iter().map(|c| c.0).fold(f64::MIN, f64::max);
        let y1 = corners.iter().map(|c| c.1).fold(f64::MIN, f64::max);
        (x0.floor() as i64 - 1, y0.floor() as i64 - 1, x1.ceil() as i64 + 1, y1.ceil() as i64 + 1)
    }

    /// Layer pixels a document rectangle touches, clipped to the image.
    pub fn pixel_rect(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> Option<PixelRect> {
        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| self.to_pixel.apply(x, y));
        let px0 = corners.iter().map(|c| c.0).fold(f64::MAX, f64::min).floor().max(0.0);
        let py0 = corners.iter().map(|c| c.1).fold(f64::MAX, f64::min).floor().max(0.0);
        let px1 = corners.iter().map(|c| c.0).fold(f64::MIN, f64::max).ceil().min(self.width as f64);
        let py1 = corners.iter().map(|c| c.1).fold(f64::MIN, f64::max).ceil().min(self.height as f64);
        (px1 > px0 && py1 > py0).then_some((px0 as u32, py0 as u32, px1 as u32, py1 as u32))
    }

    /// Lays `paint` over rows `y0..y1` (`spans` are column ranges) with each pixel's strength
    /// from `strength(x, y)`, starting from the original pixels.
    pub fn apply(
        &self,
        doc: &mut Document,
        rows: (u32, u32),
        spans: &[(u32, u32)],
        paint: &Paint,
        strength: &(dyn Fn(u32, u32) -> f32 + Sync),
    ) {
        let Some(pixels) = self.pixels_mut(doc) else { return };
        let (y0, y1) = rows;
        let w = self.width as usize;
        match pixels {
            PixelsMut::Rgba(image) => {
                let Original::Rgba(original) = &self.original else { return };
                let raw: &mut [u8] = image.as_mut();
                raw[y0 as usize * w * 4..y1 as usize * w * 4].par_chunks_mut(w * 4).enumerate().for_each(|(j, row)| {
                    let y = y0 + j as u32;
                    for &(x0, x1) in spans {
                        for x in x0..x1 {
                            let s = strength(x, y);
                            if s <= 0.0 {
                                continue;
                            }
                            let o = original.get_pixel(x, y).0;
                            let out = match paint {
                                Paint::Color(c) => over(o, *c, s),
                                Paint::Erase => [o[0], o[1], o[2], unit_u8(o[3] as f32 * (1.0 - s))],
                                Paint::Over(f) => over(o, f(x, y), s),
                                Paint::Mix(f) => mix(o, f(x, y), s),
                            };
                            row[x as usize * 4..x as usize * 4 + 4].copy_from_slice(&out);
                        }
                    }
                });
            }
            PixelsMut::Gray(gray) => {
                let Original::Gray(original) = &self.original else { return };
                let raw: &mut [u8] = gray.as_mut();
                raw[y0 as usize * w..y1 as usize * w].par_chunks_mut(w).enumerate().for_each(|(j, row)| {
                    let y = y0 + j as u32;
                    for &(x0, x1) in spans {
                        for x in x0..x1 {
                            let s = strength(x, y);
                            if s <= 0.0 {
                                continue;
                            }
                            let o = original.get_pixel(x, y)[0] as f32;
                            let (target, amount) = match paint {
                                Paint::Color(c) => (gray_of(*c) as f32, s * c[3] as f32 / 255.0),
                                Paint::Erase => (0.0, s),
                                Paint::Over(f) => {
                                    let c = f(x, y);
                                    (gray_of(c) as f32, s * c[3] as f32 / 255.0)
                                }
                                Paint::Mix(f) => (f(x, y)[0] as f32, s),
                            };
                            row[x as usize] = unit_u8(o + (target - o) * amount);
                        }
                    }
                });
            }
        }
    }
}

#[inline]
fn unit_u8(v: f32) -> u8 {
    (v + 0.5).clamp(0.0, 255.0) as u8
}

/// Rec. 601 luma, the gray a color paints on a mask.
pub fn gray_of(c: [u8; 4]) -> u8 {
    unit_u8(0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32)
}

/// Straight-alpha `s` laid over `o` at `strength` (multiplying `s`'s alpha).
#[inline]
pub fn over(o: [u8; 4], s: [u8; 4], strength: f32) -> [u8; 4] {
    let a = s[3] as f32 / 255.0 * strength.clamp(0.0, 1.0);
    if a <= 0.0 {
        return o;
    }
    let da = o[3] as f32 / 255.0;
    let oa = a + da * (1.0 - a);
    let mut out = [0u8; 4];
    for i in 0..3 {
        out[i] = unit_u8((s[i] as f32 * a + o[i] as f32 * da * (1.0 - a)) / oa.max(1e-6));
    }
    out[3] = unit_u8(oa * 255.0);
    out
}

/// Crossfades straight-alpha `o` toward `s` by `t`, in premultiplied terms.
#[inline]
pub fn mix(o: [u8; 4], s: [u8; 4], t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    let (oa, sa) = (o[3] as f32 / 255.0, s[3] as f32 / 255.0);
    let a = oa + (sa - oa) * t;
    if a <= 0.0 {
        return [0, 0, 0, 0];
    }
    let mut out = [0u8; 4];
    for i in 0..3 {
        let p = o[i] as f32 * oa + (s[i] as f32 * sa - o[i] as f32 * oa) * t;
        out[i] = unit_u8(p / a);
    }
    out[3] = unit_u8(a * 255.0);
    out
}

/// Rows of changed tiles: `rows` in layer pixels and the column spans changed in them.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub rows: (u32, u32),
    pub spans: Vec<(u32, u32)>,
}

/// One stroke in progress: the surface, the brush coverage it has laid, and the tiles changed
/// since they were last composited.
pub struct Stroke {
    pub surface: Surface,
    /// Accumulated coverage 0…1 per layer pixel. Zeroed pages are mapped lazily, so this costs
    /// only what the stroke touches.
    coverage: Vec<f32>,
    tip: Tip,
    flow: f32,
    spacing: f64,
    walker: Walker,
    /// Changed tiles by tile row, then tile column.
    dirty: BTreeMap<u32, BTreeSet<u32>>,
    /// Everything the stroke has touched.
    pub bounds: Option<PixelRect>,
    /// Where the last dab landed (document pixels).
    pub last: (f64, f64),
}

impl Stroke {
    pub fn new(surface: Surface, settings: &BrushSettings) -> Stroke {
        let n = surface.width as usize * surface.height as usize;
        Stroke {
            surface,
            coverage: vec![0.0; n],
            tip: Tip::new(settings.size as f64, settings.hardness as f64),
            flow: settings.flow.clamp(0.01, 1.0),
            spacing: (settings.size as f64 * settings.spacing as f64).max(0.5),
            walker: Walker::default(),
            dirty: BTreeMap::new(),
            bounds: None,
            last: (0.0, 0.0),
        }
    }

    pub fn start(&mut self, p: (f64, f64)) {
        let dabs = self.walker.start(p);
        self.stamp_all(&dabs);
    }

    pub fn line(&mut self, from: (f64, f64), to: (f64, f64)) {
        let dabs = self.walker.line(from, to, self.spacing);
        self.stamp_all(&dabs);
        self.last = to;
    }

    pub fn to(&mut self, p: (f64, f64)) {
        let dabs = self.walker.to(p, self.spacing);
        self.stamp_all(&dabs);
    }

    pub fn finish(&mut self) {
        let dabs = self.walker.finish(self.spacing);
        self.stamp_all(&dabs);
        self.last = self.walker.end;
    }

    fn stamp_all(&mut self, dabs: &[(f64, f64)]) {
        for &d in dabs {
            self.stamp(d);
            self.last = d;
        }
    }

    /// Adds one dab's coverage: `c + (1 − c) · tip · flow`, so dabs build toward full coverage
    /// but never past it.
    pub fn stamp(&mut self, center: (f64, f64)) {
        let r = self.tip.radius + 0.5;
        let Some(rect) = self.surface.pixel_rect(center.0 - r, center.1 - r, center.0 + r, center.1 + r) else { return };
        let (x0, y0, x1, y1) = rect;
        let w = self.surface.width as usize;
        let to_doc = self.surface.to_doc;
        let tip = &self.tip;
        let flow = self.flow;
        let stamp_row = |j: usize, row: &mut [f32]| {
            let y = y0 as f64 + j as f64 + 0.5;
            let (mut dx, mut dy) = to_doc.apply(x0 as f64 + 0.5, y);
            dx -= center.0;
            dy -= center.1;
            for c in &mut row[x0 as usize..x1 as usize] {
                let t = tip.coverage(dx * dx + dy * dy);
                if t > 0.0 {
                    *c += (1.0 - *c) * t * flow;
                }
                dx += to_doc.a;
                dy += to_doc.b;
            }
        };
        let rows = &mut self.coverage[y0 as usize * w..y1 as usize * w];
        if (x1 - x0) as usize * (y1 - y0) as usize > 4096 {
            rows.par_chunks_mut(w).enumerate().for_each(|(j, row)| stamp_row(j, row));
        } else {
            rows.chunks_mut(w).enumerate().for_each(|(j, row)| stamp_row(j, row));
        }
        self.mark(rect);
    }

    fn mark(&mut self, rect: PixelRect) {
        for ty in rect.1 / TILE..=(rect.3 - 1) / TILE {
            let set = self.dirty.entry(ty).or_default();
            for tx in rect.0 / TILE..=(rect.2 - 1) / TILE {
                set.insert(tx);
            }
        }
        self.bounds = Some(match self.bounds {
            Some(b) => (b.0.min(rect.0), b.1.min(rect.1), b.2.max(rect.2), b.3.max(rect.3)),
            None => rect,
        });
    }

    /// Coverage laid so far at layer pixel `(x, y)`.
    #[inline]
    pub fn coverage(&self, x: u32, y: u32) -> f32 {
        self.coverage[y as usize * self.surface.width as usize + x as usize]
    }

    /// Tiles changed since the last call, as bands of rows with column spans.
    pub fn take_dirty(&mut self) -> Vec<Band> {
        let (w, h) = (self.surface.width, self.surface.height);
        let dirty = std::mem::take(&mut self.dirty);
        dirty
            .into_iter()
            .map(|(ty, columns)| {
                let mut spans: Vec<(u32, u32)> = Vec::new();
                for tx in columns {
                    let (x0, x1) = (tx * TILE, ((tx + 1) * TILE).min(w));
                    match spans.last_mut() {
                        Some(last) if last.1 == x0 => last.1 = x1,
                        _ => spans.push((x0, x1)),
                    }
                }
                Band { rows: (ty * TILE, ((ty + 1) * TILE).min(h)), spans }
            })
            .collect()
    }

    /// Composites `paint` through the coverage (times `opacity` and the selection) into `bands`,
    /// and redraws them.
    pub fn composite(&self, project: &mut Project, bands: &[Band], opacity: f32, paint: &Paint) {
        let strength = |x: u32, y: u32| {
            let c = self.coverage(x, y);
            if c <= 0.0 {
                0.0
            } else {
                c * opacity * self.surface.limit(x, y)
            }
        };
        for band in bands {
            self.surface.apply(&mut project.doc, band.rows, &band.spans, paint, &strength);
            for &(x0, x1) in &band.spans {
                project.invalidate(self.surface.doc_rect((x0, band.rows.0, x1, band.rows.1)));
            }
        }
    }

    /// `take_dirty` then `composite`.
    pub fn flush(&mut self, project: &mut Project, opacity: f32, paint: &Paint) {
        let bands = self.take_dirty();
        self.composite(project, &bands, opacity, paint);
    }
}

/// What a pointer event did to a stroke.
pub enum StrokeStep {
    None,
    /// New coverage to composite.
    Painted,
    /// The stroke ended; composite what's left, then `finish_edit`.
    Finished(Box<Stroke>),
}

/// Pointer handling shared by the brush tools: starting strokes (Shift-click draws a line from
/// where the last one ended), Smoothing, and ending them.
#[derive(Default)]
pub struct StrokeDriver {
    pub stroke: Option<Stroke>,
    /// The end of the last stroke, with its layer and whether it was on the mask.
    last: Option<(Id, bool, (f64, f64))>,
    /// Where the brush is with Smoothing on; it trails the pointer.
    pub anchor: Option<(f64, f64)>,
}

impl StrokeDriver {
    pub fn pointer(
        &mut self,
        event: &PointerEvent,
        ctx: &mut ToolCtx,
        settings: &BrushSettings,
        name: &str,
        allow_mask: bool,
    ) -> StrokeStep {
        use super::PointerPhase::*;
        match event.phase {
            Press => {
                if self.stroke.is_some() {
                    return StrokeStep::None;
                }
                let surface = match Surface::begin(ctx.project, name, allow_mask) {
                    Ok(surface) => surface,
                    Err(message) => {
                        *ctx.status = Some(message);
                        return StrokeStep::None;
                    }
                };
                let mut stroke = Stroke::new(surface, settings);
                let from = self
                    .last
                    .filter(|(id, mask, _)| event.modifiers.shift && *id == stroke.surface.layer && *mask == stroke.surface.mask)
                    .map(|l| l.2);
                match from {
                    Some(from) => stroke.line(from, event.pos),
                    None => stroke.start(event.pos),
                }
                self.anchor = Some(event.pos);
                self.stroke = Some(stroke);
                StrokeStep::Painted
            }
            Drag => {
                let Some(stroke) = self.stroke.as_mut() else { return StrokeStep::None };
                let Some(anchor) = self.anchor else { return StrokeStep::None };
                let mut p = event.pos;
                if settings.smoothing > 0.0 {
                    // The brush only moves once the pointer pulls the string taut; its length is in
                    // screen points so it feels the same at any zoom.
                    let radius = settings.smoothing as f64 / ctx.zoom.max(0.01) as f64;
                    let (dx, dy) = (p.0 - anchor.0, p.1 - anchor.1);
                    let distance = dx.hypot(dy);
                    if distance <= radius {
                        return StrokeStep::None;
                    }
                    let k = (distance - radius) / distance;
                    p = (anchor.0 + dx * k, anchor.1 + dy * k);
                }
                self.anchor = Some(p);
                stroke.to(p);
                StrokeStep::Painted
            }
            Release => self.end(),
            _ => StrokeStep::None,
        }
    }

    /// Ends the stroke in progress, if any.
    pub fn end(&mut self) -> StrokeStep {
        self.anchor = None;
        let Some(mut stroke) = self.stroke.take() else { return StrokeStep::None };
        stroke.finish();
        self.last = Some((stroke.surface.layer, stroke.surface.mask, stroke.last));
        StrokeStep::Finished(Box::new(stroke))
    }
}

/// The foreground as a painting tool lays it: its gray on a mask.
pub fn paint_color(ctx: &ToolCtx, mask: bool) -> [u8; 4] {
    let c = ctx.colors.foreground;
    if mask {
        let g = gray_of(c);
        [g, g, g, c[3]]
    } else {
        c
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::doc::{Document, Selection};

    pub fn project(w: u32, h: u32, color: [u8; 4]) -> Project {
        Project::new(Document::new(w, h, Some(color)), None, "Test".into())
    }

    #[test]
    fn tip_fades_from_center_to_rim() {
        let hard = Tip::new(20.0, 1.0);
        assert_eq!(hard.coverage(0.0), 1.0);
        assert_eq!(hard.coverage(81.0), 1.0);
        assert!(hard.coverage(100.0) > 0.4 && hard.coverage(100.0) < 0.6, "anti-aliased rim");
        assert_eq!(hard.coverage(121.0), 0.0);
        let soft = Tip::new(20.0, 0.0);
        assert!((soft.coverage(0.0) - 1.0).abs() < 1e-3);
        let mut previous = 2.0;
        for d in 0..=11 {
            let c = soft.coverage((d * d) as f64);
            assert!(c <= previous, "soft tip decreases outward");
            previous = c;
        }
        assert!(soft.coverage(99.0) < 0.02);
    }

    #[test]
    fn walker_spaces_dabs_evenly() {
        let mut walker = Walker::default();
        let dabs = walker.line((0.0, 0.0), (100.0, 0.0), 10.0);
        assert_eq!(dabs.len(), 11);
        assert!((dabs[3].0 - 30.0).abs() < 1e-9);
        // Spacing carries across segments.
        let mut walker = Walker::default();
        let mut dabs = walker.start((0.0, 0.0));
        dabs.extend(walker.to((15.0, 0.0), 10.0));
        dabs.extend(walker.to((30.0, 0.0), 10.0));
        dabs.extend(walker.finish(10.0));
        let xs: Vec<f64> = dabs.iter().map(|d| d.0).collect();
        for pair in xs.windows(2) {
            assert!((pair[1] - pair[0] - 10.0).abs() < 1e-6, "{xs:?}");
        }
    }

    #[test]
    fn opacity_caps_overlapping_dabs() {
        let mut project = project(64, 64, [255, 255, 255, 255]);
        let settings = BrushSettings { size: 20.0, opacity: 0.5, ..Default::default() };
        let surface = Surface::begin(&mut project, "Brush", true).unwrap();
        let mut stroke = Stroke::new(surface, &settings);
        // Scrub back and forth over the same spot.
        stroke.start((32.0, 32.0));
        for i in 0..20 {
            stroke.to((if i % 2 == 0 { 40.0 } else { 24.0 }, 32.0));
        }
        stroke.finish();
        stroke.flush(&mut project, settings.opacity, &Paint::Color([0, 0, 0, 255]));
        let image = project.doc.layers[0].image.as_ref().unwrap();
        let p = image.get_pixel(32, 32);
        assert!((p[0] as i32 - 128).abs() <= 1, "half opacity, not built up: {p:?}");
        assert_eq!(image.get_pixel(2, 2)[0], 255);
    }

    #[test]
    fn selection_limits_paint_and_eraser_clears_alpha() {
        let mut project = project(40, 40, [200, 0, 0, 255]);
        let mut mask = GrayImage::new(40, 40);
        for y in 0..40 {
            for x in 0..20 {
                mask.put_pixel(x, y, Luma([255]));
            }
        }
        project.doc.selection = Selection::from_mask(mask);
        let surface = Surface::begin(&mut project, "Eraser", true).unwrap();
        let mut stroke = Stroke::new(surface, &BrushSettings { size: 40.0, ..Default::default() });
        stroke.start((20.0, 20.0));
        stroke.flush(&mut project, 1.0, &Paint::Erase);
        let image = project.doc.layers[0].image.as_ref().unwrap();
        assert_eq!(image.get_pixel(15, 20)[3], 0);
        assert_eq!(image.get_pixel(25, 20)[3], 255);
    }

    #[test]
    fn blank_layer_gets_pixels_and_rotated_layer_paints_in_place() {
        let mut project = Project::new(Document::new(50, 50, None), None, "Test".into());
        let surface = Surface::begin(&mut project, "Brush", true).unwrap();
        assert_eq!((surface.width, surface.height), (50, 50));
        drop(surface);
        project.finish_edit();

        // A 20×20 image placed rotated 90° and scaled to 40×40 around (25, 25).
        let layer = &mut project.doc.layers[0];
        layer.image = Some(Arc::new(RgbaImage::new(20, 20)));
        layer.transform = crate::doc::LayerTransform { rotation: 90.0, ..crate::doc::LayerTransform::rect(5.0, 5.0, 40.0, 40.0) };
        let surface = Surface::begin(&mut project, "Brush", true).unwrap();
        let mut stroke = Stroke::new(surface, &BrushSettings { size: 4.0, ..Default::default() });
        // Document (35, 15) is top-right; rotated 90° clockwise that is the image's top-left.
        stroke.start((35.0, 15.0));
        stroke.flush(&mut project, 1.0, &Paint::Color([0, 0, 255, 255]));
        let image = project.doc.layers[0].image.as_ref().unwrap();
        let (u, v) = stroke.surface.to_pixel.apply(35.0, 15.0);
        assert!(u < 10.0 && v < 10.0, "({u}, {v})");
        assert_eq!(image.get_pixel(u as u32, v as u32)[3], 255);
    }

    #[test]
    fn brush_paints_gray_on_masks() {
        let mut project = project(30, 30, [0, 0, 0, 255]);
        let id = project.doc.layers[0].id;
        crate::doc::ops::add_mask(&mut project.doc, id, false);
        project.target = EditTarget::Mask;
        let surface = Surface::begin(&mut project, "Brush", true).unwrap();
        let mut stroke = Stroke::new(surface, &BrushSettings { size: 10.0, ..Default::default() });
        stroke.start((15.0, 15.0));
        stroke.flush(&mut project, 1.0, &Paint::Erase);
        let Some(crate::doc::LayerMask { pixels: MaskPixels::Pixels(gray), .. }) = &project.doc.layers[0].mask else { panic!() };
        assert_eq!(gray.get_pixel(15, 15)[0], 0);
        assert_eq!(gray.get_pixel(1, 1)[0], 255);
    }

    #[test]
    fn over_and_mix() {
        assert_eq!(over([0, 0, 0, 0], [255, 0, 0, 255], 0.5), [255, 0, 0, 128]);
        assert_eq!(over([0, 0, 255, 255], [255, 0, 0, 255], 1.0), [255, 0, 0, 255]);
        assert_eq!(mix([255, 0, 0, 255], [0, 0, 0, 0], 1.0), [0, 0, 0, 0]);
        assert_eq!(mix([255, 0, 0, 255], [0, 0, 255, 255], 0.5)[3], 255);
    }

    #[test]
    fn bracket_keys_step_size_and_hardness() {
        let mut s = BrushSettings { size: 10.0, hardness: 0.8, ..Default::default() };
        assert!(s.key(Key::CloseBracket, Modifiers::NONE));
        assert_eq!(s.size, 12.0);
        assert!(s.key(Key::OpenBracket, Modifiers::NONE));
        assert_eq!(s.size, 10.0);
        s.key(Key::CloseBracket, Modifiers::SHIFT);
        assert_eq!(s.hardness, 1.0);
        s.key(Key::OpenBracket, Modifiers::SHIFT);
        assert_eq!(s.hardness, 0.75);
    }
}
