//! The Image, Layer, Select, Filter and View menus, and the Help dialogs.

use egui::Button;

use crate::app::MenuCtx;
use crate::doc::ops::{self, CanvasTurn};
use crate::doc::{Guide, GuideAxis, Id, Selection};
use crate::project::EditTarget;
use crate::ui::dialogs::{ok_cancel, Dialog, DialogCtx, DialogState};

fn item(ui: &mut egui::Ui, enabled: bool, label: &str, shortcut: &str) -> bool {
    let mut button = Button::new(label);
    if !shortcut.is_empty() {
        button = button.shortcut_text(shortcut);
    }
    let clicked = ui.add_enabled(enabled, button).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

pub fn image_menu(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    let has = ctx.project.is_some();
    ui.menu_button("Adjustments", |ui| {
        crate::filters::adjustments_menu(ui, ctx);
    });
    ui.separator();
    crate::filters::image_size_items(ui, ctx);
    ui.separator();
    ui.menu_button("Image Rotation", |ui| {
        for (label, turn) in [
            ("180°", CanvasTurn::Half),
            ("90° Clockwise", CanvasTurn::Clockwise),
            ("90° Counter Clockwise", CanvasTurn::CounterClockwise),
        ] {
            if item(ui, has, label, "") {
                if let Some(p) = ctx.project.as_deref_mut() {
                    p.edit("Rotate Canvas", |doc| ops::turn_canvas(doc, turn));
                    p.view.request_fit();
                }
            }
        }
        ui.separator();
        if item(ui, has, "Flip Canvas Horizontal", "") {
            if let Some(p) = ctx.project.as_deref_mut() {
                p.edit("Flip Canvas Horizontal", |doc| ops::turn_canvas(doc, CanvasTurn::FlipHorizontal));
            }
        }
        if item(ui, has, "Flip Canvas Vertical", "") {
            if let Some(p) = ctx.project.as_deref_mut() {
                p.edit("Flip Canvas Vertical", |doc| ops::turn_canvas(doc, CanvasTurn::FlipVertical));
            }
        }
    });
    let has_selection = ctx.project.as_ref().is_some_and(|p| p.doc.selection.is_some());
    if item(ui, has_selection, "Crop to Selection", "") {
        if let Some(p) = ctx.project.as_deref_mut() {
            if let Some((x0, y0, x1, y1)) = p.doc.selection.as_ref().map(|s| s.bounds) {
                p.edit("Crop", |doc| ops::crop_canvas(doc, x0 as i64, y0 as i64, x1 - x0, y1 - y0));
                p.view.request_fit();
            }
        }
    }
}

pub fn layer_menu(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    let Some(project) = ctx.project.as_deref_mut() else {
        ui.label("No document open");
        return;
    };
    let active = project.doc.active_layer().cloned();
    let has_layer = active.is_some();
    if item(ui, true, "New Layer", "Ctrl+Shift+N") {
        crate::ui::layers::new_layer(project);
    }
    if item(ui, has_layer, "Duplicate Layer", "Ctrl+J") {
        let ids = project.doc.selected_ids();
        project.edit("Duplicate Layer", |doc| ops::duplicate_layers(doc, &ids));
    }
    if item(ui, has_layer, "Delete Layer", "") {
        let ids = project.doc.selected_ids();
        project.edit("Delete Layer", |doc| doc.remove_layers(&ids));
    }
    ui.separator();
    ui.menu_button("New Adjustment Layer", |ui| {
        for kind in crate::adjust::MENU_KINDS {
            if item(ui, true, crate::adjust::kind_name(*kind), "") {
                crate::adjust::add_adjustment_layer(project, *kind);
            }
        }
    });
    ui.menu_button("Layer Style", |ui| {
        crate::effects::menu(ui, project);
    });
    ui.menu_button("Layer Mask", |ui| {
        let id = active.as_ref().map(|l| l.id);
        let has_mask = active.as_ref().is_some_and(|l| l.mask.is_some());
        let has_selection = project.doc.selection.is_some();
        if item(ui, has_layer && !has_mask, "Reveal All", "") {
            project.edit("Add Mask", |doc| ops::add_mask(doc, id.unwrap(), false));
            project.target = EditTarget::Mask;
        }
        if item(ui, has_layer && !has_mask, "Hide All", "") {
            project.edit("Add Mask", |doc| ops::add_mask(doc, id.unwrap(), true));
            project.target = EditTarget::Mask;
        }
        if item(ui, has_layer && !has_mask && has_selection, "Reveal Selection", "") {
            project.edit("Add Mask", |doc| ops::add_mask(doc, id.unwrap(), false));
            project.target = EditTarget::Mask;
        }
        ui.separator();
        let enabled = active.as_ref().and_then(|l| l.mask.as_ref()).is_some_and(|m| m.enabled);
        if item(ui, has_mask, if enabled { "Disable" } else { "Enable" }, "") {
            project.edit("Toggle Mask", |doc| {
                if let Some(m) = doc.active_layer_mut().and_then(|l| l.mask.as_mut()) {
                    m.enabled = !m.enabled;
                }
            });
        }
        let linked = active.as_ref().and_then(|l| l.mask.as_ref()).is_some_and(|m| m.linked);
        if item(ui, has_mask, if linked { "Unlink" } else { "Link" }, "") {
            project.edit("Link Mask", |doc| {
                if let Some(l) = doc.active_layer_mut() {
                    let t = l.transform;
                    if let Some(m) = l.mask.as_mut() {
                        m.linked = !m.linked;
                        if !m.linked {
                            m.placement = Some(t);
                        } else {
                            m.placement = None;
                        }
                    }
                }
            });
        }
        if item(ui, has_mask, "Apply", "") {
            project.edit("Apply Layer Mask", |doc| ops::apply_mask(doc, id.unwrap()));
            project.target = EditTarget::Image;
        }
        if item(ui, has_mask, "Delete", "") {
            project.edit("Delete Layer Mask", |doc| doc.active_layer_mut().map(|l| l.mask = None));
            project.target = EditTarget::Image;
        }
    });
    let clipped = active.as_ref().is_some_and(|l| l.clip_source.is_some());
    if item(ui, has_layer, if clipped { "Release Clipping Mask" } else { "Create Clipping Mask" }, "Ctrl+Alt+G") {
        let mut doc = project.doc.clone();
        if ops::toggle_clip(&mut doc, active.as_ref().unwrap().id) {
            project.edit("Clipping Mask", |d| *d = doc);
        }
    }
    ui.separator();
    if item(ui, has_layer, "Group Layers", "Ctrl+G") {
        let ids = project.doc.selected_ids();
        project.edit("Group Layers", |doc| ops::group_layers(doc, &ids));
    }
    let is_group = active.as_ref().is_some_and(|l| l.is_group);
    if item(ui, is_group, "Ungroup Layers", "Ctrl+Shift+G") {
        let id = active.as_ref().unwrap().id;
        project.edit("Ungroup", |doc| ops::ungroup(doc, id));
    }
    ui.menu_button("Arrange", |ui| {
        if item(ui, has_layer, "Bring Forward", "Ctrl+]") {
            let mut doc = project.doc.clone();
            if ops::shift_layers(&mut doc, true) {
                project.edit("Bring Forward", |d| *d = doc);
            }
        }
        if item(ui, has_layer, "Send Backward", "Ctrl+[") {
            let mut doc = project.doc.clone();
            if ops::shift_layers(&mut doc, false) {
                project.edit("Send Backward", |d| *d = doc);
            }
        }
    });
    ui.menu_button("Transform", |ui| {
        if item(ui, has_layer, "Free Transform", "Ctrl+T") {
            ctx.tools.current = crate::tools::ToolKind::Move;
            crate::tools::move_tool::show_controls(ctx.tools, true);
        }
        if item(ui, has_layer, "Flip Horizontal", "") {
            project.edit("Flip Layer Horizontal", |doc| ops::flip_layers(doc, true));
        }
        if item(ui, has_layer, "Flip Vertical", "") {
            project.edit("Flip Layer Vertical", |doc| ops::flip_layers(doc, false));
        }
        if item(ui, has_layer, "Rotate 90° Clockwise", "") {
            rotate_layers(project, 90.0);
        }
        if item(ui, has_layer, "Rotate 90° Counter Clockwise", "") {
            rotate_layers(project, -90.0);
        }
        if item(ui, has_layer, "Rotate 180°", "") {
            rotate_layers(project, 180.0);
        }
    });
    let rasterizable = active.as_ref().is_some_and(|l| l.is_text() || l.shape.is_some());
    if item(ui, rasterizable, "Rasterize Layer", "") {
        let id = active.as_ref().unwrap().id;
        project.edit("Rasterize Layer", |doc| ops::rasterize(doc, id));
    }
    ui.separator();
    if item(ui, has_layer, if is_group { "Merge Group" } else { "Merge Down" }, "Ctrl+E") {
        let mut doc = project.doc.clone();
        if ops::merge_down(&mut doc, ctx.cache) {
            project.edit("Merge Down", |d| *d = doc);
        }
    }
    if item(ui, true, "Merge Visible", "Ctrl+Shift+E") {
        let cache = ctx.cache;
        project.edit("Merge Visible", |doc| ops::merge_visible(doc, cache));
    }
    if item(ui, true, "Flatten Image", "") {
        let cache = ctx.cache;
        project.edit("Flatten Image", |doc| ops::flatten(doc, cache));
    }
}

fn rotate_layers(project: &mut crate::project::Project, degrees: f64) {
    project.edit("Rotate Layer", |doc| {
        for id in crate::tools::move_tool::move_targets(doc) {
            if let Some(l) = doc.layer_mut(id) {
                l.transform.rotation = (l.transform.rotation + degrees) % 360.0;
            }
        }
    });
}

pub fn select_menu(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    let Some(project) = ctx.project.as_deref_mut() else {
        ui.label("No document open");
        return;
    };
    let has_selection = project.doc.selection.is_some();
    if item(ui, true, "All", "Ctrl+A") {
        let (w, h) = (project.doc.width, project.doc.height);
        project.edit("Select All", |doc| doc.selection = Some(Selection::all(w, h)));
    }
    if item(ui, has_selection, "Deselect", "Ctrl+D") {
        project.edit("Deselect", |doc| doc.selection = None);
    }
    if item(ui, has_selection, "Inverse", "Ctrl+Shift+I") {
        let sel = project.doc.selection.clone().unwrap();
        project.edit("Select Inverse", |doc| doc.selection = sel.inverted());
    }
    ui.separator();
    if item(ui, project.doc.active_layer().is_some_and(|l| l.image.is_some()), "Load Layer as Selection", "") {
        if let Some(sel) = layer_alpha_selection(project, ctx.cache) {
            project.edit("Load Selection", |doc| doc.selection = Some(sel));
        }
    }
    crate::selection::select_menu_items(ui, project, ctx.dialog);
}

/// The active layer's coverage (pixels, mask and opacity aside) as a selection.
pub fn layer_alpha_selection(project: &crate::project::Project, cache: &crate::render::RenderCache) -> Option<Selection> {
    let layer = project.doc.active_layer()?;
    let region = crate::render::Region::full(&project.doc);
    let placed = crate::render::place_layer(layer, region, cache)?;
    let mut mask = image::GrayImage::new(project.doc.width, project.doc.height);
    for (p, src) in mask.pixels_mut().zip(&placed.px) {
        p[0] = crate::render::to_u8(src[3]);
    }
    Selection::from_mask(mask)
}

pub fn filter_menu(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    crate::filters::filter_menu(ui, ctx);
}

pub fn view_menu(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    let Some(project) = ctx.project.as_deref_mut() else {
        ui.label("No document open");
        return;
    };
    if item(ui, true, "Zoom In", "Ctrl++") {
        let z = project.view.zoom * 1.5;
        project.view.request_zoom(z);
    }
    if item(ui, true, "Zoom Out", "Ctrl+-") {
        let z = project.view.zoom / 1.5;
        project.view.request_zoom(z);
    }
    if item(ui, true, "Fit on Screen", "Ctrl+0") {
        project.view.request_fit();
    }
    if item(ui, true, "100%", "Ctrl+1") {
        project.view.request_zoom(1.0);
    }
    ui.separator();
    ui.checkbox(&mut project.view.show_selection, "Selection Edges (Ctrl+H)");
    ui.checkbox(&mut project.view.show_guides, "Guides (Ctrl+;)");
    ui.checkbox(&mut project.view.show_grid, "Grid (Ctrl+')");
    ui.separator();
    if item(ui, true, "New Guide…", "") {
        *ctx.dialog = Some(Box::new(NewGuideDialog { horizontal: false, position: project.doc.width as f64 / 2.0 }));
    }
    if item(ui, !project.doc.guides.is_empty(), "Clear Guides", "") {
        project.edit("Clear Guides", |doc| doc.guides.clear());
    }
}

struct NewGuideDialog {
    horizontal: bool,
    position: f64,
}

impl Dialog for NewGuideDialog {
    fn title(&self) -> String {
        "New Guide".into()
    }
    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut DialogCtx) -> DialogState {
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.horizontal, true, "Horizontal");
            ui.radio_value(&mut self.horizontal, false, "Vertical");
        });
        ui.horizontal(|ui| {
            ui.label("Position");
            ui.add(egui::DragValue::new(&mut self.position).suffix(" px"));
        });
        match ok_cancel(ui, "OK") {
            Some(true) => {
                if let Some(p) = ctx.project.as_deref_mut() {
                    let guide = Guide {
                        id: Id::new(),
                        axis: if self.horizontal { GuideAxis::Horizontal } else { GuideAxis::Vertical },
                        position: self.position,
                    };
                    p.edit("New Guide", |doc| doc.guides.push(guide));
                }
                DialogState::Closed
            }
            Some(false) => DialogState::Closed,
            None => DialogState::Open,
        }
    }
}

