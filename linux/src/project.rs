//! One open document (a tab): its layers, history, file location and view.

use std::path::PathBuf;

use crate::doc::history::History;
use crate::doc::Document;
use crate::ui::canvas::CanvasView;

/// Which pixels painting tools change on the active layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EditTarget {
    #[default]
    Image,
    Mask,
}

/// Document rectangle in pixels, `(x0, y0, x1, y1)` exclusive.
pub type DocRect = (i64, i64, i64, i64);

pub struct Project {
    pub doc: Document,
    pub history: History,
    /// Where the project was opened from or saved to (`None` until first saved as a `.comp`).
    pub path: Option<PathBuf>,
    pub title: String,
    pub view: CanvasView,
    pub target: EditTarget,
    /// Areas of the canvas to redraw; `None` in the list means everything.
    pub dirty: Vec<Option<DocRect>>,
    /// Set while a multi-step edit (a stroke, a drag) is in progress.
    pub pending_edit: Option<String>,
    /// Fingerprint of the project as last read or written, to notice changes made by others.
    pub disk_digest: Option<u64>,
    /// A newer fingerprint seen on disk and when (seconds), reloaded once writes settle.
    pub pending_digest: Option<(u64, f64)>,
    /// The selection most recently cleared by an edit, for Select > Reselect.
    pub last_selection: Option<crate::doc::Selection>,
}

impl Project {
    pub fn new(doc: Document, path: Option<PathBuf>, title: String) -> Self {
        Project {
            doc,
            history: History::default(),
            path,
            title,
            view: CanvasView::default(),
            target: EditTarget::Image,
            dirty: vec![None],
            pending_edit: None,
            disk_digest: None,
            pending_digest: None,
            last_selection: None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    pub fn display_title(&self) -> String {
        if self.is_dirty() {
            format!("{} •", self.title)
        } else {
            self.title.clone()
        }
    }

    pub fn invalidate_all(&mut self) {
        self.dirty.push(None);
    }

    pub fn invalidate(&mut self, rect: DocRect) {
        self.dirty.push(Some(rect));
    }

    /// Records the current state for undo under `name`, then runs `edit`. Everything redraws.
    pub fn edit<R>(&mut self, name: &str, edit: impl FnOnce(&mut Document) -> R) -> R {
        let before = self.doc.selection.clone();
        self.history.push(name, self.doc.clone());
        let result = edit(&mut self.doc);
        if self.doc.selection.is_none() {
            // Remember what Deselect (or anything else) cleared, while it still fits the canvas.
            if let Some(selection) = before.filter(|s| s.mask.dimensions() == (self.doc.width, self.doc.height)) {
                self.last_selection = Some(selection);
            }
        }
        self.invalidate_all();
        result
    }

    /// Starts an edit made over several events (a drag); `finish_edit` ends it.
    pub fn begin_edit(&mut self, name: &str) {
        if self.pending_edit.is_none() {
            self.history.push(name, self.doc.clone());
            self.pending_edit = Some(name.to_string());
        }
    }

    pub fn finish_edit(&mut self) {
        self.pending_edit = None;
    }

    pub fn undo(&mut self) {
        self.pending_edit = None;
        if self.history.undo(&mut self.doc).is_some() {
            self.invalidate_all();
        }
    }

    pub fn redo(&mut self) {
        self.pending_edit = None;
        if self.history.redo(&mut self.doc).is_some() {
            self.invalidate_all();
        }
    }
}
