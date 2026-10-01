//! Floating dialogs. A dialog is a window that edits the current project (or makes a new one)
//! until it's confirmed or cancelled.

use std::path::PathBuf;

use crate::doc::Document;
use crate::project::Project;
use crate::render::RenderCache;
use crate::tools::Colors;

pub struct DialogCtx<'a> {
    pub project: Option<&'a mut Project>,
    pub cache: &'a RenderCache,
    pub colors: &'a mut Colors,
    /// Projects the dialog wants opened as new tabs.
    pub new_projects: &'a mut Vec<Project>,
    pub status: &'a mut Option<String>,
    /// A file dialog the dialog wants shown, with what to do with the result.
    pub file_request: &'a mut Option<crate::app::FileRequest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogState {
    Open,
    Closed,
}

pub trait Dialog {
    fn title(&self) -> String;
    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut DialogCtx) -> DialogState;
    /// Called when the window's close button is used: undo any preview.
    fn cancel(&mut self, _ctx: &mut DialogCtx) {}
    /// Receives a path chosen in a file dialog this dialog requested.
    fn file_chosen(&mut self, _path: PathBuf, _ctx: &mut DialogCtx) {}
}

/// Shows `dialog` as a window; returns false once it has closed.
pub fn show(egui_ctx: &egui::Context, dialog: &mut Box<dyn Dialog>, ctx: &mut DialogCtx) -> bool {
    let mut open = true;
    let mut state = DialogState::Open;
    egui::Window::new(dialog.title())
        .id(egui::Id::new("compositor-dialog"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
        .show(egui_ctx, |ui| {
            state = dialog.ui(ui, ctx);
        });
    if !open {
        dialog.cancel(ctx);
        return false;
    }
    state == DialogState::Open
}

/// OK/Cancel buttons; returns `Some(true)` for OK, `Some(false)` for Cancel. Enter and Escape work too.
pub fn ok_cancel(ui: &mut egui::Ui, ok_label: &str) -> Option<bool> {
    let mut result = None;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Cancel").clicked() {
            result = Some(false);
        }
        if ui.add(egui::Button::new(ok_label).fill(ui.visuals().selection.bg_fill)).clicked() {
            result = Some(true);
        }
    });
    if result.is_none() {
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if enter && !ui.ctx().egui_wants_keyboard_input() {
            result = Some(true);
        } else if escape {
            result = Some(false);
        }
    }
    result
}

// ---------------------------------------------------------------------------------------------

pub struct NewDocumentDialog {
    width: u32,
    height: u32,
    resolution: f64,
    background: usize,
    preset: usize,
}

const PRESETS: &[(&str, u32, u32)] = &[
    ("Custom", 0, 0),
    ("HD 1920 × 1080", 1920, 1080),
    ("4K UHD 3840 × 2160", 3840, 2160),
    ("Square 2048 × 2048", 2048, 2048),
    ("Instagram Portrait 1080 × 1350", 1080, 1350),
    ("Story 1080 × 1920", 1080, 1920),
    ("A4 300 ppi 2480 × 3508", 2480, 3508),
    ("Letter 300 ppi 2550 × 3300", 2550, 3300),
];

impl Default for NewDocumentDialog {
    fn default() -> Self {
        NewDocumentDialog { width: 1920, height: 1080, resolution: 72.0, background: 0, preset: 1 }
    }
}

impl NewDocumentDialog {
    pub fn with_size(width: u32, height: u32) -> Self {
        NewDocumentDialog { width, height, preset: 0, ..Default::default() }
    }
}

impl Dialog for NewDocumentDialog {
    fn title(&self) -> String {
        "New Document".into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut DialogCtx) -> DialogState {
        egui::Grid::new("new-doc").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Preset");
            egui::ComboBox::from_id_salt("preset").selected_text(PRESETS[self.preset].0).show_ui(ui, |ui| {
                for (i, (name, w, h)) in PRESETS.iter().enumerate() {
                    if ui.selectable_value(&mut self.preset, i, *name).clicked() && *w > 0 {
                        self.width = *w;
                        self.height = *h;
                    }
                }
            });
            ui.end_row();
            ui.label("Width");
            if ui.add(egui::DragValue::new(&mut self.width).range(1..=30_000).suffix(" px")).changed() {
                self.preset = 0;
            }
            ui.end_row();
            ui.label("Height");
            if ui.add(egui::DragValue::new(&mut self.height).range(1..=30_000).suffix(" px")).changed() {
                self.preset = 0;
            }
            ui.end_row();
            ui.label("Resolution");
            ui.add(egui::DragValue::new(&mut self.resolution).range(1.0..=9600.0).suffix(" ppi"));
            ui.end_row();
            ui.label("Background");
            egui::ComboBox::from_id_salt("bg")
                .selected_text(["White", "Black", "Background Color", "Transparent"][self.background])
                .show_ui(ui, |ui| {
                    for (i, name) in ["White", "Black", "Background Color", "Transparent"].iter().enumerate() {
                        ui.selectable_value(&mut self.background, i, *name);
                    }
                });
            ui.end_row();
        });
        if self.width as u64 * self.height as u64 > 100_000_000 {
            ui.colored_label(ui.visuals().error_fg_color, "Canvases are limited to 100 megapixels.");
        }
        match ok_cancel(ui, "Create") {
            Some(true) if self.width as u64 * self.height as u64 <= 100_000_000 => {
                let background = match self.background {
                    0 => Some([255, 255, 255, 255]),
                    1 => Some([0, 0, 0, 255]),
                    2 => Some(ctx.colors.background),
                    _ => None,
                };
                let mut doc = Document::new(self.width, self.height, background);
                doc.resolution = self.resolution;
                let n = ctx.new_projects.len() + 1;
                ctx.new_projects.push(Project::new(doc, None, format!("Untitled-{n}")));
                DialogState::Closed
            }
            Some(false) => DialogState::Closed,
            _ => DialogState::Open,
        }
    }
}

