//! Select > Color Range: every pixel near the colors picked from the image, anywhere on the
//! canvas. Colors are picked by clicking the canvas or the dialog's thumbnail (Shift adds a
//! color, Alt takes one away); the selection updates live and OK keeps it as one undo step.

use std::sync::Arc;

use egui::{Color32, ColorImage, Sense, TextureHandle, TextureOptions};
use image::{GrayImage, RgbaImage};
use rayon::prelude::*;

use crate::doc::Selection;
use crate::project::Project;
use crate::render::RenderCache;
use crate::ui::dialogs::{ok_cancel, Dialog, DialogCtx, DialogState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickMode {
    Replace,
    Add,
    Remove,
}

/// The thumbnails fit in this, in points.
const THUMB: egui::Vec2 = egui::vec2(220.0, 160.0);

pub struct ColorRangeDialog {
    /// The image as shown, straight alpha at canvas size: what colors are matched against.
    image: Arc<RgbaImage>,
    /// The selection when the dialog opened, put back on Cancel.
    original: Option<Selection>,
    include: Vec<[u8; 3]>,
    exclude: Vec<[u8; 3]>,
    fuzziness: f32,
    invert: bool,
    mode: PickMode,
    result: Option<Selection>,
    thumbnail: Option<TextureHandle>,
    preview: Option<TextureHandle>,
    dirty: bool,
}

impl ColorRangeDialog {
    pub fn new(project: &Project) -> Self {
        let cache = RenderCache::default();
        let doc = &project.doc;
        let image = crate::render::composite(doc, crate::render::Region::full(doc), &cache).to_rgba8();
        ColorRangeDialog {
            image: Arc::new(image),
            original: doc.selection.clone(),
            include: Vec::new(),
            exclude: Vec::new(),
            fuzziness: 40.0,
            invert: false,
            mode: PickMode::Replace,
            result: None,
            thumbnail: None,
            preview: None,
            dirty: false,
        }
    }

    /// A click at document point `pos`; Shift adds the color and Alt takes it away, whichever
    /// eyedropper is chosen.
    fn pick(&mut self, pos: (f64, f64), modifiers: egui::Modifiers) {
        let (w, h) = self.image.dimensions();
        let (x, y) = (pos.0.floor(), pos.1.floor());
        if x < 0.0 || y < 0.0 || x >= w as f64 || y >= h as f64 {
            return;
        }
        let p = self.image.get_pixel(x as u32, y as u32);
        let color = [p[0], p[1], p[2]];
        let mode = if modifiers.alt {
            PickMode::Remove
        } else if modifiers.shift {
            PickMode::Add
        } else {
            self.mode
        };
        match mode {
            PickMode::Replace => {
                self.include = vec![color];
                self.exclude.clear();
            }
            PickMode::Add => self.include.push(color),
            PickMode::Remove => self.exclude.push(color),
        }
        self.dirty = true;
    }

    fn update(&mut self, ui: &egui::Ui, project: &mut Project) {
        self.dirty = false;
        if self.include.is_empty() {
            self.result = self.original.clone();
            self.preview = None;
        } else {
            let mask = range_mask(&self.image, &self.include, &self.exclude, self.fuzziness, self.invert);
            self.preview = Some(ui.ctx().load_texture("color-range-preview", gray_thumbnail(&mask), TextureOptions::LINEAR));
            self.result = Selection::from_mask(mask);
        }
        // Shown on the canvas without an undo step until OK.
        project.doc.selection = self.result.clone();
    }

    /// Takes clicks on the canvas: a transparent area over it, under the dialog, so the current
    /// tool doesn't see them.
    fn canvas_picker(&mut self, ui: &egui::Ui, project: &Project) {
        let viewport = project.view.viewport;
        if !viewport.is_positive() {
            return;
        }
        let ctx = ui.ctx().clone();
        let area = egui::Area::new(egui::Id::new("color-range-canvas"))
            .order(egui::Order::Middle)
            .fixed_pos(viewport.min)
            .show(&ctx, |ui| {
                let (_, response) = ui.allocate_exact_size(viewport.size(), Sense::click());
                let response = response.on_hover_cursor(egui::CursorIcon::Crosshair);
                if response.clicked() {
                    if let Some(p) = response.interact_pointer_pos() {
                        let view = project.view.transform(project.doc.width, project.doc.height);
                        let modifiers = ui.input(|i| i.modifiers);
                        return Some((view.to_doc(p), modifiers));
                    }
                }
                None
            });
        // Keep the dialog above the picker.
        ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, egui::Id::new("compositor-dialog")));
        if let Some((pos, modifiers)) = area.inner {
            self.pick(pos, modifiers);
        }
    }
}

