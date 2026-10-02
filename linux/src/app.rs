//! The application window: menus, tabs, toolbar, panels and keyboard shortcuts.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Key, KeyboardShortcut, Modifiers};
use egui_file_dialog::FileDialog;
use image::RgbaImage;

use crate::doc::ops;
use crate::doc::{Layer, LayerTransform, Selection};
use crate::io;
use crate::project::{EditTarget, Project};
use crate::render::RenderCache;
use crate::tools::{Colors, ToolCtx, ToolKind, Tools};
use crate::ui::dialogs::{self, Dialog, DialogCtx};
use crate::ui::{canvas, color, layers::LayersPanel, properties};

/// What to do with the path picked in the file dialog.
#[derive(Clone, Debug)]
pub enum FileRequest {
    Open,
    Place,
    SaveAs,
    Export { extension: String, quality: u8, name: String },
    /// A path for the dialog that's currently open.
    ForDialog { save: bool, extensions: Vec<String> },
}

/// Everything a menu command can reach.
pub struct MenuCtx<'a> {
    pub project: Option<&'a mut Project>,
    pub cache: &'a RenderCache,
    pub colors: &'a mut Colors,
    pub dialog: &'a mut Option<Box<dyn Dialog>>,
    pub status: &'a mut Option<String>,
    pub tools: &'a mut Tools,
}

pub struct App {
    projects: Vec<Project>,
    current: usize,
    tools: Tools,
    colors: Colors,
    cache: Arc<RenderCache>,
    layers_panel: LayersPanel,
    file_dialog: FileDialog,
    file_request: Option<FileRequest>,
    dialog: Option<Box<dyn Dialog>>,
    status: Option<String>,
    error: Option<String>,
    clipboard: Option<(RgbaImage, (i64, i64))>,
    recent: Vec<PathBuf>,
    /// Projects waiting for an answer about unsaved changes before closing.
    closing: Option<usize>,
    quit_requested: bool,
    allow_quit: bool,
    hovered_doc: Option<(f64, f64)>,
    open_adjustment: Option<crate::adjust::AdjustmentKind>,
    /// A project with unsaved edits that also changed on disk, waiting for the person to choose.
    external_change: Option<usize>,
    last_watch: f64,
}

const RECENT_KEY: &str = "recent-files";
const COLORS_KEY: &str = "colors";

impl App {
    pub fn new(cc: &eframe::CreationContext, files: Vec<PathBuf>) -> Self {
        crate::ui::theme::apply(&cc.egui_ctx);
        let recent: Vec<PathBuf> = cc
            .storage
            .and_then(|s| eframe::get_value::<Vec<String>>(s, RECENT_KEY))
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let colors = cc
            .storage
            .and_then(|s| eframe::get_value::<([u8; 4], [u8; 4])>(s, COLORS_KEY))
            .map(|(f, b)| Colors { foreground: f, background: b })
            .unwrap_or_default();
        let mut app = App {
            projects: Vec::new(),
            current: 0,
            tools: Tools::new(),
            colors,
            cache: Arc::new(RenderCache::default()),
            layers_panel: LayersPanel::default(),
            file_dialog: FileDialog::new(),
            file_request: None,
            dialog: None,
            status: None,
            error: None,
            clipboard: None,
            recent,
            closing: None,
            quit_requested: false,
            allow_quit: false,
            hovered_doc: None,
            open_adjustment: None,
            external_change: None,
            last_watch: 0.0,
        };
        for file in files {
            app.open_path(&file);
        }
        app
    }

    fn project(&mut self) -> Option<&mut Project> {
        self.projects.get_mut(self.current)
    }

    fn open_path(&mut self, path: &Path) {
        let root = io::comp::package_root(path);
        if let Some(index) = self.projects.iter().position(|p| p.path.as_deref() == Some(root.as_path())) {
            self.current = index;
            return;
        }
        match io::open_document(path) {
            Ok(doc) => {
                let is_project = io::comp::is_project(path);
                let title = root.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
                let mut project = Project::new(doc, is_project.then(|| root.clone()), title);
                if is_project {
                    project.disk_digest = io::watch::digest(&root);
                } else {
                    // An imported image becomes a new, unsaved project.
                    project.history.mark_unsaved();
                }
                self.projects.push(project);
                self.current = self.projects.len() - 1;
                self.add_recent(root);
            }
            Err(error) => self.error = Some(format!("Couldn't open “{}”.\n\n{error:#}", path.display())),
        }
    }

    fn add_recent(&mut self, path: PathBuf) {
        self.recent.retain(|p| *p != path);
        self.recent.insert(0, path);
        self.recent.truncate(12);
    }

    /// Places an image file as a new layer in the current project, fitted inside the canvas.
    fn place_path(&mut self, path: &Path) {
        let Some(project) = self.projects.get_mut(self.current) else {
            self.open_path(path);
            return;
        };
        let result = if io::PSD_EXTENSIONS.contains(&io::extension(path).as_str()) {
            io::psd::load(path).map(|d| io::flatten(&d))
        } else {
            io::read_image(path)
        };
        match result {
            Ok(image) => {
                let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Placed".into());
                place_image(project, image, &name, None);
            }
            Err(error) => self.error = Some(format!("Couldn't place “{}”.\n\n{error:#}", path.display())),
        }
    }

