//! Type tool: click to start point text, drag to draw a paragraph box, click a text layer to
//! edit it. Typing happens on the canvas with a caret and selection; Escape, clicking elsewhere or
//! switching tools commits. Each editing session is one undo step, and Ctrl+Z inside it takes back
//! typing as the macOS app's text box does. Face and color apply to the selected letters.

use std::time::{Duration, Instant};

use egui::{Color32, CursorIcon, Event, Key, Modifiers, Painter, Pos2, Stroke};

use super::{PointerEvent, PointerPhase, Tool, ToolCtx, ToolKind};
use crate::doc::{Affine, Id, Layer, LayerTransform};
use crate::project::Project;
use crate::text::layout::{self, Layout};
use crate::text::{self, fonts, utf16_len, TextStyle, PADDING};
use crate::ui::canvas::ViewTransform;

/// What the last change in a session was, so a run of typing undoes in one step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    Typing,
    Other,
}

#[derive(Clone)]
struct Snapshot {
    style: TextStyle,
    caret: usize,
    anchor: usize,
    origin: (f64, f64),
}

/// The text being edited.
struct Session {
    doc: Id,
    /// `None` for new text until its first letter is typed.
    layer: Option<Id>,
    /// Whether this session made the layer (empty new text is removed on commit).
    created: bool,
    /// Top-left of new text that has no layer yet.
    origin: (f64, f64),
    style: TextStyle,
    /// UTF-16 offsets; the selection runs from `anchor` to `caret`.
    caret: usize,
    anchor: usize,
    layout: Option<Layout>,
    /// Whether the session's undo step has been recorded.
    recorded: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<EditKind>,
    /// Where Up and Down aim, in layer pixels.
    goal_x: Option<f32>,
    /// Restarts the caret blink.
    changed_at: Instant,
}

impl Session {
    fn selection(&self) -> std::ops::Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { style: self.style.clone(), caret: self.caret, anchor: self.anchor, origin: self.origin }
    }

    /// Where the text sits on the document, and its pixel size.
    fn placement(&self, project: &Project) -> (LayerTransform, (u32, u32)) {
        if let Some(layer) = self.layer.and_then(|id| project.doc.layer(id)) {
            return (layer.transform, layer.pixel_size());
        }
        let (w, h) = self.layout.as_ref().map_or((16, 16), |l| (l.width, l.height));
        (LayerTransform::rect(self.origin.0, self.origin.1, w as f64, h as f64), (w, h))
    }

    /// Layer-pixel → document mapping.
    fn to_document(&self, project: &Project) -> Affine {
        let (transform, (w, h)) = self.placement(project);
        transform.pixel_to_document(w, h)
    }

    fn to_layer(&self, project: &Project, pos: (f64, f64)) -> Option<(f32, f32)> {
        let (u, v) = self.to_document(project).inverse()?.apply(pos.0, pos.1);
        Some((u as f32, v as f32))
    }

    fn hit(&self, project: &Project, pos: (f64, f64)) -> usize {
        match (self.to_layer(project, pos), &self.layout) {
            (Some((x, y)), Some(layout)) => layout.hit(x, y),
            _ => self.caret,
        }
    }
}

enum Drag {
    /// Dragging out a paragraph box (or a click, when it stays small).
    NewBox,
    Select,
    /// Resizing the box by one of its handles, clockwise from the top-left corner.
    Resize { handle: usize, start: LayerTransform, start_pixels: (u32, u32), start_box: [f64; 2] },
}

#[derive(Default)]
pub struct TypeTool {
    /// The settings new text starts with.
    defaults: Option<TextStyle>,
    session: Option<Session>,
    drag: Option<Drag>,
    /// The box being dragged out, in document coordinates.
    drag_from: Option<(f64, f64)>,
    drag_to: Option<(f64, f64)>,
    /// When a double click selected a word, so a press arriving with it doesn't undo that.
    double_click: Option<Instant>,
}