impl Dialog for ColorRangeDialog {
    fn title(&self) -> String {
        "Color Range".into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut DialogCtx) -> DialogState {
        let Some(project) = ctx.project.as_deref_mut() else { return DialogState::Closed };
        if project.doc.width != self.image.width() || project.doc.height != self.image.height() {
            return DialogState::Closed;
        }
        self.canvas_picker(ui, project);
        use crate::ui::icons;
        ui.horizontal(|ui| {
            for (mode, label, hint) in [
                (PickMode::Replace, icons::EYEDROPPER.to_string(), "Click the image to select that color"),
                (PickMode::Add, format!("{} +", icons::EYEDROPPER), "Click to add a color (or Shift-click)"),
                (PickMode::Remove, format!("{} −", icons::EYEDROPPER), "Click to take a color away (or Alt-click)"),
            ] {
                ui.selectable_value(&mut self.mode, mode, label).on_hover_text(hint);
            }
        });
        ui.horizontal(|ui| {
            let thumbnail = self.thumbnail.get_or_insert_with(|| {
                ui.ctx().load_texture("color-range-image", rgba_thumbnail(&self.image), TextureOptions::LINEAR)
            });
            let size = fit(thumbnail.size_vec2(), THUMB);
            let response = ui
                .add(egui::Image::new(&*thumbnail).fit_to_exact_size(size).sense(Sense::click()))
                .on_hover_cursor(egui::CursorIcon::Crosshair);
            if response.clicked() {
                if let Some(p) = response.interact_pointer_pos() {
                    let u = ((p.x - response.rect.min.x) / response.rect.width()) as f64;
                    let v = ((p.y - response.rect.min.y) / response.rect.height()) as f64;
                    let modifiers = ui.input(|i| i.modifiers);
                    self.pick((u * self.image.width() as f64, v * self.image.height() as f64), modifiers);
                }
            }
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
            if let Some(preview) = &self.preview {
                ui.painter().image(preview.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
        });
        ui.label(if self.include.is_empty() {
            "Click the canvas or the image to pick the color to select."
        } else {
            "Shift-click adds a color, Alt-click takes one away."
        });
        ui.horizontal(|ui| {
            ui.label("Fuzziness");
            if ui.add(egui::Slider::new(&mut self.fuzziness, 0.0..=200.0).step_by(1.0)).changed() {
                self.dirty = true;
            }
        });
        if ui.checkbox(&mut self.invert, "Invert").changed() {
            self.dirty = true;
        }
        if self.dirty {
            self.update(ui, project);
        }
        match ok_cancel(ui, "OK") {
            Some(true) => {
                if self.dirty {
                    self.update(ui, project);
                }
                project.doc.selection = self.original.clone();
                if !self.include.is_empty() {
                    let result = self.result.clone();
                    project.edit("Color Range", |doc| doc.selection = result);
                }
                DialogState::Closed
            }
            Some(false) => {
                self.cancel(ctx);
                DialogState::Closed
            }
            None => DialogState::Open,
        }
    }

    fn cancel(&mut self, ctx: &mut DialogCtx) {
        if let Some(project) = ctx.project.as_deref_mut() {
            if project.doc.width == self.image.width() && project.doc.height == self.image.height() {
                project.doc.selection = self.original.clone();
            }
        }
    }
}

/// `size` scaled down to fit in `bounds`, keeping its proportions.
fn fit(size: egui::Vec2, bounds: egui::Vec2) -> egui::Vec2 {
    let scale = (bounds.x / size.x.max(1.0)).min(bounds.y / size.y.max(1.0)).min(1.0);
    size * scale
}

/// How near `rgb` is to any of `colors`: 1 within half the fuzziness on every channel, fading to
/// 0 at the full fuzziness.
fn nearness(rgb: [u8; 3], colors: &[[u8; 3]], fuzziness: f32) -> f32 {
    let mut best = 0f32;
    for c in colors {
        let d = (0..3).map(|i| (rgb[i] as i32 - c[i] as i32).abs()).max().unwrap_or(0) as f32;
        let v = if d <= fuzziness * 0.5 {
            1.0
        } else if d <= fuzziness {
            (fuzziness - d) / (fuzziness * 0.5)
        } else {
            0.0
        };
        best = best.max(v);
        if best >= 1.0 {
            break;
        }
    }
    best
}

/// The canvas-sized selection for the picked colors; transparent pixels are never selected
/// (unless inverted).
pub fn range_mask(image: &RgbaImage, include: &[[u8; 3]], exclude: &[[u8; 3]], fuzziness: f32, invert: bool) -> GrayImage {
    let (w, h) = image.dimensions();
    let mut mask = GrayImage::new(w, h);
    let raw: &mut [u8] = &mut mask;
    raw.par_chunks_mut(w as usize).zip(image.as_raw().par_chunks(w as usize * 4)).for_each(|(out, row)| {
        for (o, p) in out.iter_mut().zip(row.chunks_exact(4)) {
            let mut v = 0.0;
            if p[3] > 0 {
                let rgb = [p[0], p[1], p[2]];
                v = nearness(rgb, include, fuzziness);
                if v > 0.0 && !exclude.is_empty() {
                    v *= 1.0 - nearness(rgb, exclude, fuzziness);
                }
            }
            if invert {
                v = 1.0 - v;
            }
            *o = (v * 255.0).round() as u8;
        }
    });
    mask
}

fn rgba_thumbnail(image: &RgbaImage) -> ColorImage {
    let (w, h) = image.dimensions();
    let size = fit(egui::vec2(w as f32, h as f32), THUMB * 2.0);
    let small = image::imageops::thumbnail(image, (size.x.round() as u32).max(1), (size.y.round() as u32).max(1));
    ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw())
}

fn gray_thumbnail(mask: &GrayImage) -> ColorImage {
    let (w, h) = mask.dimensions();
    let size = fit(egui::vec2(w as f32, h as f32), THUMB * 2.0);
    let (tw, th) = ((size.x.round() as u32).max(1), (size.y.round() as u32).max(1));
    let small = image::imageops::thumbnail(mask, tw, th);
    ColorImage::from_gray([small.width() as usize, small.height() as usize], small.as_raw())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn picks_near_colors_softly() {
        let image = RgbaImage::from_fn(5, 1, |x, _| Rgba([100 + x as u8 * 10, 0, 0, 255]));
        let mask = range_mask(&image, &[[100, 0, 0]], &[], 40.0, false);
        let values: Vec<u8> = mask.pixels().map(|p| p[0]).collect();
        assert_eq!(values, vec![255, 255, 255, 128, 0]);
        let inverted = range_mask(&image, &[[100, 0, 0]], &[], 40.0, true);
        assert_eq!(inverted.get_pixel(4, 0)[0], 255);
        let excluded = range_mask(&image, &[[100, 0, 0]], &[[120, 0, 0]], 10.0, false);
        assert_eq!(excluded.get_pixel(0, 0)[0], 255);
        assert_eq!(excluded.get_pixel(2, 0)[0], 0);
    }
}
