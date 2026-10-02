//! Clone Stamp: Alt-click sets the source, then strokes paint pixels copied from there. Aligned,
//! the source keeps its offset from stroke to stroke; otherwise each stroke starts copying from
//! the source point again. It samples the layer itself or all visible layers, as they were when
//! the stroke began.

use std::collections::HashMap;

use egui::{CursorIcon, Key, Modifiers, Painter};
use rayon::prelude::*;

use super::paint::{self, Band, BrushSettings, Fields, Original, Paint, Stroke, StrokeDriver, StrokeStep, TILE};
use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::doc::Document;
use crate::project::Project;
use crate::render::{self, Region, RenderCache};
use crate::ui::canvas::ViewTransform;

pub struct CloneStampTool {
    pub settings: BrushSettings,
    pub aligned: bool,
    pub all_layers: bool,
    /// Where Alt-click set the source (document pixels).
    source: Option<(f64, f64)>,
    /// Source minus destination, fixed by the first aligned stroke after the source was set.
    offset: Option<(f64, f64)>,
    /// The offset of the stroke in progress.
    stroke_offset: (f64, f64),
    driver: StrokeDriver,
    /// The document as the stroke began, for sampling all layers.
    snapshot: Option<Document>,
    /// Merged source pixels per stroke tile, resolved to layer pixels.
    samples: HashMap<(u32, u32), Vec<[u8; 4]>>,
}

impl Default for CloneStampTool {
    fn default() -> Self {
        CloneStampTool {
            settings: BrushSettings { size: 60.0, hardness: 0.5, ..Default::default() },
            aligned: true,
            all_layers: false,
            source: None,
            offset: None,
            stroke_offset: (0.0, 0.0),
            driver: StrokeDriver::default(),
            snapshot: None,
            samples: HashMap::new(),
        }
    }
}

/// The merged image under one stroke tile, read at each of its layer pixels shifted by `offset`.
fn sample_tile(stroke: &Stroke, snapshot: &Document, cache: &RenderCache, key: (u32, u32), offset: (f64, f64)) -> Vec<[u8; 4]> {
    let surface = &stroke.surface;
    let (x0, y0) = (key.0 * TILE, key.1 * TILE);
    let (x1, y1) = ((x0 + TILE).min(surface.width), (y0 + TILE).min(surface.height));
    let r = surface.doc_rect((x0, y0, x1, y1));
    let region = Region {
        x: r.0 + offset.0.floor() as i64,
        y: r.1 + offset.1.floor() as i64,
        width: (r.2 - r.0 + 2) as usize,
        height: (r.3 - r.1 + 2) as usize,
        scale: 1.0,
    };
    let merged = render::composite(snapshot, region, cache).to_rgba8();
    let mut out = vec![[0u8; 4]; (TILE * TILE) as usize];
    for y in y0..y1 {
        for x in x0..x1 {
            let (dx, dy) = surface.to_doc.apply(x as f64 + 0.5, y as f64 + 0.5);
            let sx = (dx + offset.0).floor() as i64 - region.x;
            let sy = (dy + offset.1).floor() as i64 - region.y;
            if sx >= 0 && sy >= 0 && (sx as u32) < merged.width() && (sy as u32) < merged.height() {
                out[((y - y0) * TILE + x - x0) as usize] = merged.get_pixel(sx as u32, sy as u32).0;
            }
        }
    }
    out
}