/// Handle positions on the box, clockwise from the top-left corner.
const HANDLES: [(f64, f64); 8] = [(0.0, 0.0), (0.5, 0.0), (1.0, 0.0), (1.0, 0.5), (1.0, 1.0), (0.5, 1.0), (0.0, 1.0), (0.0, 0.5)];

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\''
}

/// The start of the word before `unit`.
fn word_before(content: &str, unit: usize) -> usize {
    let chars: Vec<(usize, char)> = units(content);
    let mut i = chars.iter().rposition(|(u, _)| *u < unit).map_or(0, |p| p + 1);
    while i > 0 && !is_word(chars[i - 1].1) {
        i -= 1;
    }
    while i > 0 && is_word(chars[i - 1].1) {
        i -= 1;
    }
    chars.get(i).map_or(0, |c| c.0)
}

/// The end of the word after `unit`.
fn word_after(content: &str, unit: usize) -> usize {
    let chars: Vec<(usize, char)> = units(content);
    let total = utf16_len(content);
    let mut i = chars.iter().position(|(u, _)| *u >= unit).unwrap_or(chars.len());
    while i < chars.len() && !is_word(chars[i].1) {
        i += 1;
    }
    while i < chars.len() && is_word(chars[i].1) {
        i += 1;
    }
    chars.get(i).map_or(total, |c| c.0)
}

fn units(content: &str) -> Vec<(usize, char)> {
    let mut unit = 0;
    content
        .chars()
        .map(|c| {
            let at = unit;
            unit += c.len_utf16();
            (at, c)
        })
        .collect()
}

/// The letter boundary before or after `unit`, stepping over surrogate pairs.
fn step(content: &str, unit: usize, forward: bool) -> usize {
    let chars = units(content);
    if forward {
        chars.iter().find(|(u, _)| *u >= unit).map_or(utf16_len(content), |(u, c)| u + c.len_utf16())
    } else {
        chars.iter().rev().find(|(u, _)| *u < unit).map_or(0, |(u, _)| *u)
    }
}

/// Keeps letters and line breaks; other control characters are dropped.
fn clean(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).collect()
}

impl TypeTool {
    fn defaults(&mut self) -> TextStyle {
        self.defaults
            .get_or_insert_with(|| TextStyle {
                content: String::new(),
                font_name: fonts::library().default_post_script_name(),
                ..TextStyle::default()
            })
            .clone()
    }

    /// Ends the session if its document or layer went away (a tab switch, an undo), and picks up
    /// changes made to its layer elsewhere (the Properties panel).
    fn validate(&mut self, project: &Project) {
        let Some(session) = self.session.as_mut() else { return };
        if session.doc != project.doc.id {
            self.session = None;
            return;
        }
        let Some(id) = session.layer else { return };
        match project.doc.layer(id).and_then(|l| l.text.as_ref()) {
            None => self.session = None,
            Some(text) if *text != session.style => {
                session.style = text.clone();
                let count = session.style.len16();
                session.caret = session.caret.min(count);
                session.anchor = session.anchor.min(count);
                session.layout = layout::layout(&session.style);
            }
            _ => {}
        }
    }

    /// Text layer under a document point, topmost first.
    fn text_layer_at(project: &Project, pos: (f64, f64)) -> Option<Id> {
        let doc = &project.doc;
        doc.layers
            .iter()
            .rev()
            .find(|l| l.is_text() && l.image.is_some() && doc.is_effectively_visible(l.id) && l.transform.contains(pos.0, pos.1))
            .map(|l| l.id)
    }

