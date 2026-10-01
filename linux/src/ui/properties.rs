//! The Properties panel: settings of the active adjustment layer, or the active layer's
//! transform, text and effects.

use crate::project::Project;
use crate::render::RenderCache;
use crate::tools::Colors;

/// Name of the edit that live property changes are grouped under until the pointer lifts.
const EDIT: &str = "Properties";

pub fn show(ui: &mut egui::Ui, project: &mut Project, colors: &mut Colors, cache: &RenderCache) {
    // A drag on a slider is one undo step.
    if project.pending_edit.as_deref() == Some(EDIT) && !ui.input(|i| i.pointer.any_down()) {
        project.finish_edit();
    }
    let Some(layer) = project.doc.active_layer().cloned() else {
        ui.label("No layer selected");
        return;
    };
    if let Some(mut adjustment) = layer.adjustment.clone() {
        ui.heading(crate::adjust::kind_name(adjustment.kind));
        if crate::adjust::properties_ui(ui, &mut adjustment) {
            change(project, |doc| {
                if let Some(l) = doc.active_layer_mut() {
                    l.adjustment = Some(adjustment);
                }
            });
        }
        return;
    }
    ui.heading(if layer.is_group { "Folder" } else if layer.is_text() { "Text" } else { "Layer" });
    if !layer.is_group {
        transform_ui(ui, project);
    }
    if layer.is_text() {
        ui.separator();
        crate::text::properties_ui(ui, project, colors, cache);
    }
    if !layer.is_group {
        ui.separator();
        crate::effects::properties_ui(ui, project);
    }
}

/// Applies a property change, recording one undo step per interaction.
pub fn change(project: &mut Project, edit: impl FnOnce(&mut crate::doc::Document)) {
    if project.pending_edit.is_none() {
        project.begin_edit(EDIT);
    }
    edit(&mut project.doc);
    project.invalidate_all();
}

fn transform_ui(ui: &mut egui::Ui, project: &mut Project) {
    let Some(layer) = project.doc.active_layer() else { return };
    let mut t = layer.transform;
    let (pw, ph) = layer.pixel_size();
    let before = t;
    egui::Grid::new("transform").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("X");
        ui.add(egui::DragValue::new(&mut t.origin[0]).speed(1.0).suffix(" px"));
        ui.label("Y");
        ui.add(egui::DragValue::new(&mut t.origin[1]).speed(1.0).suffix(" px"));
        ui.end_row();
        ui.label("W");
        ui.add(egui::DragValue::new(&mut t.size[0]).speed(1.0).range(1.0..=300_000.0).suffix(" px"));
        ui.label("H");
        ui.add(egui::DragValue::new(&mut t.size[1]).speed(1.0).range(1.0..=300_000.0).suffix(" px"));
        ui.end_row();
        ui.label("Angle");
        ui.add(egui::DragValue::new(&mut t.rotation).speed(0.5).suffix("°"));
        ui.label("Scale");
        let mut scale = t.size[0] / pw.max(1) as f64 * 100.0;
        if ui.add(egui::DragValue::new(&mut scale).speed(0.5).range(0.1..=10_000.0).suffix("%")).changed() {
            let (cx, cy) = t.center();
            t.size = [pw as f64 * scale / 100.0, ph as f64 * scale / 100.0];
            t.origin = [cx - t.size[0] / 2.0, cy - t.size[1] / 2.0];
        }
        ui.end_row();
    });
    ui.horizontal(|ui| {
        ui.toggle_value(&mut t.flip_x, "⇋ Flip H");
        ui.toggle_value(&mut t.flip_y, "⇵ Flip V");
        egui::ComboBox::from_id_salt("sampling")
            .selected_text(match t.sampling {
                crate::doc::Sampling::High => "High quality",
                crate::doc::Sampling::Smooth => "Smooth",
                crate::doc::Sampling::Nearest => "Nearest",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut t.sampling, crate::doc::Sampling::High, "High quality");
                ui.selectable_value(&mut t.sampling, crate::doc::Sampling::Smooth, "Smooth");
                ui.selectable_value(&mut t.sampling, crate::doc::Sampling::Nearest, "Nearest");
            });
    });
    if t != before && t.is_valid() {
        change(project, |doc| {
            if let Some(l) = doc.active_layer_mut() {
                l.transform = t;
            }
        });
    }
}
