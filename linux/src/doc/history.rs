//! Undo and redo by whole-document snapshots. Layer pixels sit behind `Arc`s, so a snapshot
//! shares every image that an edit didn't touch.

use super::Document;

struct Entry {
    name: String,
    doc: Document,
    generation: u64,
}

pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    limit: usize,
    /// Every state the document passes through gets a number; the saved one is remembered.
    generation: u64,
    next_generation: u64,
    saved_generation: u64,
}

impl Default for History {
    fn default() -> Self {
        History { undo: Vec::new(), redo: Vec::new(), limit: 100, generation: 0, next_generation: 1, saved_generation: 0 }
    }
}

impl History {
    /// Records `before` (the document as it was before an edit named `name`).
    pub fn push(&mut self, name: impl Into<String>, before: Document) {
        self.undo.push(Entry { name: name.into(), doc: before, generation: self.generation });
        self.redo.clear();
        self.generation = self.next_generation;
        self.next_generation += 1;
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
    }

    pub fn undo(&mut self, current: &mut Document) -> Option<String> {
        let entry = self.undo.pop()?;
        let after = std::mem::replace(current, entry.doc);
        restore_session_state(current, &after);
        self.redo.push(Entry { name: entry.name.clone(), doc: after, generation: self.generation });
        self.generation = entry.generation;
        Some(entry.name)
    }

    pub fn redo(&mut self, current: &mut Document) -> Option<String> {
        let entry = self.redo.pop()?;
        let before = std::mem::replace(current, entry.doc);
        restore_session_state(current, &before);
        self.undo.push(Entry { name: entry.name.clone(), doc: before, generation: self.generation });
        self.generation = entry.generation;
        Some(entry.name)
    }

    pub fn undo_name(&self) -> Option<&str> {
        self.undo.last().map(|e| e.name.as_str())
    }

    pub fn redo_name(&self) -> Option<&str> {
        self.redo.last().map(|e| e.name.as_str())
    }

    pub fn mark_saved(&mut self) {
        self.saved_generation = self.generation;
    }

    /// Unsaved changes, without a save point: a new document that was never saved.
    pub fn mark_unsaved(&mut self) {
        self.saved_generation = u64::MAX;
    }

    pub fn is_dirty(&self) -> bool {
        self.saved_generation != self.generation
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.saved_generation = self.generation;
    }
}

/// Folder expansion is a view preference, not an edit: undo keeps what's on screen.
fn restore_session_state(doc: &mut Document, from: &Document) {
    for layer in &mut doc.layers {
        if let Some(other) = from.layer(layer.id) {
            layer.expanded = other.expanded;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_tracking() {
        let mut h = History::default();
        let mut doc = Document::new(10, 10, None);
        assert!(!h.is_dirty());
        h.push("A", doc.clone());
        doc.width = 20;
        assert!(h.is_dirty());
        h.undo(&mut doc);
        assert!(!h.is_dirty());
        assert_eq!(doc.width, 10);
        h.redo(&mut doc);
        assert!(h.is_dirty());
        h.mark_saved();
        h.undo(&mut doc);
        assert!(h.is_dirty());
        h.redo(&mut doc);
        assert!(!h.is_dirty());
    }
}