    fn save(&mut self, save_as: bool) {
        let Some(project) = self.projects.get_mut(self.current) else { return };
        match (&project.path, save_as) {
            (Some(path), false) => {
                let path = path.clone();
                self.save_to(&path);
            }
            _ => {
                let name = format!("{}.comp", project.title);
                self.request_file(FileRequest::SaveAs, Some(name));
            }
        }
    }

    fn save_to(&mut self, path: &Path) {
        let mut path = path.to_path_buf();
        if io::extension(&path) != "comp" {
            path.set_extension("comp");
        }
        let Some(project) = self.projects.get_mut(self.current) else { return };
        if let Some(tool) = self.tools.get_mut(self.tools.current) {
            let mut status = None;
            let mut ctx = ToolCtx { project, colors: &mut self.colors, cache: &self.cache, zoom: 1.0, status: &mut status };
            tool.commit(&mut ctx);
        }
        let project = self.projects.get_mut(self.current).unwrap();
        match io::comp::save(&project.doc, &path) {
            Ok(()) => {
                project.path = Some(path.clone());
                project.title = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                project.history.mark_saved();
                project.disk_digest = io::watch::digest(&path);
                project.pending_digest = None;
                self.status = Some(format!("Saved {}", path.display()));
                self.add_recent(path);
            }
            Err(error) => self.error = Some(format!("The project couldn't be saved. The previous version has not been replaced.\n\n{error:#}")),
        }
    }

    fn request_file(&mut self, request: FileRequest, default_name: Option<String>) {
        let mut dialog = FileDialog::new()
            .title(match &request {
                FileRequest::Open => "Open",
                FileRequest::Place => "Place Image",
                FileRequest::SaveAs => "Save Project As",
                FileRequest::Export { .. } => "Export",
                FileRequest::ForDialog { save: true, .. } => "Save",
                FileRequest::ForDialog { .. } => "Open",
            })
            .default_size([760.0, 480.0]);
        if let Some(dir) = self.recent.first().and_then(|p| p.parent()) {
            if dir.is_dir() {
                dialog = dialog.initial_directory(dir.to_path_buf());
            }
        }
        match &request {
            FileRequest::Open | FileRequest::Place => {
                let mut exts: Vec<&'static str> = io::IMAGE_EXTENSIONS.to_vec();
                exts.extend_from_slice(io::PSD_EXTENSIONS);
                dialog = dialog
                    .add_file_filter_extensions("Images and projects", {
                        let mut e = exts.clone();
                        e.push("json");
                        e
                    })
                    .add_file_filter_extensions("Images", exts)
                    .default_file_filter("Images and projects");
                // A .comp project is a folder: picking it opens it instead of browsing inside.
                dialog.set_open_directory_filter(egui_file_dialog::Filter::new(|p: &Path| !io::comp::is_project(p)));
                dialog.pick_file();
            }
            FileRequest::SaveAs => {
                if let Some(name) = &default_name {
                    dialog = dialog.default_file_name(name);
                }
                dialog.set_open_directory_filter(egui_file_dialog::Filter::new(|p: &Path| !io::comp::is_project(p)));
                dialog.save_file();
            }
            FileRequest::Export { extension, name, .. } => {
                dialog = dialog.default_file_name(&format!("{name}.{extension}"));
                dialog.save_file();
            }
            FileRequest::ForDialog { save, extensions } => {
                if !extensions.is_empty() {
                    let leaked: Vec<&'static str> = extensions.iter().map(|e| &*Box::leak(e.clone().into_boxed_str())).collect();
                    dialog = dialog.add_file_filter_extensions("Supported files", leaked).default_file_filter("Supported files");
                }
                if let Some(name) = &default_name {
                    dialog = dialog.default_file_name(name);
                }
                if *save {
                    dialog.save_file();
                } else {
                    dialog.pick_file();
                }
            }
        }
        self.file_dialog = dialog;
        self.file_request = Some(request);
    }

    fn file_picked(&mut self, path: PathBuf, request: FileRequest) {
        match request {
            FileRequest::Open => self.open_path(&path),
            FileRequest::Place => self.place_path(&path),
            FileRequest::SaveAs => self.save_to(&path),
            FileRequest::Export { extension, quality, .. } => {
                let mut path = path;
                if !matches!(io::extension(&path).as_str(), "jpg" | "jpeg" | "png") {
                    path.set_extension(&extension);
                }
                if let Some(project) = self.projects.get(self.current) {
                    match io::export(&project.doc, &path, quality) {
                        Ok(()) => self.status = Some(format!("Exported {}", path.display())),
                        Err(error) => self.error = Some(format!("Export failed.\n\n{error:#}")),
                    }
                }
            }
            FileRequest::ForDialog { .. } => {
                if let Some(mut dialog) = self.dialog.take() {
                    let mut new_projects = Vec::new();
                    let mut file_request = None;
                    let mut ctx = DialogCtx {
                        project: self.projects.get_mut(self.current),
                        cache: &self.cache,
                        colors: &mut self.colors,
                        new_projects: &mut new_projects,
                        status: &mut self.status,
                        file_request: &mut file_request,
                    };
                    dialog.file_chosen(path, &mut ctx);
                    self.dialog = Some(dialog);
                    self.projects.extend(new_projects);
                }
            }
        }
    }