    fn edit_layer(&mut self, ctx: &mut ToolCtx, id: Id, pos: Option<(f64, f64)>) {
        let Some(style) = ctx.project.doc.layer(id).and_then(|l| l.text.clone()) else { return };
        ctx.project.doc.active = Some(id);
        ctx.project.doc.selected.clear();
        ctx.project.target = crate::project::EditTarget::Image;
        let end = style.len16();
        let mut session = Session {
            doc: ctx.project.doc.id,
            layer: Some(id),
            created: false,
            origin: (0.0, 0.0),
            layout: layout::layout(&style),
            style,
            caret: end,
            anchor: end,
            recorded: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            goal_x: None,
            changed_at: Instant::now(),
        };
        if let Some(pos) = pos {
            session.caret = session.hit(ctx.project, pos);
            session.anchor = session.caret;
        }
        self.session = Some(session);
    }

    /// New text: point text with its first baseline at `pos`, or a paragraph box.
    fn begin_new(&mut self, ctx: &mut ToolCtx, pos: (f64, f64), box_rect: Option<(f64, f64, f64, f64)>) {
        let mut style = self.defaults();
        style.content = String::new();
        let fg = ctx.colors.foreground;
        style.red = fg[0] as f64 / 255.0;
        style.green = fg[1] as f64 / 255.0;
        style.blue = fg[2] as f64 / 255.0;
        let origin = match box_rect {
            Some((x0, y0, x1, y1)) => {
                style.box_size = Some([(x1 - x0).round().max(16.0), (y1 - y0).round().max(16.0)]);
                (x0.round(), y0.round())
            }
            None => {
                style.box_size = None;
                let layout = layout::layout(&style);
                let baseline = layout.as_ref().and_then(|l| l.lines.first()).map_or(PADDING + style.font_size, |l| l.baseline as f64);
                ((pos.0 - PADDING).round(), (pos.1 - baseline).round())
            }
        };
        if fonts::library().is_empty() {
            *ctx.status = Some("No fonts are installed, so text can't be drawn.".into());
            return;
        }
        self.session = Some(Session {
            doc: ctx.project.doc.id,
            layer: None,
            created: false,
            origin,
            layout: layout::layout(&style),
            style,
            caret: 0,
            anchor: 0,
            recorded: false,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            goal_x: None,
            changed_at: Instant::now(),
        });
    }

    /// Writes the session's style to its layer (making the layer for new text). False when the
    /// text can't be drawn, leaving the document as it was.
    fn write(&mut self, project: &mut Project) -> bool {
        let Some(session) = self.session.as_mut() else { return false };
        let Some(layout) = layout::layout(&session.style) else { return false };
        if session.layer.is_none() && session.style.content.is_empty() {
            session.layout = Some(layout);
            return true;
        }
        if !session.recorded {
            project.begin_edit(if session.layer.is_some() { "Edit Text" } else { "New Text Layer" });
            project.finish_edit();
            session.recorded = true;
        }
        match session.layer {
            Some(id) => {
                let Some(layer) = project.doc.layer_mut(id) else { return false };
                if !text::restyle(layer, session.style.clone()) {
                    return false;
                }
                if session.created {
                    layer.name = text::layer_name(&session.style.content);
                }
            }
            None => {
                let image = layout.draw();
                let (w, h) = image.dimensions();
                let transform = LayerTransform::rect(session.origin.0, session.origin.1, w as f64, h as f64);
                let mut layer = Layer::new_pixel(text::layer_name(&session.style.content), image, transform);
                layer.text = Some(session.style.clone());
                session.layer = Some(project.doc.insert_above_active(layer));
                session.created = true;
                project.target = crate::project::EditTarget::Image;
            }
        }
        session.layout = Some(layout);
        session.changed_at = Instant::now();
        project.invalidate_all();
        true
    }

    /// Replaces the session's style and selection, recording a step for the session's own undo.
    fn change(&mut self, project: &mut Project, style: TextStyle, caret: usize, anchor: usize, kind: EditKind) -> bool {
        let Some(session) = self.session.as_mut() else { return false };
        if !style.is_valid() {
            return false;
        }
        let before = session.snapshot();
        if !(kind == EditKind::Typing && session.last_edit == Some(EditKind::Typing)) {
            session.undo.push(before.clone());
            session.redo.clear();
        }
        session.last_edit = Some(kind);
        session.style = style;
        session.caret = caret;
        session.anchor = anchor;
        session.goal_x = None;
        if self.write(project) {
            return true;
        }
        let session = self.session.as_mut().unwrap();
        session.style = before.style;
        session.caret = before.caret;
        session.anchor = before.anchor;
        false
    }

