//! The Layers panel: blend mode and opacity for the active layer, the layer stack (top first)
//! with folders, masks and clipping, drag-and-drop reordering, and the buttons along the bottom.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Color32, RichText, Sense, TextureHandle, TextureOptions, Vec2};
use image::RgbaImage;

use crate::doc::ops::{self, Placement};
use crate::doc::{BlendMode, Document, Id, Layer, MaskPixels};
use crate::project::{EditTarget, Project};
use crate::render::RenderCache;
use crate::ui::icons;

const THUMB: f32 = 30.0;

#[derive(Default)]
pub struct LayersPanel {
    thumbnails: HashMap<usize, TextureHandle>,
    renaming: Option<(Id, String)>,
    opacity_drag: Option<f32>,
}

/// What a row asks for; applied after drawing so the borrow of the document ends first.
enum Action {
    Select(Id, bool, bool),
    ToggleVisible(Id),
    ToggleExpanded(Id),
    Rename(Id, String),
    Move(Vec<Id>, Id, Placement),
    SelectMask(Id),
    SelectImage(Id),
    ToggleMaskEnabled(Id),
    ToggleClip(Id),
    Delete(Id),
    Duplicate(Id),
    MergeDown,
    ApplyMask(Id),
    DeleteMask(Id),
    Rasterize(Id),
    Group,
    Ungroup(Id),
}

#[derive(Clone)]
struct DragPayload(Vec<Id>);