impl CloneStampTool {
    fn flush(&mut self, ctx: &mut ToolCtx, stroke: &mut Stroke) {
        let bands = stroke.take_dirty();
        if bands.is_empty() {
            return;
        }
        let offset = self.stroke_offset;
        if self.all_layers {
            if let Some(snapshot) = &self.snapshot {
                let missing: Vec<(u32, u32)> = bands
                    .iter()
                    .flat_map(|b: &Band| {
                        let ty = b.rows.0 / TILE;
                        b.spans.iter().flat_map(move |&(x0, x1)| (x0 / TILE..x1.div_ceil(TILE)).map(move |tx| (tx, ty)))
                    })
                    .filter(|key| !self.samples.contains_key(key))
                    .collect();
                let cache = ctx.cache;
                let made: Vec<_> =
                    missing.into_par_iter().map(|key| (key, sample_tile(stroke, snapshot, cache, key, offset))).collect();
                self.samples.extend(made);
            }
            let samples = &self.samples;
            let source = move |x: u32, y: u32| {
                samples.get(&(x / TILE, y / TILE)).map_or([0; 4], |tile| tile[((y % TILE) * TILE + x % TILE) as usize])
            };
            stroke.composite(ctx.project, &bands, self.settings.opacity, &Paint::Over(&source));
        } else {
            let Original::Rgba(image) = &stroke.surface.original else { return };
            let image = image.clone();
            let (w, h) = (stroke.surface.width as f64, stroke.surface.height as f64);
            let (px, py) = stroke.surface.to_pixel.apply_vector(offset.0, offset.1);
            let to_doc = stroke.surface.to_doc;
            let to_pixel = stroke.surface.to_pixel;
            let source = move |x: u32, y: u32| {
                // Shift in layer pixels; on a rotated layer, go through the document.
                let (sx, sy) = if to_doc.b == 0.0 && to_doc.c == 0.0 {
                    (x as f64 + 0.5 + px, y as f64 + 0.5 + py)
                } else {
                    let (dx, dy) = to_doc.apply(x as f64 + 0.5, y as f64 + 0.5);
                    to_pixel.apply(dx + offset.0, dy + offset.1)
                };
                if sx < 0.0 || sy < 0.0 || sx >= w || sy >= h {
                    [0; 4]
                } else {
                    image.get_pixel(sx as u32, sy as u32).0
                }
            };
            stroke.composite(ctx.project, &bands, self.settings.opacity, &Paint::Over(&source));
        }
    }

    fn handle(&mut self, step: StrokeStep, ctx: &mut ToolCtx) {
        match step {
            StrokeStep::None => {}
            StrokeStep::Painted => {
                let Some(mut stroke) = self.driver.stroke.take() else { return };
                self.flush(ctx, &mut stroke);
                self.driver.stroke = Some(stroke);
            }
            StrokeStep::Finished(mut stroke) => {
                self.flush(ctx, &mut stroke);
                ctx.project.finish_edit();
                self.snapshot = None;
                self.samples.clear();
            }
        }
    }

    /// Where the source sits for a stroke at `pos`.
    fn source_for(&self, pos: (f64, f64)) -> Option<(f64, f64)> {
        let source = self.source?;
        Some(match (self.aligned, self.offset) {
            (true, Some(o)) => (pos.0 + o.0, pos.1 + o.1),
            _ if self.driver.stroke.is_some() => (pos.0 + self.stroke_offset.0, pos.1 + self.stroke_offset.1),
            _ => source,
        })
    }
}