    fn replace_selection(&mut self, project: &mut Project, text: &str, kind: EditKind) {
        let Some(session) = self.session.as_ref() else { return };
        let range = session.selection();
        let added = utf16_len(text);
        if session.style.len16() - range.len() + added > text::MAX_LENGTH {
            return;
        }
        let mut style = session.style.clone();
        style.replace(range.clone(), text);
        let caret = range.start + added;
        self.change(project, style, caret, caret, kind);
    }

    fn delete(&mut self, project: &mut Project, forward: bool, word: bool) {
        let Some(session) = self.session.as_mut() else { return };
        if session.caret == session.anchor {
            let content = &session.style.content;
            session.anchor = match (forward, word) {
                (true, true) => word_after(content, session.caret),
                (true, false) => step(content, session.caret, true),
                (false, true) => word_before(content, session.caret),
                (false, false) => step(content, session.caret, false),
            };
        }
        if session.caret != session.anchor {
            self.replace_selection(project, "", EditKind::Other);
        }
    }

    fn restore(&mut self, project: &mut Project, redo: bool) {
        let Some(session) = self.session.as_mut() else { return };
        let Some(snapshot) = (if redo { session.redo.pop() } else { session.undo.pop() }) else { return };
        let current = session.snapshot();
        if redo {
            session.undo.push(current);
        } else {
            session.redo.push(current);
        }
        session.style = snapshot.style;
        session.caret = snapshot.caret;
        session.anchor = snapshot.anchor;
        session.origin = snapshot.origin;
        session.last_edit = None;
        self.write(project);
    }

    fn move_caret(&mut self, key: Key, modifiers: Modifiers) {
        let Some(session) = self.session.as_mut() else { return };
        let content = &session.style.content;
        let total = utf16_len(content);
        let word = modifiers.command;
        let collapsing = !modifiers.shift && session.caret != session.anchor;
        let range = session.selection();
        let mut goal = None;
        let caret = match key {
            Key::ArrowLeft if collapsing => range.start,
            Key::ArrowRight if collapsing => range.end,
            Key::ArrowLeft if word => word_before(content, session.caret),
            Key::ArrowRight if word => word_after(content, session.caret),
            Key::ArrowLeft => step(content, session.caret, false),
            Key::ArrowRight => step(content, session.caret, true),
            Key::Home if word => 0,
            Key::End if word => total,
            Key::Home | Key::End | Key::ArrowUp | Key::ArrowDown => {
                let Some(layout) = session.layout.as_ref() else { return };
                let line = layout.line_of(session.caret);
                match key {
                    Key::Home => layout.lines[line].start,
                    Key::End => layout.lines[line].end,
                    _ => {
                        let x = session.goal_x.unwrap_or_else(|| layout.caret_x(line, session.caret));
                        goal = Some(x);
                        match key {
                            Key::ArrowUp if line == 0 => 0,
                            Key::ArrowUp => layout.hit_in_line(line - 1, x),
                            _ if line + 1 >= layout.lines.len() => total,
                            _ => layout.hit_in_line(line + 1, x),
                        }
                    }
                }
            }
            _ => return,
        };
        session.caret = caret;
        if !modifiers.shift {
            session.anchor = caret;
        }
        session.goal_x = goal;
        session.last_edit = None;
        session.changed_at = Instant::now();
    }