impl LayersPanel {
    pub fn show(&mut self, ui: &mut egui::Ui, project: &mut Project, cache: &RenderCache, open_adjustment: &mut Option<crate::adjust::AdjustmentKind>) {
        self.header(ui, project);
        ui.separator();
        let mut actions = Vec::new();
        let available = ui.available_height() - 34.0;
        egui::ScrollArea::vertical().max_height(available.max(80.0)).auto_shrink([false, false]).show(ui, |ui| {
            let rows = visible_rows(&project.doc);
            for (id, depth) in rows {
                self.row(ui, project, id, depth, &mut actions);
            }
        });
        ui.separator();
        self.footer(ui, project, &mut actions, open_adjustment);
        self.apply(project, actions, cache);
        // Forget thumbnails of images that are gone.
        if self.thumbnails.len() > project.doc.layers.len() * 3 + 32 {
            self.thumbnails.clear();
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, project: &mut Project) {
        let Some(layer) = project.doc.active_layer().cloned() else { return };
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!layer.is_group, |ui| {
                let mut blend = layer.blend;
                egui::ComboBox::from_id_salt("blend-mode").width(130.0).selected_text(blend.name()).show_ui(ui, |ui| {
                    for (gi, group) in BlendMode::GROUPS.iter().enumerate() {
                        if gi > 0 {
                            ui.separator();
                        }
                        for mode in group.iter() {
                            ui.selectable_value(&mut blend, *mode, mode.name());
                        }
                    }
                });
                if blend != layer.blend {
                    project.edit("Blend Mode", |doc| {
                        if let Some(l) = doc.active_layer_mut() {
                            l.blend = blend;
                        }
                    });
                }
            });
            ui.label("Opacity");
            let mut opacity = self.opacity_drag.unwrap_or(layer.opacity * 100.0);
            let response = ui.add(egui::DragValue::new(&mut opacity).range(0.0..=100.0).suffix("%").speed(0.5));
            if response.drag_started() {
                project.begin_edit("Opacity");
            }
            if response.changed() {
                if project.pending_edit.is_none() {
                    project.history.push("Opacity", project.doc.clone());
                }
                if let Some(l) = project.doc.active_layer_mut() {
                    l.opacity = (opacity / 100.0).clamp(0.0, 1.0);
                }
                project.invalidate_all();
            }
            if response.drag_stopped() {
                project.finish_edit();
            }
            self.opacity_drag = None;
        });
    }

    fn row(&mut self, ui: &mut egui::Ui, project: &Project, id: Id, depth: usize, actions: &mut Vec<Action>) {
        let doc = &project.doc;
        let Some(layer) = doc.layer(id) else { return };
        let selected = doc.selected_ids().contains(&id);
        let active = doc.active == Some(id);
        let row_height = THUMB + 6.0;
        let fill = if selected { ui.visuals().selection.bg_fill.linear_multiply(if active { 1.0 } else { 0.6 }) } else { Color32::TRANSPARENT };

        let payload_ids = if selected { doc.selected_ids() } else { vec![id] };
        let row = ui.dnd_drag_source(egui::Id::new(("layer-row", id)), DragPayload(payload_ids), |ui| {
            egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(2, 1)).show(ui, |ui| {
                ui.set_min_height(row_height);
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let eye = if layer.visible { icons::EYE } else { icons::EYE_SLASH };
                    if ui.add(egui::Button::new(eye).frame(false).min_size(Vec2::new(18.0, THUMB))).on_hover_text("Show/Hide").clicked() {
                        actions.push(Action::ToggleVisible(id));
                    }
                    ui.add_space(depth as f32 * 14.0);
                    if layer.clip_source.is_some() {
                        ui.label(RichText::new(icons::ARROW_ELBOW_DOWN_RIGHT).strong()).on_hover_text("Clipped to the layer below");
                    }
                    if layer.is_group {
                        let arrow = if layer.expanded { icons::CARET_DOWN } else { icons::CARET_RIGHT };
                        if ui.add(egui::Button::new(arrow).frame(false)).clicked() {
                            actions.push(Action::ToggleExpanded(id));
                        }
                        ui.label(RichText::new(icons::FOLDER_SIMPLE).size(18.0));
                    } else if let Some(adj) = &layer.adjustment {
                        let r = ui.add(egui::Button::new(RichText::new(crate::adjust::symbol(adj.kind)).size(16.0)).min_size(Vec2::splat(THUMB)));
                        if r.clicked() {
                            actions.push(Action::Select(id, false, false));
                        }
                    } else {
                        let thumb = self.thumbnail(ui, layer);
                        let mut image = egui::Image::new(&thumb).fit_to_exact_size(Vec2::splat(THUMB)).sense(Sense::click());
                        if layer.mask.is_some() && project.target == EditTarget::Image && active {
                            image = image.bg_fill(ui.visuals().selection.stroke.color);
                        }
                        if ui.add(image).clicked() {
                            actions.push(Action::SelectImage(id));
                        }
                    }
                    if let Some(mask) = &layer.mask {
                        let tex = self.mask_thumbnail(ui, &mask.pixels, layer);
                        let mut image = egui::Image::new(&tex).fit_to_exact_size(Vec2::splat(THUMB)).sense(Sense::click());
                        if project.target == EditTarget::Mask && active {
                            image = image.bg_fill(ui.visuals().selection.stroke.color);
                        }
                        let r = ui.add(image).on_hover_text("Layer mask — Shift-click to disable");
                        if r.clicked() {
                            if ui.input(|i| i.modifiers.shift) {
                                actions.push(Action::ToggleMaskEnabled(id));
                            } else {
                                actions.push(Action::SelectMask(id));
                            }
                        }
                        if !mask.enabled {
                            ui.painter().line_segment([r.rect.left_top(), r.rect.right_bottom()], egui::Stroke::new(2.0, Color32::RED));
                        }
                    }
                    match &mut self.renaming {
                        Some((rid, text)) if *rid == id => {
                            let r = ui.text_edit_singleline(text);
                            r.request_focus();
                            if r.lost_focus() {
                                let name = text.trim().to_string();
                                if !name.is_empty() {
                                    actions.push(Action::Rename(id, name));
                                }
                                self.renaming = None;
                            }
                        }
                        _ => {
                            let mut text = RichText::new(&layer.name);
                            if layer.is_text() {
                                text = text.italics();
                            }
                            if !doc.is_effectively_visible(id) {
                                text = text.weak();
                            }
                            let r = ui.add(egui::Label::new(text).truncate().sense(Sense::click()));
                            if r.double_clicked() {
                                self.renaming = Some((id, layer.name.clone()));
                            } else if r.clicked() {
                                let m = ui.input(|i| i.modifiers);
                                actions.push(Action::Select(id, m.command || m.ctrl, m.shift));
                            }
                            if layer.effects.as_ref().is_some_and(|e| !crate::effects::is_empty(e)) {
                                ui.label(RichText::new("fx").small().italics());
                            }
                        }
                    }
                });
            })
            .response
        });
        let response = row.response.interact(Sense::click());
        if response.clicked() {
            let m = ui.input(|i| i.modifiers);
            if m.alt {
                actions.push(Action::ToggleClip(id));
            } else {
                actions.push(Action::Select(id, m.command || m.ctrl, m.shift));
            }
        }
        response.context_menu(|ui| {
            if ui.button("Duplicate Layer").clicked() {
                actions.push(Action::Duplicate(id));
                ui.close();
            }
            if ui.button("Delete Layer").clicked() {
                actions.push(Action::Delete(id));
                ui.close();
            }
            if ui.button("Rename…").clicked() {
                self.renaming = Some((id, layer.name.clone()));
                ui.close();
            }
            ui.separator();
            if ui.button("Group Layers").clicked() {
                actions.push(Action::Select(id, false, false));
                actions.push(Action::Group);
                ui.close();
            }
            if layer.is_group && ui.button("Ungroup").clicked() {
                actions.push(Action::Ungroup(id));
                ui.close();
            }
            if ui.button(if layer.clip_source.is_some() { "Release Clipping Mask" } else { "Create Clipping Mask" }).clicked() {
                actions.push(Action::ToggleClip(id));
                ui.close();
            }
            if ui.button("Merge Down").clicked() {
                actions.push(Action::Select(id, false, false));
                actions.push(Action::MergeDown);
                ui.close();
            }
            if layer.mask.is_some() {
                ui.separator();
                if ui.button("Apply Layer Mask").clicked() {
                    actions.push(Action::ApplyMask(id));
                    ui.close();
                }
                if ui.button("Delete Layer Mask").clicked() {
                    actions.push(Action::DeleteMask(id));
                    ui.close();
                }
            }
            if layer.is_text() || layer.shape.is_some() {
                ui.separator();
                if ui.button("Rasterize Layer").clicked() {
                    actions.push(Action::Rasterize(id));
                    ui.close();
                }
            }
        });

        // Dropping: top third above, bottom third below, middle of a folder into it.
        if let (Some(pointer), Some(payload)) = (ui.input(|i| i.pointer.interact_pos()), response.dnd_hover_payload::<DragPayload>()) {
            let rect = response.rect;
            let t = (pointer.y - rect.top()) / rect.height();
            let placement = if layer.is_group && (0.3..0.7).contains(&t) {
                Placement::Into
            } else if t < 0.5 {
                Placement::Above
            } else {
                Placement::Below
            };
            let stroke = egui::Stroke::new(2.0, ui.visuals().selection.stroke.color);
            match placement {
                Placement::Above => {
                    ui.painter().hline(rect.x_range(), rect.top(), stroke);
                }
                Placement::Below => {
                    ui.painter().hline(rect.x_range(), rect.bottom(), stroke);
                }
                Placement::Into => {
                    ui.painter().rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
                }
            }
            if response.dnd_release_payload::<DragPayload>().is_some() {
                actions.push(Action::Move(payload.0.clone(), id, placement));
            }
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui, project: &mut Project, actions: &mut Vec<Action>, open_adjustment: &mut Option<crate::adjust::AdjustmentKind>) {
        ui.horizontal(|ui| {
            let active = project.doc.active;
            if ui.button(icons::PLUS).on_hover_text("New Layer").clicked() {
                new_layer(project);
            }
            if ui.button(icons::FOLDER_PLUS).on_hover_text("New Group from selected layers").clicked() {
                actions.push(Action::Group);
            }
            if ui.button(icons::SQUARE_HALF).on_hover_text("Add Layer Mask (Alt: hide all)").clicked() {
                let hide = ui.input(|i| i.modifiers.alt);
                if let Some(id) = active {
                    project.edit("Add Mask", |doc| ops::add_mask(doc, id, hide));
                    project.target = EditTarget::Mask;
                }
            }
            ui.menu_button(icons::CIRCLE_HALF, |ui| {
                for kind in crate::adjust::MENU_KINDS {
                    if ui.button(crate::adjust::kind_name(*kind)).clicked() {
                        *open_adjustment = Some(*kind);
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("New Adjustment Layer");
            if ui.button(icons::TRASH).on_hover_text("Delete Layer").clicked() {
                if let Some(id) = active {
                    actions.push(Action::Delete(id));
                }
            }
        });
    }

    fn apply(&mut self, project: &mut Project, actions: Vec<Action>, cache: &RenderCache) {
        for action in actions {
            match action {
                Action::Select(id, toggle, range) => select(&mut project.doc, id, toggle, range, &mut project.target),
                Action::ToggleVisible(id) => {
                    project.edit("Visibility", |doc| {
                        if let Some(l) = doc.layer_mut(id) {
                            l.visible = !l.visible;
                        }
                    });
                }
                Action::ToggleExpanded(id) => {
                    if let Some(l) = project.doc.layer_mut(id) {
                        l.expanded = !l.expanded;
                    }
                }
                Action::Rename(id, name) => {
                    if project.doc.layer(id).is_some_and(|l| l.name != name) {
                        project.edit("Rename Layer", |doc| doc.layer_mut(id).unwrap().name = name);
                    }
                }
                Action::Move(ids, target, placement) => {
                    let mut moved = project.doc.clone();
                    if ops::move_layers(&mut moved, &ids, target, placement) {
                        project.edit("Move Layers", |doc| *doc = moved);
                    }
                }
                Action::SelectMask(id) => {
                    select(&mut project.doc, id, false, false, &mut project.target);
                    project.target = EditTarget::Mask;
                }
                Action::SelectImage(id) => {
                    select(&mut project.doc, id, false, false, &mut project.target);
                    project.target = EditTarget::Image;
                }
                Action::ToggleMaskEnabled(id) => project.edit("Disable Mask", |doc| {
                    if let Some(m) = doc.layer_mut(id).and_then(|l| l.mask.as_mut()) {
                        m.enabled = !m.enabled;
                    }
                }),
                Action::ToggleClip(id) => {
                    let mut doc = project.doc.clone();
                    if ops::toggle_clip(&mut doc, id) {
                        project.edit("Clipping Mask", |d| *d = doc);
                    }
                }
                Action::Delete(id) => {
                    let ids = if project.doc.selected_ids().contains(&id) { project.doc.selected_ids() } else { vec![id] };
                    project.edit("Delete Layer", |doc| doc.remove_layers(&ids));
                }
                Action::Duplicate(id) => {
                    let ids = if project.doc.selected_ids().contains(&id) { project.doc.selected_ids() } else { vec![id] };
                    project.edit("Duplicate Layer", |doc| ops::duplicate_layers(doc, &ids));
                }
                Action::MergeDown => {
                    let mut doc = project.doc.clone();
                    if ops::merge_down(&mut doc, cache) {
                        project.edit("Merge Down", |d| *d = doc);
                    }
                }
                Action::ApplyMask(id) => project.edit("Apply Layer Mask", |doc| ops::apply_mask(doc, id)).then_some(()).unwrap_or(()),
                Action::DeleteMask(id) => {
                    project.edit("Delete Layer Mask", |doc| doc.layer_mut(id).map(|l| l.mask = None));
                    project.target = EditTarget::Image;
                }
                Action::Rasterize(id) => {
                    project.edit("Rasterize Layer", |doc| ops::rasterize(doc, id));
                }
                Action::Group => {
                    let ids = project.doc.selected_ids();
                    project.edit("Group Layers", |doc| ops::group_layers(doc, &ids));
                }
                Action::Ungroup(id) => {
                    project.edit("Ungroup", |doc| ops::ungroup(doc, id));
                }
            }
        }
    }

    fn thumbnail(&mut self, ui: &egui::Ui, layer: &Layer) -> TextureHandle {
        let key = layer.image.as_ref().map(|i| Arc::as_ptr(i) as usize).unwrap_or(0);
        if let Some(t) = self.thumbnails.get(&key) {
            return t.clone();
        }
        let image = match &layer.image {
            Some(image) => thumbnail_image(image),
            None => egui::ColorImage::new([4, 4], vec![Color32::from_gray(200); 16]),
        };
        let tex = ui.ctx().load_texture(format!("thumb-{key}"), image, TextureOptions::LINEAR);
        self.thumbnails.insert(key, tex.clone());
        tex
    }

    fn mask_thumbnail(&mut self, ui: &egui::Ui, pixels: &MaskPixels, _layer: &Layer) -> TextureHandle {
        let (key, image) = match pixels {
            MaskPixels::Uniform(v) => (1_000_000 + *v as usize, egui::ColorImage::new([4, 4], vec![Color32::from_gray(*v); 16])),
            MaskPixels::Pixels(gray) => {
                let key = Arc::as_ptr(gray) as usize;
                if let Some(t) = self.thumbnails.get(&key) {
                    return t.clone();
                }
                let (w, h) = gray.dimensions();
                let scale = (64.0 / w.max(h) as f32).min(1.0);
                let small = image::imageops::thumbnail(&**gray, ((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
                let pixels = small.pixels().map(|p| Color32::from_gray(p[0])).collect();
                (key, egui::ColorImage::new([small.width() as usize, small.height() as usize], pixels))
            }
        };
        if let Some(t) = self.thumbnails.get(&key) {
            return t.clone();
        }
        let tex = ui.ctx().load_texture(format!("mask-{key}"), image, TextureOptions::LINEAR);
        self.thumbnails.insert(key, tex.clone());
        tex
    }
}

fn thumbnail_image(image: &RgbaImage) -> egui::ColorImage {
    let (w, h) = image.dimensions();
    let scale = (64.0 / w.max(h) as f32).min(1.0);
    let small = image::imageops::thumbnail(image, ((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
    let (sw, sh) = small.dimensions();
    let mut pixels = Vec::with_capacity((sw * sh) as usize);
    for (x, y, p) in small.enumerate_pixels() {
        let check = if ((x / 4) + (y / 4)) % 2 == 0 { 255.0 } else { 204.0 };
        let a = p[3] as f32 / 255.0;
        let mix = |c: u8| (c as f32 * a + check * (1.0 - a)) as u8;
        pixels.push(Color32::from_rgb(mix(p[0]), mix(p[1]), mix(p[2])));
    }
    egui::ColorImage::new([sw as usize, sh as usize], pixels)
}

/// Rows top to bottom with their indent depth, skipping the insides of collapsed folders.
fn visible_rows(doc: &Document) -> Vec<(Id, usize)> {
    fn visit(doc: &Document, parent: Option<Id>, depth: usize, out: &mut Vec<(Id, usize)>) {
        let children: Vec<&Layer> = doc.layers.iter().filter(|l| l.parent == parent).collect();
        for layer in children.into_iter().rev() {
            out.push((layer.id, depth));
            if layer.is_group && layer.expanded {
                visit(doc, Some(layer.id), depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    visit(doc, None, 0, &mut out);
    out
}

fn select(doc: &mut Document, id: Id, toggle: bool, range: bool, target: &mut EditTarget) {
    if toggle {
        if doc.selected_ids().contains(&id) && doc.selected_ids().len() > 1 {
            doc.selected.retain(|s| *s != id);
            if doc.active == Some(id) {
                doc.active = doc.selected.last().copied();
            }
        } else {
            if let Some(a) = doc.active {
                if !doc.selected.contains(&a) {
                    doc.selected.push(a);
                }
            }
            doc.selected.push(id);
            doc.active = Some(id);
        }
    } else if range {
        let rows: Vec<Id> = visible_rows(doc).into_iter().map(|(i, _)| i).collect();
        if let (Some(a), Some(b)) = (doc.active.and_then(|a| rows.iter().position(|r| *r == a)), rows.iter().position(|r| *r == id)) {
            let (lo, hi) = (a.min(b), a.max(b));
            doc.selected = rows[lo..=hi].to_vec();
            doc.active = Some(id);
        }
    } else {
        if doc.active != Some(id) {
            *target = EditTarget::Image;
        }
        doc.selected.clear();
        doc.active = Some(id);
    }
    if doc.layer(id).is_some_and(|l| l.mask.is_none()) {
        *target = EditTarget::Image;
    }
}

pub fn new_layer(project: &mut Project) {
    let transform = project.doc.full_canvas_transform();
    let name = project.doc.unique_name("Layer");
    project.edit("New Layer", |doc| doc.insert_above_active(Layer::blank(name, transform)));
    project.target = EditTarget::Image;
}