impl Tool for CloneStampTool {
    fn kind(&self) -> ToolKind {
        ToolKind::CloneStamp
    }
    fn name(&self) -> &'static str {
        "Clone Stamp"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::STAMP
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::S)
    }
    fn options_ui(&mut self, ui: &mut egui::Ui, _ctx: &mut ToolCtx) {
        paint::options_ui(ui, &mut self.settings, Fields::ALL);
        ui.separator();
        ui.checkbox(&mut self.aligned, "Aligned").on_hover_text("Keep the source's offset from stroke to stroke");
        egui::ComboBox::from_label("Sample")
            .selected_text(if self.all_layers { "All Layers" } else { "Current Layer" })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.all_layers, false, "Current Layer");
                ui.selectable_value(&mut self.all_layers, true, "All Layers");
            });
    }
    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        if event.phase == PointerPhase::Press && self.driver.stroke.is_none() {
            if event.modifiers.alt {
                self.source = Some(event.pos);
                self.offset = None;
                *ctx.status = Some("Clone source set".into());
                return;
            }
            let Some(source) = self.source else {
                *ctx.status = Some("Alt-click to set where Clone Stamp copies from.".into());
                return;
            };
            let fresh = (source.0 - event.pos.0, source.1 - event.pos.1);
            self.stroke_offset = if self.aligned { *self.offset.get_or_insert(fresh) } else { fresh };
            self.snapshot = self.all_layers.then(|| ctx.project.doc.clone());
            self.samples.clear();
        }
        let step = self.driver.pointer(event, ctx, &self.settings, "Clone Stamp", false);
        if event.phase == PointerPhase::Press && self.driver.stroke.is_none() {
            self.snapshot = None;
        }
        self.handle(step, ctx);
    }
    fn key(&mut self, key: Key, modifiers: Modifiers, _ctx: &mut ToolCtx) -> bool {
        self.driver.stroke.is_none() && self.settings.key(key, modifiers)
    }
    fn overlay(&self, painter: &Painter, view: &ViewTransform, _project: &Project, hover: Option<(f64, f64)>) {
        let Some(pos) = hover else { return };
        paint::draw_outline(painter, view, pos, self.settings.size);
        // Where the copy comes from, following the brush.
        let at = self.driver.anchor.unwrap_or(pos);
        if let Some(source) = self.source_for(at) {
            let center = view.to_screen(source.0, source.1);
            paint::draw_crosshair(painter, center, 6.0);
            if self.settings.size * view.zoom >= 6.0 {
                painter.circle_stroke(center, self.settings.size * view.zoom / 2.0, egui::Stroke::new(1.0, egui::Color32::from_white_alpha(90)));
            }
        }
    }
    fn cursor(&self, project: &Project, _hover: (f64, f64), modifiers: Modifiers) -> CursorIcon {
        if modifiers.alt {
            CursorIcon::Crosshair
        } else {
            paint::cursor(self.settings.size, project.view.zoom)
        }
    }
    fn commit(&mut self, ctx: &mut ToolCtx) {
        let step = self.driver.end();
        self.handle(step, ctx);
    }
    fn cancel(&mut self, ctx: &mut ToolCtx) {
        self.commit(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::paint::tests::project;
    use crate::tools::{Colors, PointerEvent};
    use image::Rgba;

    fn event(phase: PointerPhase, pos: (f64, f64), modifiers: Modifiers) -> PointerEvent {
        PointerEvent { phase, pos, press_origin: pos, modifiers, pressure: 1.0 }
    }

    #[test]
    fn copies_from_the_source_offset() {
        let mut project = project(100, 40, [255, 255, 255, 255]);
        {
            let layer = &mut project.doc.layers[0];
            let image = std::sync::Arc::make_mut(layer.image.as_mut().unwrap());
            for y in 10..30 {
                for x in 10..30 {
                    image.put_pixel(x, y, Rgba([255, 0, 0, 255]));
                }
            }
        }
        let cache = RenderCache::default();
        for all_layers in [false, true] {
            let mut colors = Colors::default();
            let mut status = None;
            let mut p = project_clone(&project);
            let mut ctx = ToolCtx { project: &mut p, colors: &mut colors, cache: &cache, zoom: 1.0, status: &mut status };
            let mut tool = CloneStampTool { all_layers, ..Default::default() };
            tool.settings.hardness = 1.0;
            tool.settings.size = 10.0;
            tool.pointer(&event(PointerPhase::Press, (20.0, 20.0), Modifiers::ALT), &mut ctx);
            tool.pointer(&event(PointerPhase::Press, (70.0, 20.0), Modifiers::NONE), &mut ctx);
            tool.pointer(&event(PointerPhase::Release, (70.0, 20.0), Modifiers::NONE), &mut ctx);
            let image = p.doc.layers[0].image.as_ref().unwrap();
            assert_eq!(image.get_pixel(70, 20).0, [255, 0, 0, 255], "all layers: {all_layers}");
            assert_eq!(image.get_pixel(90, 20).0, [255, 255, 255, 255]);
        }
    }

    fn project_clone(p: &Project) -> Project {
        Project::new(p.doc.clone(), None, "Test".into())
    }
}