    fn key_event(&mut self, ctx: &mut ToolCtx, key: Key, modifiers: Modifiers) {
        // Alt with the arrows sets spacing, as in Photoshop: left and right the tracking, up and
        // down the leading. Shift makes each step ten.
        if modifiers.alt && matches!(key, Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown) {
            let Some(session) = self.session.as_ref() else { return };
            let step = if modifiers.shift { 10.0 } else { 1.0 };
            let mut style = session.style.clone();
            match key {
                Key::ArrowLeft => style.tracking = (style.tracking - step).max(-100.0),
                Key::ArrowRight => style.tracking = (style.tracking + step).min(1000.0),
                Key::ArrowUp => style.leading = (style.line_height() - step).max(1.0),
                _ => style.leading = (style.line_height() + step).min(5000.0),
            }
            let (caret, anchor) = (session.caret, session.anchor);
            self.change(ctx.project, style, caret, anchor, EditKind::Other);
            return;
        }
        match key {
            Key::Escape => self.commit(ctx),
            Key::Enter if modifiers.command => self.commit(ctx),
            Key::Enter => self.replace_selection(ctx.project, "\n", EditKind::Typing),
            Key::Backspace => self.delete(ctx.project, false, modifiers.command || modifiers.alt),
            Key::Delete => self.delete(ctx.project, true, modifiers.command || modifiers.alt),
            Key::A if modifiers.command => {
                if let Some(session) = self.session.as_mut() {
                    session.anchor = 0;
                    session.caret = session.style.len16();
                }
            }
            Key::Z if modifiers.command => self.restore(ctx.project, modifiers.shift),
            Key::Y if modifiers.command => self.restore(ctx.project, true),
            Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown | Key::Home | Key::End => self.move_caret(key, modifiers),
            _ => {}
        }
    }

    /// Typing, shortcuts and the clipboard while editing.
    fn handle_input(&mut self, ui: &egui::Ui, ctx: &mut ToolCtx) {
        if self.session.is_none() || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let events = ui.input(|i| i.events.clone());
        for event in events {
            if self.session.is_none() {
                break;
            }
            match event {
                Event::Text(text) => {
                    let text = clean(&text);
                    if !text.is_empty() {
                        self.replace_selection(ctx.project, &text, EditKind::Typing);
                    }
                }
                Event::Ime(egui::ImeEvent::Commit(text)) => self.replace_selection(ctx.project, &clean(&text), EditKind::Typing),
                Event::Paste(text) => self.replace_selection(ctx.project, &clean(&text), EditKind::Other),
                Event::Copy | Event::Cut => {
                    let Some(session) = self.session.as_ref() else { continue };
                    let range = session.selection();
                    if range.is_empty() {
                        continue;
                    }
                    let content = &session.style.content;
                    let selected = content[text::byte_index(content, range.start)..text::byte_index(content, range.end)].to_string();
                    ui.ctx().copy_text(selected);
                    if matches!(event, Event::Cut) {
                        self.replace_selection(ctx.project, "", EditKind::Other);
                    }
                }
                Event::Key { key, pressed: true, modifiers, .. } => self.key_event(ctx, key, modifiers),
                _ => {}
            }
        }
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }

    /// A type setting changed in the options bar: the selected letters (or all of them) while
    /// editing, the active text layer otherwise, or the settings for new text.
    fn apply_change(&mut self, project: &mut Project, change: text::StyleChange) {
        if let Some(session) = self.session.as_ref() {
            let mut style = session.style.clone();
            change.apply(&mut style, session.selection());
            let (caret, anchor) = (session.caret, session.anchor);
            if style != session.style {
                self.change(project, style, caret, anchor, EditKind::Other);
            }
        } else if let Some(style) = project.doc.active_layer().and_then(|l| l.text.clone()) {
            let mut next = style.clone();
            change.apply(&mut next, 0..0);
            if next != style {
                crate::ui::properties::change(project, |doc| {
                    if let Some(layer) = doc.active_layer_mut() {
                        text::restyle(layer, next);
                    }
                });
            }
        }
        if self.session.is_none() || !matches!(change, text::StyleChange::Color(_)) {
            let mut defaults = self.defaults();
            change.apply(&mut defaults, 0..0);
            self.defaults = Some(defaults.as_defaults());
        }
    }