pub struct AboutDialog;

impl Dialog for AboutDialog {
    fn title(&self) -> String {
        "About Compositor".into()
    }
    fn ui(&mut self, ui: &mut egui::Ui, _ctx: &mut DialogCtx) -> DialogState {
        ui.vertical_centered(|ui| {
            ui.heading("Compositor for Linux");
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.add_space(6.0);
            ui.label("A free, open-source image editor for compositing and photo work.");
            ui.label("Original macOS app by Robbie Tilton. Linux port by Caleb Trueman.");
            ui.hyperlink_to("github.com/calebtrueman/Compositor", "https://github.com/calebtrueman/Compositor");
            ui.label("MIT License");
        });
        if ui.button("Close").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            DialogState::Closed
        } else {
            DialogState::Open
        }
    }
}

pub struct ShortcutsDialog;

const SHORTCUTS: &[(&str, &str)] = &[
    ("New / Open / Save", "Ctrl+N / Ctrl+O / Ctrl+S"),
    ("Save As / Export", "Ctrl+Shift+S / Ctrl+Alt+Shift+S"),
    ("Undo / Redo", "Ctrl+Z / Ctrl+Shift+Z"),
    ("Copy / Copy Merged / Paste", "Ctrl+C / Ctrl+Shift+C / Ctrl+V"),
    ("Free Transform", "Ctrl+T"),
    ("New Layer / Duplicate", "Ctrl+Shift+N / Ctrl+J"),
    ("Group / Ungroup", "Ctrl+G / Ctrl+Shift+G"),
    ("Clipping Mask", "Ctrl+Alt+G (or Alt-click a layer)"),
    ("Merge Down / Visible", "Ctrl+E / Ctrl+Shift+E"),
    ("Bring Forward / Send Backward", "Ctrl+] / Ctrl+["),
    ("Select All / Deselect / Inverse", "Ctrl+A / Ctrl+D / Ctrl+Shift+I"),
    ("Fill Foreground / Background", "Alt+Backspace / Ctrl+Backspace"),
    ("Invert", "Ctrl+I"),
    ("Zoom In / Out / Fit / 100%", "Ctrl++ / Ctrl+- / Ctrl+0 / Ctrl+1"),
    ("Pan", "Space-drag, middle-drag or scroll"),
    ("Swap / Reset Colors", "X / D"),
    ("Brush size", "[ and ]"),
];

impl Dialog for ShortcutsDialog {
    fn title(&self) -> String {
        "Keyboard Shortcuts".into()
    }
    fn ui(&mut self, ui: &mut egui::Ui, _ctx: &mut DialogCtx) -> DialogState {
        egui::Grid::new("shortcuts").striped(true).num_columns(2).show(ui, |ui| {
            for (what, keys) in SHORTCUTS {
                ui.label(*what);
                ui.label(egui::RichText::new(*keys).monospace());
                ui.end_row();
            }
        });
        ui.label("Tools: V Move, M Marquee, L Lasso, W Magic Wand, C Crop, I Eyedropper, J Spot Healing, B Brush, E Eraser, S Clone Stamp, R Blur, G Gradient/Paint Bucket, U Shape, T Type, H Hand, Z Zoom.");
        if ui.button("Close").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            DialogState::Closed
        } else {
            DialogState::Open
        }
    }
}
