//! Destructive filters and image-wide operations: Image > Adjustments (the adjustment-layer
//! math applied to the active layer), Image Size, Canvas Size and Trim, and the Filter menu.
//! Each works on the active layer (or its mask), limited to the selection, with a live preview.

pub mod dialog;
pub mod kinds;
pub mod size;

use crate::adjust::{Adjustment, AdjustmentColor, AdjustmentKind, GradientMapSettings, HueSaturationSettings};
use crate::app::MenuCtx;
use crate::project::{EditTarget, Project};
use dialog::{Job, LiveDialog};
use kinds::{FilterKind, FilterSettings};

/// Image > Adjustments, in the macOS app's order.
const ADJUSTMENTS: &[(AdjustmentKind, &str)] = &[
    (AdjustmentKind::Levels, "Ctrl+L"),
    (AdjustmentKind::Curves, "Ctrl+M"),
    (AdjustmentKind::Exposure, ""),
    (AdjustmentKind::HueSaturation, "Ctrl+U"),
    (AdjustmentKind::ColorBalance, "Ctrl+B"),
    (AdjustmentKind::BlackWhite, "Alt+Shift+Ctrl+B"),
    (AdjustmentKind::GradientMap, ""),
    (AdjustmentKind::Grain, ""),
];

/// Whether the active layer has pixels a color adjustment can change.
fn can_adjust(project: &Project) -> bool {
    project.target == EditTarget::Image && dialog::Source::of(&project.doc, EditTarget::Image, false).is_some()
}

/// Whether a filter can run on the active layer, or its mask when that's the target.
fn can_filter(project: &Project, kind: FilterKind) -> bool {
    let allow_empty = kind == FilterKind::Vignette && project.target == EditTarget::Image;
    dialog::Source::of(&project.doc, project.target, allow_empty).is_some()
}

/// Opens the dialog for an Image > Adjustments command.
pub fn open_adjustment(ctx: &mut MenuCtx, kind: AdjustmentKind) {
    let Some(project) = ctx.project.as_deref_mut() else { return };
    if !can_adjust(project) {
        return;
    }
    let mut adjustment = Adjustment::new(kind);
    if kind == AdjustmentKind::GradientMap {
        // Gradient Map starts from the foreground and background colors, as in Photoshop.
        adjustment.gradient_map_settings = Some(GradientMapSettings {
            shadows: AdjustmentColor::from_rgba8(ctx.colors.foreground),
            highlights: AdjustmentColor::from_rgba8(ctx.colors.background),
            reversed: false,
        });
    }
    if let Some(d) = LiveDialog::new(project, Job::Adjust(adjustment)) {
        *ctx.dialog = Some(Box::new(d));
    }
}

pub fn open_filter(ctx: &mut MenuCtx, kind: FilterKind) {
    let Some(project) = ctx.project.as_deref_mut() else { return };
    if !can_filter(project, kind) {
        return;
    }
    if let Some(d) = LiveDialog::new(project, Job::Filter(FilterSettings::last_used(kind))) {
        *ctx.dialog = Some(Box::new(d));
    }
}

/// Image > Adjustments > Desaturate: every color to its gray (HSL lightness), as Photoshop's.
pub fn desaturate(project: &mut Project) {
    if !can_adjust(project) {
        return;
    }
    let mut adjustment = Adjustment::new(AdjustmentKind::HueSaturation);
    adjustment.hsv_settings = Some(HueSaturationSettings::new(0.0, -100.0, 0.0, false));
    dialog::apply_now(project, "Desaturate", &Job::Adjust(adjustment), 0);
}