    fn handle_at(&self, project: &Project, pos: (f64, f64), zoom: f32) -> Option<usize> {
        let session = self.session.as_ref()?;
        session.style.box_size?;
        let (w, h) = session.layout.as_ref().map(|l| (l.width as f64, l.height as f64))?;
        let map = session.to_document(project);
        let radius = 7.0 / zoom as f64;
        HANDLES.iter().position(|(u, v)| {
            let (x, y) = map.apply(u * w, v * h);
            (x - pos.0).hypot(y - pos.1) <= radius
        })
    }

    fn resize(&mut self, project: &mut Project, pos: (f64, f64), handle: usize, start: LayerTransform, pixels: (u32, u32), start_box: [f64; 2]) {
        let map = start.pixel_to_document(pixels.0, pixels.1);
        let Some(inverse) = map.inverse() else { return };
        let (u, v) = inverse.apply(pos.0, pos.1);
        let (mut x0, mut y0, mut x1, mut y1) = (0.0, 0.0, start_box[0], start_box[1]);
        let (hu, hv) = HANDLES[handle];
        if hu == 0.0 {
            x0 = u.min(x1 - 16.0);
        } else if hu == 1.0 {
            x1 = u.max(x0 + 16.0);
        }
        if hv == 0.0 {
            y0 = v.min(y1 - 16.0);
        } else if hv == 1.0 {
            y1 = v.max(y0 + 16.0);
        }
        let size = [(x1 - x0).round(), (y1 - y0).round()];
        let unit = Affine::scale(size[0], size[1]).then(&Affine::translate(x0.round(), y0.round())).then(&map);
        let mut transform = LayerTransform::from_affine(&unit, start.rotation, start.flip_x);
        transform.sampling = start.sampling;
        let Some(session) = self.session.as_mut() else { return };
        session.style.box_size = Some(size);
        session.last_edit = Some(EditKind::Other);
        if session.layer.is_none() {
            session.origin = (transform.origin[0], transform.origin[1]);
        }
        let layer = session.layer;
        if self.write(project) {
            if let Some(layer) = layer.and_then(|id| project.doc.layer_mut(id)) {
                layer.transform = transform;
            }
        }
    }
}