// ---------------------------------------------------------------------------------------------

pub struct ExportDialog {
    format: crate::io::ExportFormat,
    quality: u8,
    preview: Option<(egui::TextureHandle, usize)>,
    encoded_size: Option<usize>,
    dirty: bool,
}

impl Default for ExportDialog {
    fn default() -> Self {
        ExportDialog { format: crate::io::ExportFormat::Jpeg, quality: 90, preview: None, encoded_size: None, dirty: true }
    }
}

impl Dialog for ExportDialog {
    fn title(&self) -> String {
        "Export As".into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut DialogCtx) -> DialogState {
        use crate::io::ExportFormat;
        let Some(project) = ctx.project.as_deref_mut() else { return DialogState::Closed };
        ui.horizontal(|ui| {
            ui.label("Format");
            let before = (self.format, self.quality);
            ui.selectable_value(&mut self.format, ExportFormat::Jpeg, "JPEG");
            ui.selectable_value(&mut self.format, ExportFormat::Png, "PNG");
            if self.format == ExportFormat::Jpeg {
                ui.add(egui::Slider::new(&mut self.quality, 1..=100).text("Quality"));
            }
            if before != (self.format, self.quality) {
                self.dirty = true;
            }
        });
        if self.dirty && !ui.input(|i| i.pointer.any_down()) {
            self.dirty = false;
            let flat = crate::io::flatten(&project.doc);
            if let Ok(bytes) = crate::io::encode(&flat, self.format, self.quality) {
                self.encoded_size = Some(bytes.len());
                if let Ok(decoded) = image::load_from_memory(&bytes) {
                    let small = decoded.thumbnail(640, 640).to_rgba8();
                    let image = egui::ColorImage::from_rgba_unmultiplied([small.width() as usize, small.height() as usize], small.as_raw());
                    let tex = ui.ctx().load_texture("export-preview", image, egui::TextureOptions::LINEAR);
                    self.preview = Some((tex, bytes.len()));
                }
            }
        }
        if let Some((tex, _)) = &self.preview {
            ui.add(egui::Image::new(tex).max_size(egui::vec2(480.0, 360.0)));
        }
        if let Some(size) = self.encoded_size {
            ui.label(format!(
                "{} × {} px — about {}",
                project.doc.width,
                project.doc.height,
                human_size(size)
            ));
        }
        match ok_cancel(ui, "Export…") {
            Some(true) => {
                let ext = if self.format == ExportFormat::Jpeg { "jpg" } else { "png" };
                *ctx.file_request = Some(crate::app::FileRequest::Export { extension: ext.into(), quality: self.quality, name: project.title.clone() });
                DialogState::Closed
            }
            Some(false) => DialogState::Closed,
            None => DialogState::Open,
        }
    }
}

pub fn human_size(bytes: usize) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1 << 20) as f64)
    } else {
        format!("{:.0} KB", (bytes as f64 / 1024.0).max(1.0))
    }
}

// ---------------------------------------------------------------------------------------------

#[derive(Default)]
pub struct FillDialog {
    source: usize,
    opacity: f32,
    custom: [u8; 4],
}

impl FillDialog {
    pub fn new(colors: &Colors) -> Self {
        FillDialog { source: 0, opacity: 100.0, custom: colors.foreground }
    }
}

impl Dialog for FillDialog {
    fn title(&self) -> String {
        "Fill".into()
    }
    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut DialogCtx) -> DialogState {
        egui::Grid::new("fill").num_columns(2).show(ui, |ui| {
            ui.label("Contents");
            egui::ComboBox::from_id_salt("fill-src")
                .selected_text(["Foreground Color", "Background Color", "Color…", "Black", "50% Gray", "White"][self.source])
                .show_ui(ui, |ui| {
                    for (i, name) in ["Foreground Color", "Background Color", "Color…", "Black", "50% Gray", "White"].iter().enumerate() {
                        ui.selectable_value(&mut self.source, i, *name);
                    }
                });
            ui.end_row();
            if self.source == 2 {
                ui.label("Color");
                ui.color_edit_button_srgba_unmultiplied(&mut self.custom);
                ui.end_row();
            }
            ui.label("Opacity");
            ui.add(egui::Slider::new(&mut self.opacity, 0.0..=100.0).suffix("%"));
            ui.end_row();
        });
        match ok_cancel(ui, "OK") {
            Some(true) => {
                let mut color = match self.source {
                    0 => ctx.colors.foreground,
                    1 => ctx.colors.background,
                    2 => self.custom,
                    3 => [0, 0, 0, 255],
                    4 => [128, 128, 128, 255],
                    _ => [255, 255, 255, 255],
                };
                color[3] = (color[3] as f32 * self.opacity / 100.0).round() as u8;
                if let Some(project) = ctx.project.as_deref_mut() {
                    fill_target(project, color);
                }
                DialogState::Closed
            }
            Some(false) => DialogState::Closed,
            None => DialogState::Open,
        }
    }
}

/// Fills the selection of the layer, or of its mask when the mask is the target (using the
/// color's gray value).
pub fn fill_target(project: &mut Project, color: [u8; 4]) {
    use crate::project::EditTarget;
    if project.target == EditTarget::Mask {
        let gray = ((color[0] as u32 * 30 + color[1] as u32 * 59 + color[2] as u32 * 11) / 100) as u8;
        project.edit("Fill Mask", |doc| crate::doc::ops::fill_mask(doc, gray));
    } else {
        project.edit("Fill", |doc| crate::doc::ops::fill(doc, color));
    }
}