pub fn adjustments_menu(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    let adjustable = ctx.project.as_deref().is_some_and(can_adjust);
    for (kind, shortcut) in ADJUSTMENTS {
        let label = format!("{}…", crate::adjust::kind_name(*kind));
        if ui.add_enabled(adjustable, egui::Button::new(label).shortcut_text(*shortcut)).clicked() {
            open_adjustment(ctx, *kind);
            ui.close();
        }
    }
    ui.separator();
    if ui.add_enabled(adjustable, egui::Button::new("Desaturate").shortcut_text("Shift+Ctrl+U")).clicked() {
        if let Some(p) = ctx.project.as_deref_mut() {
            desaturate(p);
        }
        ui.close();
    }
    let has = ctx.project.is_some();
    let mask = ctx.project.as_deref().is_some_and(|p| p.target == EditTarget::Mask);
    if ui.add_enabled(has, egui::Button::new(if mask { "Invert Mask" } else { "Invert" }).shortcut_text("Ctrl+I")).clicked() {
        if let Some(p) = ctx.project.as_deref_mut() {
            crate::adjust::invert_active(p);
        }
        ui.close();
    }
}

pub fn image_size_items(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    let has = ctx.project.is_some();
    if ui.add_enabled(has, egui::Button::new("Image Size…").shortcut_text("Alt+Ctrl+I")).clicked() {
        if let Some(p) = ctx.project.as_deref() {
            *ctx.dialog = Some(Box::new(size::ImageSizeDialog::new(&p.doc)));
        }
        ui.close();
    }
    if ui.add_enabled(has, egui::Button::new("Canvas Size…").shortcut_text("Alt+Ctrl+C")).clicked() {
        if let Some(p) = ctx.project.as_deref() {
            *ctx.dialog = Some(Box::new(size::CanvasSizeDialog::new(&p.doc)));
        }
        ui.close();
    }
    if ui.add_enabled(has, egui::Button::new("Trim…")).clicked() {
        *ctx.dialog = Some(Box::new(size::TrimDialog::default()));
        ui.close();
    }
}

pub fn filter_menu(ui: &mut egui::Ui, ctx: &mut MenuCtx) {
    for kind in FilterKind::ALL {
        let enabled = ctx.project.as_deref().is_some_and(|p| can_filter(p, kind));
        if kind == FilterKind::UnsharpMask || kind == FilterKind::Vignette || kind == FilterKind::LensCorrection {
            ui.separator();
        }
        if ui.add_enabled(enabled, egui::Button::new(format!("{}…", kind.name()))).clicked() {
            open_filter(ctx, kind);
            ui.close();
        }
    }
}

/// Keyboard shortcuts for the Image menu's adjustments and sizes.
pub fn shortcuts(egui_ctx: &egui::Context, ctx: &mut MenuCtx) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    if ctx.dialog.is_some() || ctx.project.is_none() {
        return;
    }
    let cmd = Modifiers::COMMAND;
    let pressed = |m: Modifiers, k: Key| egui_ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
    // Shifted and Alt variants first: a plain shortcut also matches them.
    if pressed(cmd | Modifiers::ALT | Modifiers::SHIFT, Key::B) {
        open_adjustment(ctx, AdjustmentKind::BlackWhite);
    } else if pressed(cmd, Key::B) {
        open_adjustment(ctx, AdjustmentKind::ColorBalance);
    }
    if pressed(cmd | Modifiers::SHIFT, Key::U) {
        if let Some(p) = ctx.project.as_deref_mut() {
            desaturate(p);
        }
    } else if pressed(cmd, Key::U) {
        open_adjustment(ctx, AdjustmentKind::HueSaturation);
    }
    if pressed(cmd, Key::L) {
        open_adjustment(ctx, AdjustmentKind::Levels);
    }
    if pressed(cmd, Key::M) {
        open_adjustment(ctx, AdjustmentKind::Curves);
    }
    if pressed(cmd | Modifiers::ALT, Key::I) {
        if let Some(p) = ctx.project.as_deref() {
            *ctx.dialog = Some(Box::new(size::ImageSizeDialog::new(&p.doc)));
        }
    }
    if pressed(cmd | Modifiers::ALT, Key::C) {
        if let Some(p) = ctx.project.as_deref() {
            *ctx.dialog = Some(Box::new(size::CanvasSizeDialog::new(&p.doc)));
        }
    }
}
