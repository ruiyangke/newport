//! A keyboard cursor for the lazy project tree, independent of the open document.
use super::*;

impl CodeWorkspace {
    pub(crate) fn focus_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.tree_rows();
        let path = self
            .tree_cursor
            .as_ref()
            .filter(|p| rows.iter().any(|row| &row.path == *p))
            .or_else(|| {
                self.active
                    .and_then(|i| self.buffers.get(i))
                    .map(|b| &b.path)
                    .filter(|p| rows.iter().any(|row| &row.path == *p))
            })
            .or_else(|| rows.first().map(|row| &row.path))
            .cloned();
        self.tree_cursor = path;
        if let Some(index) = rows
            .iter()
            .position(|r| Some(&r.path) == self.tree_cursor.as_ref())
        {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
        self.tree_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn tree_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.tree_focus.is_focused(window) || window.has_active_dialog(cx) {
            return;
        }
        let key = &event.keystroke;
        if key.modifiers.platform
            || key.modifiers.control
            || key.modifiers.alt
            || key.modifiers.shift
        {
            return;
        }
        if !matches!(
            key.key.as_str(),
            "up" | "down"
                | "left"
                | "right"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
                | "enter"
                | "escape"
        ) {
            return;
        }
        window.prevent_default();
        cx.stop_propagation();
        self.reveal_target = None;
        if key.key == "escape" {
            self.focus_document(window, cx);
            cx.notify();
            return;
        }
        let rows = self.tree_rows();
        if rows.is_empty() {
            return;
        }
        let current = self
            .tree_cursor
            .as_ref()
            .and_then(|path| rows.iter().position(|r| &r.path == path))
            .or_else(|| {
                self.active
                    .and_then(|i| self.buffers.get(i))
                    .and_then(|b| rows.iter().position(|r| r.path == b.path))
            })
            .unwrap_or(0);
        let row = &rows[current];
        let mut next = current;
        match key.key.as_str() {
            "up" => next = current.saturating_sub(1),
            "down" => next = (current + 1).min(rows.len() - 1),
            "home" => next = 0,
            "end" => next = rows.len() - 1,
            "pageup" => next = current.saturating_sub(15),
            "pagedown" => next = (current + 15).min(rows.len() - 1),
            "left" => {
                if row.directory && self.expanded.remove(&row.path) {
                    // Keep the collapsed folder selected.
                } else if row.depth > 0 {
                    next = rows[..current]
                        .iter()
                        .rposition(|r| r.depth < row.depth)
                        .unwrap_or(current);
                }
            }
            "right" if row.directory => {
                if self.expanded.insert(row.path.clone()) {
                    if !self.directories.contains_key(&row.path) {
                        self.list(row.path.clone(), window, cx);
                    }
                } else if rows.get(current + 1).is_some_and(|r| r.depth > row.depth) {
                    next += 1;
                }
            }
            "enter" if row.directory => {
                if !self.expanded.remove(&row.path) {
                    self.expanded.insert(row.path.clone());
                    if !self.directories.contains_key(&row.path) {
                        self.list(row.path.clone(), window, cx);
                    }
                }
            }
            "enter" => {
                self.tree_cursor = Some(row.path.clone());
                self.open(row.path.clone(), window, cx);
                return;
            }
            _ => {}
        }
        self.tree_cursor = Some(rows[next].path.clone());
        self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }
}