    fn close_project(&mut self, index: usize, force: bool) {
        if index >= self.projects.len() {
            return;
        }
        if !force && self.projects[index].is_dirty() {
            self.closing = Some(index);
            return;
        }
        self.projects.remove(index);
        if self.current >= self.projects.len() {
            self.current = self.projects.len().saturating_sub(1);
        }
    }

    fn copy(&mut self, merged: bool, cut: bool) {
        let Some(project) = self.projects.get_mut(self.current) else { return };
        let Some(image) = ops::copy_pixels(&project.doc, merged, &self.cache) else { return };
        let Some((x0, y0, x1, y1)) = ops::alpha_bounds(&image) else { return };
        let cropped = image::imageops::crop_imm(&image, x0, y0, x1 - x0, y1 - y0).to_image();
        crate::ui::clipboard::put_image(&cropped);
        self.clipboard = Some((cropped, (x0 as i64, y0 as i64)));
        if cut && !merged {
            project.edit("Cut", |doc| {
                if doc.selection.is_some() {
                    ops::clear_selection_pixels(doc);
                }
            });
        }
    }

    fn paste(&mut self) {
        let system = crate::ui::clipboard::get_image();
        let Some(project) = self.projects.get_mut(self.current) else {
            if let Some(image) = system.or_else(|| self.clipboard.as_ref().map(|c| c.0.clone())) {
                let doc = crate::doc::Document::from_image(image, "Pasted");
                let mut p = Project::new(doc, None, "Pasted".into());
                p.history.mark_unsaved();
                self.projects.push(p);
                self.current = self.projects.len() - 1;
            }
            return;
        };
        let (image, origin) = match (system, &self.clipboard) {
            (Some(sys), Some((own, origin))) if sys.dimensions() == own.dimensions() => (own.clone(), Some(*origin)),
            (Some(sys), _) => (sys, None),
            (None, Some((own, origin))) => (own.clone(), Some(*origin)),
            (None, None) => return,
        };
        place_image(project, image, "Pasted Layer", origin);
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let cmd = Modifiers::COMMAND;
        let shift_cmd = Modifiers::COMMAND | Modifiers::SHIFT;
        let alt_cmd = Modifiers::COMMAND | Modifiers::ALT;
        let shortcut = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        // egui ignores extra Shift and Alt when matching; shortcuts here need the exact modifiers,
        // so Ctrl+Shift+N doesn't also count as Ctrl+N.
        let pressed = |s: KeyboardShortcut| {
            ctx.input_mut(|i| {
                let exact = i.events.iter().any(|e| {
                    matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. }
                        if *key == s.logical_key && same_modifiers(*modifiers, s.modifiers))
                });
                exact && i.consume_shortcut(&s)
            })
        };

        if pressed(shortcut(cmd, Key::N)) {
            self.dialog = Some(Box::new(dialogs::NewDocumentDialog::default()));
        }
        if pressed(shortcut(cmd, Key::O)) {
            self.request_file(FileRequest::Open, None);
        }
        if pressed(shortcut(shift_cmd, Key::S)) {
            self.save(true);
        } else if pressed(shortcut(Modifiers::COMMAND | Modifiers::ALT | Modifiers::SHIFT, Key::S)) || pressed(shortcut(alt_cmd, Key::E)) {
            if !self.projects.is_empty() {
                self.dialog = Some(Box::new(dialogs::ExportDialog::default()));
            }
        } else if pressed(shortcut(cmd, Key::S)) {
            self.save(false);
        }
        if pressed(shortcut(cmd, Key::W)) {
            self.close_project(self.current, false);
        }
        if pressed(shortcut(cmd, Key::Q)) {
            self.quit_requested = true;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let tool_busy = self.tools.current().captures_keyboard();
        if !tool_busy {
            if pressed(shortcut(shift_cmd, Key::Z)) || pressed(shortcut(cmd, Key::Y)) {
                if let Some(p) = self.project() {
                    p.redo();
                }
            } else if pressed(shortcut(cmd, Key::Z)) {
                if let Some(p) = self.project() {
                    p.undo();
                }
            }
            if pressed(shortcut(shift_cmd, Key::C)) {
                self.copy(true, false);
            } else if pressed(shortcut(cmd, Key::C)) {
                self.copy(false, false);
            }
            if pressed(shortcut(cmd, Key::X)) {
                self.copy(false, true);
            }
            if pressed(shortcut(cmd, Key::V)) {
                self.paste();
            }
        }
        let Some(project) = self.projects.get_mut(self.current) else { return };
        if tool_busy {
            return;
        }
        // Selection
        if pressed(shortcut(cmd, Key::A)) {
            let (w, h) = (project.doc.width, project.doc.height);
            project.edit("Select All", |doc| doc.selection = Some(Selection::all(w, h)));
        }
        if pressed(shortcut(shift_cmd, Key::D)) && project.doc.selection.is_none() {
            if let Some(selection) = project.last_selection.clone().filter(|s| s.mask.dimensions() == (project.doc.width, project.doc.height)) {
                project.edit("Reselect", |doc| doc.selection = Some(selection));
            }
        }
        if pressed(shortcut(cmd, Key::D)) && project.doc.selection.is_some() {
            project.edit("Deselect", |doc| doc.selection = None);
        }
        if pressed(shortcut(shift_cmd, Key::I)) {
            if let Some(sel) = project.doc.selection.clone() {
                project.edit("Select Inverse", |doc| doc.selection = sel.inverted());
            }
        }
        // Layers
        if pressed(shortcut(shift_cmd, Key::N)) {
            crate::ui::layers::new_layer(project);
        }
        if pressed(shortcut(cmd, Key::J)) {
            let ids = project.doc.selected_ids();
            project.edit("Duplicate Layer", |doc| ops::duplicate_layers(doc, &ids));
        }
        if pressed(shortcut(shift_cmd, Key::G)) {
            if let Some(id) = project.doc.active {
                project.edit("Ungroup", |doc| ops::ungroup(doc, id));
            }
        } else if pressed(shortcut(alt_cmd, Key::G)) {
            if let Some(id) = project.doc.active {
                let mut doc = project.doc.clone();
                if ops::toggle_clip(&mut doc, id) {
                    project.edit("Clipping Mask", |d| *d = doc);
                }
            }
        } else if pressed(shortcut(cmd, Key::G)) {
            let ids = project.doc.selected_ids();
            project.edit("Group Layers", |doc| ops::group_layers(doc, &ids));
        }
        if pressed(shortcut(shift_cmd, Key::E)) {
            let cache = self.cache.clone();
            project.edit("Merge Visible", |doc| ops::merge_visible(doc, &cache));
        } else if pressed(shortcut(cmd, Key::E)) {
            let mut doc = project.doc.clone();
            if ops::merge_down(&mut doc, &self.cache) {
                project.edit("Merge Down", |d| *d = doc);
            }
        }
        if pressed(shortcut(cmd, Key::CloseBracket)) {
            let mut doc = project.doc.clone();
            if ops::shift_layers(&mut doc, true) {
                project.edit("Bring Forward", |d| *d = doc);
            }
        }
        if pressed(shortcut(cmd, Key::OpenBracket)) {
            let mut doc = project.doc.clone();
            if ops::shift_layers(&mut doc, false) {
                project.edit("Send Backward", |d| *d = doc);
            }
        }
        if pressed(shortcut(cmd, Key::I)) {
            crate::adjust::invert_active(project);
        }
        if pressed(shortcut(cmd, Key::T)) {
            self.tools.current = ToolKind::Move;
            crate::tools::move_tool::show_controls(&mut self.tools, true);
        }
        let project = self.projects.get_mut(self.current).unwrap();
        // Fill: Alt+Backspace foreground, Ctrl+Backspace background, Delete clears.
        if pressed(shortcut(Modifiers::ALT, Key::Backspace)) {
            dialogs::fill_target(project, self.colors.foreground);
        } else if pressed(shortcut(cmd, Key::Backspace)) {
            dialogs::fill_target(project, self.colors.background);
        } else if pressed(shortcut(Modifiers::SHIFT, Key::F5)) {
            self.dialog = Some(Box::new(dialogs::FillDialog::new(&self.colors)));
        } else if pressed(shortcut(Modifiers::NONE, Key::Delete)) || pressed(shortcut(Modifiers::NONE, Key::Backspace)) {
            if project.doc.selection.is_some() {
                if project.target == EditTarget::Mask {
                    project.edit("Clear Mask", |doc| ops::fill_mask(doc, 0));
                } else {
                    project.edit("Clear", ops::clear_selection_pixels);
                }
            } else if project.doc.active.is_some() {
                let ids = project.doc.selected_ids();
                project.edit("Delete Layer", |doc| doc.remove_layers(&ids));
            }
        }
        // View
        if pressed(shortcut(cmd, Key::Equals)) || pressed(shortcut(cmd, Key::Plus)) || pressed(shortcut(shift_cmd, Key::Equals)) {
            let z = project.view.zoom * 1.5;
            project.view.request_zoom(z);
        }
        if pressed(shortcut(cmd, Key::Minus)) {
            let z = project.view.zoom / 1.5;
            project.view.request_zoom(z);
        }
        if pressed(shortcut(cmd, Key::Num0)) {
            project.view.request_fit();
        }
        if pressed(shortcut(cmd, Key::Num1)) {
            project.view.request_zoom(1.0);
        }
        if pressed(shortcut(cmd, Key::Quote)) {
            project.view.show_grid = !project.view.show_grid;
        }
        if pressed(shortcut(cmd, Key::Semicolon)) {
            project.view.show_guides = !project.view.show_guides;
        }
        if pressed(shortcut(cmd, Key::H)) {
            project.view.show_selection = !project.view.show_selection;
        }
        // Colors
        if pressed(shortcut(Modifiers::NONE, Key::X)) {
            std::mem::swap(&mut self.colors.foreground, &mut self.colors.background);
        }
        if pressed(shortcut(Modifiers::NONE, Key::D)) {
            self.colors = Colors::default();
        }
        // Enter/Escape go to the tool.
        let (enter, escape) = ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
        if enter || escape {
            let project = self.projects.get_mut(self.current).unwrap();
            let mut ctx_tool = ToolCtx { project, colors: &mut self.colors, cache: &self.cache, zoom: 1.0, status: &mut self.status };
            if enter {
                self.tools.current_mut().commit(&mut ctx_tool);
            } else {
                self.tools.current_mut().cancel(&mut ctx_tool);
            }
        }
        // Remaining keys: tool keys, then tool shortcuts.
        let events: Vec<(Key, Modifiers)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
                .collect()
        });
        for (key, modifiers) in events {
            let project = self.projects.get_mut(self.current).unwrap();
            let zoom = project.view.zoom;
            let mut tool_ctx = ToolCtx { project, colors: &mut self.colors, cache: &self.cache, zoom, status: &mut self.status };
            if self.tools.current_mut().key(key, modifiers, &mut tool_ctx) {
                continue;
            }
            if modifiers.is_none() || modifiers == Modifiers::SHIFT {
                let matching: Vec<ToolKind> = self.tools.list.iter().filter(|t| t.shortcut() == Some(key)).map(|t| t.kind()).collect();
                if !matching.is_empty() {
                    // Pressing the same key again cycles tools sharing it.
                    let next = match matching.iter().position(|k| *k == self.tools.current) {
                        Some(i) => matching[(i + 1) % matching.len()],
                        None => matching[0],
                    };
                    self.switch_tool(next);
                }
            }
        }
    }

    pub fn switch_tool(&mut self, kind: ToolKind) {
        if self.tools.current == kind {
            return;
        }
        if let Some(project) = self.projects.get_mut(self.current) {
            let zoom = project.view.zoom;
            let mut ctx = ToolCtx { project, colors: &mut self.colors, cache: &self.cache, zoom, status: &mut self.status };
            self.tools.current_mut().commit(&mut ctx);
        }
        self.tools.current = kind;
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.add(egui::Button::new("New…").shortcut_text("Ctrl+N")).clicked() {
                    self.dialog = Some(Box::new(dialogs::NewDocumentDialog::default()));
                    ui.close();
                }
                if ui.add(egui::Button::new("Open…").shortcut_text("Ctrl+O")).clicked() {
                    self.request_file(FileRequest::Open, None);
                    ui.close();
                }
                ui.menu_button("Open Recent", |ui| {
                    if self.recent.is_empty() {
                        ui.label("No recent files");
                    }
                    for path in self.recent.clone() {
                        if ui.button(path.display().to_string()).clicked() {
                            self.open_path(&path);
                            ui.close();
                        }
                    }
                    if !self.recent.is_empty() {
                        ui.separator();
                        if ui.button("Clear Recent").clicked() {
                            self.recent.clear();
                            ui.close();
                        }
                    }
                });
                ui.separator();
                let has = !self.projects.is_empty();
                if ui.add_enabled(has, egui::Button::new("Close").shortcut_text("Ctrl+W")).clicked() {
                    self.close_project(self.current, false);
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Save").shortcut_text("Ctrl+S")).clicked() {
                    self.save(false);
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Save As…").shortcut_text("Ctrl+Shift+S")).clicked() {
                    self.save(true);
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new("Place Image…")).clicked() {
                    self.request_file(FileRequest::Place, None);
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Export As…").shortcut_text("Ctrl+Alt+Shift+S")).clicked() {
                    self.dialog = Some(Box::new(dialogs::ExportDialog::default()));
                    ui.close();
                }
                ui.separator();
                if ui.add(egui::Button::new("Quit").shortcut_text("Ctrl+Q")).clicked() {
                    self.quit_requested = true;
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| self.edit_menu(ui));
            ui.menu_button("Image", |ui| {
                let mut ctx = self.menu_ctx();
                crate::ui::menus::image_menu(ui, &mut ctx);
            });
            ui.menu_button("Layer", |ui| {
                let mut ctx = self.menu_ctx();
                crate::ui::menus::layer_menu(ui, &mut ctx);
            });
            ui.menu_button("Select", |ui| {
                let mut ctx = self.menu_ctx();
                crate::ui::menus::select_menu(ui, &mut ctx);
            });
            ui.menu_button("Filter", |ui| {
                let mut ctx = self.menu_ctx();
                crate::ui::menus::filter_menu(ui, &mut ctx);
            });
            ui.menu_button("View", |ui| {
                let mut ctx = self.menu_ctx();
                crate::ui::menus::view_menu(ui, &mut ctx);
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Keyboard Shortcuts").clicked() {
                    self.dialog = Some(Box::new(crate::ui::menus::ShortcutsDialog));
                    ui.close();
                }
                if ui.button("About Compositor").clicked() {
                    self.dialog = Some(Box::new(crate::ui::menus::AboutDialog));
                    ui.close();
                }
            });
        });
    }

    fn edit_menu(&mut self, ui: &mut egui::Ui) {
        let has = !self.projects.is_empty();
        let (undo, redo) = self
            .projects
            .get(self.current)
            .map(|p| (p.history.undo_name().map(str::to_string), p.history.redo_name().map(str::to_string)))
            .unwrap_or((None, None));
        let undo_label = undo.as_ref().map(|n| format!("Undo {n}")).unwrap_or_else(|| "Undo".into());
        if ui.add_enabled(undo.is_some(), egui::Button::new(undo_label).shortcut_text("Ctrl+Z")).clicked() {
            if let Some(p) = self.project() {
                p.undo();
            }
            ui.close();
        }
        let redo_label = redo.as_ref().map(|n| format!("Redo {n}")).unwrap_or_else(|| "Redo".into());
        if ui.add_enabled(redo.is_some(), egui::Button::new(redo_label).shortcut_text("Ctrl+Shift+Z")).clicked() {
            if let Some(p) = self.project() {
                p.redo();
            }
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(has, egui::Button::new("Cut").shortcut_text("Ctrl+X")).clicked() {
            self.copy(false, true);
            ui.close();
        }
        if ui.add_enabled(has, egui::Button::new("Copy").shortcut_text("Ctrl+C")).clicked() {
            self.copy(false, false);
            ui.close();
        }
        if ui.add_enabled(has, egui::Button::new("Copy Merged").shortcut_text("Ctrl+Shift+C")).clicked() {
            self.copy(true, false);
            ui.close();
        }
        if ui.add(egui::Button::new("Paste").shortcut_text("Ctrl+V")).clicked() {
            self.paste();
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(has, egui::Button::new("Fill…").shortcut_text("Shift+F5")).clicked() {
            self.dialog = Some(Box::new(dialogs::FillDialog::new(&self.colors)));
            ui.close();
        }
        if ui.add_enabled(has, egui::Button::new("Free Transform").shortcut_text("Ctrl+T")).clicked() {
            self.tools.current = ToolKind::Move;
            crate::tools::move_tool::show_controls(&mut self.tools, true);
            ui.close();
        }
    }

    fn menu_ctx(&mut self) -> MenuCtx<'_> {
        MenuCtx {
            project: self.projects.get_mut(self.current),
            cache: &self.cache,
            colors: &mut self.colors,
            dialog: &mut self.dialog,
            status: &mut self.status,
            tools: &mut self.tools,
        }
    }

    fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let mut close = None;
            for (i, project) in self.projects.iter().enumerate() {
                let selected = i == self.current;
                let frame = egui::Frame::new()
                    .fill(if selected { ui.visuals().widgets.active.bg_fill } else { ui.visuals().faint_bg_color })
                    .corner_radius(4)
                    .inner_margin(egui::Margin::symmetric(8, 3));
                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.add(egui::Label::new(project.display_title()).sense(egui::Sense::click())).clicked() {
                            self.current = i;
                        }
                        if ui.add(egui::Button::new("×").frame(false).small()).on_hover_text("Close").clicked() {
                            close = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = close {
                self.close_project(i, false);
            }
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            let kinds: Vec<(ToolKind, &'static str, &'static str, Option<Key>)> =
                self.tools.list.iter().map(|t| (t.kind(), t.icon(), t.name(), t.shortcut())).collect();
            for (kind, icon, name, key) in kinds {
                let selected = self.tools.current == kind;
                let hint = match key {
                    Some(k) => format!("{name} ({})", k.name()),
                    None => name.to_string(),
                };
                let button = egui::Button::new(egui::RichText::new(icon).size(18.0)).selected(selected).min_size(egui::vec2(34.0, 30.0));
                if ui.add(button).on_hover_text(hint).clicked() {
                    self.switch_tool(kind);
                }
            }
            ui.add_space(10.0);
            color::swatches(ui, &mut self.colors);
        });
    }

    fn options_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let name = self.tools.current().name();
            ui.label(egui::RichText::new(name).strong());
            ui.separator();
            if let Some(project) = self.projects.get_mut(self.current) {
                let zoom = project.view.zoom;
                let mut ctx = ToolCtx { project, colors: &mut self.colors, cache: &self.cache, zoom, status: &mut self.status };
                self.tools.current_mut().options_ui(ui, &mut ctx);
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(project) = self.projects.get(self.current) {
                ui.label(format!("{:.0}%", project.view.zoom * 100.0));
                ui.separator();
                ui.label(format!("{} × {} px", project.doc.width, project.doc.height));
                if let Some((x, y)) = self.hovered_doc {
                    ui.separator();
                    ui.label(format!("X {:.0}  Y {:.0}", x.floor(), y.floor()));
                }
                if project.target == EditTarget::Mask {
                    ui.separator();
                    ui.label("Editing mask");
                }
            }
            if let Some(status) = &self.status {
                ui.separator();
                ui.label(status);
            }
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            ui.heading("Compositor");
            ui.label("A free, open-source image editor for compositing and photo work.");
            ui.add_space(16.0);
            if ui.button("New Document…").clicked() {
                self.dialog = Some(Box::new(dialogs::NewDocumentDialog::default()));
            }
            if ui.button("Open…").clicked() {
                self.request_file(FileRequest::Open, None);
            }
            ui.add_space(16.0);
            if !self.recent.is_empty() {
                ui.label(egui::RichText::new("Recent").strong());
                for path in self.recent.clone().into_iter().take(8) {
                    if ui.link(path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).on_hover_text(path.display().to_string()).clicked() {
                        self.open_path(&path);
                    }
                }
            }
            ui.add_space(12.0);
            ui.label(egui::RichText::new("Drop images or .comp projects here to open them.").weak());
        });
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        for path in dropped {
            // Like the macOS app: images dropped on an open project become layers.
            if !self.projects.is_empty() && io::is_importable_image(&path) && !io::comp::is_project(&path) {
                self.place_path(&path);
            } else {
                self.open_path(&path);
            }
        }
    }

    /// Reloads projects that changed on disk once writes have settled for a third of a second.
    /// With unsaved edits of their own, the person chooses which version to keep.
    fn watch_projects(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if self.projects.iter().any(|p| p.path.is_some()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        }
        if now - self.last_watch < 0.3 {
            return;
        }
        self.last_watch = now;
        for index in 0..self.projects.len() {
            let project = &mut self.projects[index];
            let Some(path) = project.path.clone() else { continue };
            if project.pending_edit.is_some() {
                continue;
            }
            let Some(digest) = io::watch::digest(&path) else { continue };
            if project.disk_digest == Some(digest) {
                project.pending_digest = None;
                continue;
            }
            match project.pending_digest {
                Some((pending, since)) if pending == digest && now - since >= 0.3 => {
                    project.pending_digest = None;
                    if project.is_dirty() {
                        if self.external_change.is_none() {
                            self.external_change = Some(index);
                        }
                    } else {
                        self.reload(index, digest);
                    }
                }
                Some((pending, _)) if pending == digest => {}
                _ => project.pending_digest = Some((digest, now)),
            }
        }
    }

    /// Replaces a project with what's on disk, keeping the view and selection; undo starts over.
    fn reload(&mut self, index: usize, digest: u64) {
        let project = &mut self.projects[index];
        let Some(path) = project.path.clone() else { return };
        // A write that fails to load is ignored until the next change.
        project.disk_digest = Some(digest);
        let Ok(mut doc) = io::comp::load(&path) else { return };
        doc.selection = project.doc.selection.take().filter(|s| s.mask.dimensions() == (doc.width, doc.height));
        if let Some(active) = project.doc.active.filter(|a| doc.layer(*a).is_some()) {
            doc.active = Some(active);
        }
        for layer in &mut doc.layers {
            if let Some(old) = project.doc.layer(layer.id) {
                layer.expanded = old.expanded;
            }
        }
        project.doc = doc;
        project.history.clear();
        project.invalidate_all();
    }

    fn external_change_prompt(&mut self, ctx: &egui::Context) {
        let Some(index) = self.external_change else { return };
        let Some(project) = self.projects.get(index) else {
            self.external_change = None;
            return;
        };
        let title = project.title.clone();
        let mut choice = None;
        egui::Modal::new(egui::Id::new("external-change")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.heading(format!("“{title}” changed on disk"));
            ui.label("Another program changed this project while you had unsaved changes. Use the version on disk, or keep yours?");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Keep Mine").clicked() {
                    choice = Some(false);
                }
                if ui.button("Revert to Disk").clicked() {
                    choice = Some(true);
                }
            });
        });
        if let Some(revert) = choice {
            self.external_change = None;
            let path = self.projects[index].path.clone();
            if let Some(digest) = path.as_deref().and_then(io::watch::digest) {
                if revert {
                    self.reload(index, digest);
                } else {
                    self.projects[index].disk_digest = Some(digest);
                }
            }
        }
    }

    fn unsaved_prompt(&mut self, ctx: &egui::Context) {
        let Some(index) = self.closing else { return };
        let Some(project) = self.projects.get(index) else {
            self.closing = None;
            return;
        };
        let title = project.title.clone();
        let mut choice = None;
        egui::Modal::new(egui::Id::new("unsaved")).show(ctx, |ui| {
            ui.heading(format!("Save changes to “{title}”?"));
            ui.label("Your changes will be lost if you don't save them.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Don't Save").clicked() {
                    choice = Some(0);
                }
                if ui.button("Cancel").clicked() {
                    choice = Some(1);
                }
                if ui.button("Save").clicked() {
                    choice = Some(2);
                }
            });
        });
        match choice {
            Some(0) => {
                self.closing = None;
                self.close_project(index, true);
                if self.quit_requested {
                    self.continue_quit(ctx);
                }
            }
            Some(1) => {
                self.closing = None;
                self.quit_requested = false;
            }
            Some(2) => {
                self.closing = None;
                self.current = index;
                self.save(false);
                if !self.projects[index].is_dirty() {
                    self.close_project(index, true);
                    if self.quit_requested {
                        self.continue_quit(ctx);
                    }
                } else {
                    self.quit_requested = false;
                }
            }
            _ => {}
        }
    }

    fn continue_quit(&mut self, ctx: &egui::Context) {
        if let Some(index) = self.projects.iter().position(|p| p.is_dirty()) {
            self.closing = Some(index);
        } else {
            self.allow_quit = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn same_modifiers(a: Modifiers, b: Modifiers) -> bool {
    a.alt == b.alt && a.shift == b.shift && (a.command || a.ctrl) == (b.command || b.ctrl)
}

/// Adds `image` as a layer above the active one: at `origin` when given, otherwise centered and
/// scaled down to fit the canvas.
pub fn place_image(project: &mut Project, image: RgbaImage, name: &str, origin: Option<(i64, i64)>) {
    let (w, h) = (image.width() as f64, image.height() as f64);
    let (cw, ch) = (project.doc.width as f64, project.doc.height as f64);
    let transform = match origin {
        Some((x, y)) => LayerTransform::rect(x as f64, y as f64, w, h),
        None => {
            let scale = (cw / w).min(ch / h).min(1.0);
            let (sw, sh) = ((w * scale).round().max(1.0), (h * scale).round().max(1.0));
            LayerTransform::rect(((cw - sw) / 2.0).round(), ((ch - sh) / 2.0).round(), sw, sh)
        }
    };
    let layer = Layer::new_pixel(name, image, transform);
    project.edit(&format!("Place {name}"), |doc| doc.insert_above_active(layer));
    project.target = EditTarget::Image;
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let recent: Vec<String> = self.recent.iter().map(|p| p.display().to_string()).collect();
        eframe::set_value(storage, RECENT_KEY, &recent);
        eframe::set_value(storage, COLORS_KEY, &(self.colors.foreground, self.colors.background));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        // Closing the window asks about unsaved work first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_quit {
            if self.projects.iter().any(|p| p.is_dirty()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.quit_requested = true;
                if self.closing.is_none() {
                    self.continue_quit(ctx);
                }
            }
        }
        if self.quit_requested && self.closing.is_none() {
            self.continue_quit(ctx);
        }

        // Window title
        let title = match self.projects.get(self.current) {
            Some(p) => format!("{} — Compositor", p.display_title()),
            None => "Compositor".into(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        self.handle_dropped_files(ctx);
        if self.dialog.is_none() && self.closing.is_none() {
            self.handle_shortcuts(ctx);
        }

        // File dialog
        self.file_dialog.update(ctx);
        if let Some(path) = self.file_dialog.take_picked() {
            if let Some(request) = self.file_request.take() {
                self.file_picked(path, request);
            }
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        if !self.projects.is_empty() {
            egui::Panel::top("options").show(ui, |ui| self.options_bar(ui));
            egui::Panel::top("tabs").show(ui, |ui| self.tabs(ui));
        }
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("toolbar").resizable(false).exact_size(48.0).show(ui, |ui| self.toolbar(ui));
        if !self.projects.is_empty() {
            egui::Panel::right("panels").default_size(300.0).min_size(240.0).show(ui, |ui| {
                let project = self.projects.get_mut(self.current).unwrap();
                egui::Panel::top("properties").resizable(true).default_size(240.0).show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        properties::show(ui, project, &mut self.colors, &self.cache);
                    });
                });
                ui.heading("Layers");
                self.layers_panel.show(ui, project, &self.cache, &mut self.open_adjustment);
            });
        }
        if let Some(kind) = self.open_adjustment.take() {
            if let Some(project) = self.projects.get_mut(self.current) {
                crate::adjust::add_adjustment_layer(project, kind);
            }
        }

        let temporary_hand = !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_down(Key::Space)) && !self.tools.current().captures_keyboard();
        egui::CentralPanel::default().frame(egui::Frame::new()).show(ui, |ui| {
            if let Some(project) = self.projects.get_mut(self.current) {
                let output = canvas::show(ui, project, &mut self.tools, &mut self.colors, &self.cache, &mut self.status, temporary_hand);
                self.hovered_doc = output.hovered_doc;
            } else {
                self.welcome(ui);
            }
        });

        // Dialogs
        if let Some(mut dialog) = self.dialog.take() {
            let mut new_projects = Vec::new();
            let mut file_request = None;
            let keep = {
                let mut dctx = DialogCtx {
                    project: self.projects.get_mut(self.current),
                    cache: &self.cache,
                    colors: &mut self.colors,
                    new_projects: &mut new_projects,
                    status: &mut self.status,
                    file_request: &mut file_request,
                };
                dialogs::show(ctx, &mut dialog, &mut dctx)
            };
            if keep && self.dialog.is_none() {
                self.dialog = Some(dialog);
            }
            if !new_projects.is_empty() {
                self.projects.extend(new_projects);
                self.current = self.projects.len() - 1;
            }
            if let Some(request) = file_request {
                let name = match &request {
                    FileRequest::Export { name, extension, .. } => Some(format!("{name}.{extension}")),
                    _ => None,
                };
                self.request_file(request, name);
            }
        }
        self.unsaved_prompt(ctx);
        self.watch_projects(ctx);
        self.external_change_prompt(ctx);

        if let Some(error) = self.error.clone() {
            egui::Modal::new(egui::Id::new("error")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading("Something went wrong");
                ui.label(error);
                if ui.button("OK").clicked() || ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape)) {
                    self.error = None;
                }
            });
        }
    }
}