impl Tool for TypeTool {
    fn kind(&self) -> ToolKind {
        ToolKind::Type
    }
    fn name(&self) -> &'static str {
        "Type"
    }
    fn icon(&self) -> &'static str {
        crate::ui::icons::TEXT_T
    }
    fn shortcut(&self) -> Option<Key> {
        Some(Key::T)
    }

    fn options_ui(&mut self, ui: &mut egui::Ui, ctx: &mut ToolCtx) {
        fonts::warm_up();
        self.validate(ctx.project);
        self.handle_input(ui, ctx);
        let (style, range) = match &self.session {
            Some(session) => (session.style.clone(), session.selection()),
            None => match ctx.project.doc.active_layer().and_then(|l| l.text.clone()) {
                Some(style) => (style, 0..0),
                None => (self.defaults(), 0..0),
            },
        };
        if let Some(change) = text::controls(ui, "options", &style, range) {
            self.apply_change(ctx.project, change);
        }
        if self.session.is_some() {
            ui.separator();
            if ui.button("Commit").on_hover_text("Finish editing (Escape or Ctrl+Enter)").clicked() {
                self.commit(ctx);
            }
        }
    }

    fn pointer(&mut self, event: &PointerEvent, ctx: &mut ToolCtx) {
        self.validate(ctx.project);
        let pos = event.pos;
        match event.phase {
            PointerPhase::DoubleClick => {
                let Some(session) = self.session.as_mut() else { return };
                if !session.placement(ctx.project).0.contains(pos.0, pos.1) {
                    return;
                }
                let at = session.hit(ctx.project, pos);
                let content = &session.style.content;
                session.anchor = word_before(content, step(content, at, true));
                session.caret = word_after(content, session.anchor);
                self.drag = None;
                self.double_click = Some(Instant::now());
            }
            PointerPhase::Press => {
                if self.double_click.take().is_some_and(|t| t.elapsed() < Duration::from_millis(50)) {
                    return;
                }
                if let Some(handle) = self.handle_at(ctx.project, pos, ctx.zoom) {
                    let session = self.session.as_mut().unwrap();
                    let (start, start_pixels) = session.placement(ctx.project);
                    let start_box = session.style.box_size.unwrap_or([start_pixels.0 as f64, start_pixels.1 as f64]);
                    session.undo.push(session.snapshot());
                    session.redo.clear();
                    self.drag = Some(Drag::Resize { handle, start, start_pixels, start_box });
                    return;
                }
                let inside = self.session.as_ref().is_some_and(|s| s.placement(ctx.project).0.contains(pos.0, pos.1));
                if inside {
                    let session = self.session.as_mut().unwrap();
                    session.caret = session.hit(ctx.project, pos);
                    if !event.modifiers.shift {
                        session.anchor = session.caret;
                    }
                    session.goal_x = None;
                    session.last_edit = None;
                    session.changed_at = Instant::now();
                    self.drag = Some(Drag::Select);
                    return;
                }
                self.commit(ctx);
                if let Some(id) = Self::text_layer_at(ctx.project, pos) {
                    self.edit_layer(ctx, id, Some(pos));
                    self.drag = Some(Drag::Select);
                } else {
                    self.drag = Some(Drag::NewBox);
                    self.drag_from = Some(pos);
                    self.drag_to = Some(pos);
                }
            }
            PointerPhase::Drag => match self.drag {
                Some(Drag::Select) => {
                    if let Some(session) = self.session.as_mut() {
                        session.caret = session.hit(ctx.project, pos);
                    }
                }
                Some(Drag::NewBox) => self.drag_to = Some(pos),
                Some(Drag::Resize { handle, start, start_pixels, start_box }) => {
                    self.resize(ctx.project, pos, handle, start, start_pixels, start_box);
                }
                None => {}
            },
            PointerPhase::Release => {
                if let Some(Drag::NewBox) = self.drag.take() {
                    // A click makes point text; a drag, a paragraph box.
                    let origin = self.drag_from.unwrap_or(event.press_origin);
                    let (x0, y0, x1, y1) = (origin.0.min(pos.0), origin.1.min(pos.1), origin.0.max(pos.0), origin.1.max(pos.1));
                    let small = 4.0 / ctx.zoom as f64;
                    if x1 - x0 > small && y1 - y0 > small {
                        self.begin_new(ctx, origin, Some((x0, y0, x1, y1)));
                    } else {
                        self.begin_new(ctx, origin, None);
                    }
                }
                self.drag_from = None;
                self.drag_to = None;
            }
            PointerPhase::Hover => {}
        }
    }

    fn overlay(&self, painter: &Painter, view: &ViewTransform, project: &Project, _hover: Option<(f64, f64)>) {
        let accent = Color32::from_rgb(0, 150, 255);
        if let (Some(Drag::NewBox), Some(from), Some(to)) = (&self.drag, self.drag_from, self.drag_to) {
            let rect = egui::Rect::from_two_pos(view.to_screen(from.0, from.1), view.to_screen(to.0, to.1));
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0, accent), egui::StrokeKind::Middle);
        }
        let Some(session) = &self.session else { return };
        if session.doc != project.doc.id {
            return;
        }
        let map = session.to_document(project);
        let (transform, _) = session.placement(project);
        let to_screen = |x: f32, y: f32| -> Pos2 {
            let (dx, dy) = map.apply(x as f64, y as f64);
            view.to_screen(dx, dy)
        };
        // The box.
        let corners: Vec<Pos2> = transform.corners().iter().map(|(x, y)| view.to_screen(*x, *y)).collect();
        let dashed = egui::Shape::dashed_line(&[corners[0], corners[1], corners[2], corners[3], corners[0]], Stroke::new(1.0, accent), 4.0, 3.0);
        painter.extend(dashed);
        if session.style.box_size.is_some() {
            if let Some(layout) = &session.layout {
                for (u, v) in HANDLES {
                    let p = to_screen(u as f32 * layout.width as f32, v as f32 * layout.height as f32);
                    let r = egui::Rect::from_center_size(p, egui::vec2(7.0, 7.0));
                    painter.rect_filled(r, 0.0, Color32::WHITE);
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0, accent), egui::StrokeKind::Inside);
                }
            }
        }
        let Some(layout) = &session.layout else { return };
        // Selection.
        let fill = Color32::from_rgba_unmultiplied(0, 120, 255, 70);
        for (x0, y0, x1, y1) in layout.selection_rects(session.selection()) {
            let points = vec![to_screen(x0, y0), to_screen(x1, y0), to_screen(x1, y1), to_screen(x0, y1)];
            painter.add(egui::Shape::convex_polygon(points, fill, Stroke::NONE));
        }
        // Caret, blinking.
        let blink = session.changed_at.elapsed().as_millis() % 1000 < 600;
        if session.caret == session.anchor && blink {
            let line = layout.line_of(session.caret);
            if let Some(l) = layout.lines.get(line) {
                let x = layout.caret_x(line, session.caret);
                painter.line_segment([to_screen(x, l.top), to_screen(x, l.top + layout.line_height)], Stroke::new(1.5, accent));
            }
        }
        // Text a box is too small for.
        if layout.lines.iter().any(|l| !l.visible) {
            let p = to_screen(layout.width as f32 - 6.0, layout.height as f32 - 6.0);
            painter.text(p, egui::Align2::RIGHT_BOTTOM, "+", egui::FontId::proportional(14.0), Color32::RED);
        }
        painter.ctx().request_repaint_after(Duration::from_millis(100));
    }

    fn cursor(&self, project: &Project, hover: (f64, f64), _modifiers: Modifiers) -> CursorIcon {
        if let Some(handle) = self.handle_at(project, hover, project.view.zoom) {
            return match handle {
                0 | 4 => CursorIcon::ResizeNwSe,
                2 | 6 => CursorIcon::ResizeNeSw,
                1 | 5 => CursorIcon::ResizeVertical,
                _ => CursorIcon::ResizeHorizontal,
            };
        }
        CursorIcon::Text
    }

    fn commit(&mut self, ctx: &mut ToolCtx) {
        self.validate(ctx.project);
        self.drag = None;
        let Some(session) = self.session.take() else { return };
        if let Some(id) = session.layer {
            if session.created && session.style.content.trim().is_empty() {
                ctx.project.doc.remove_layers(&[id]);
                ctx.project.invalidate_all();
            }
        }
        let defaults = session.style.as_defaults();
        self.defaults = Some(TextStyle { font_name: session.style.font_name_at(0).to_string(), ..defaults });
    }

    fn cancel(&mut self, ctx: &mut ToolCtx) {
        self.commit(ctx);
    }

    fn captures_keyboard(&self) -> bool {
        self.session.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_by_letters_and_words() {
        let s = "ab😀 cd";
        assert_eq!(step(s, 2, true), 4);
        assert_eq!(step(s, 4, false), 2);
        assert_eq!(word_before("hello world", 11), 6);
        assert_eq!(word_before("hello world", 6), 0);
        assert_eq!(word_after("hello world", 0), 5);
        assert_eq!(word_after("hello world", 5), 11);
        assert_eq!(clean("a\r\nb\u{7}c"), "a\nbc");
    }
}
