//! Large files retain just the displayed section, without highlighting or undo history.
use super::*;
#[derive(Clone)]
pub(super) struct Page {
    pub section: files::Page,
    /// Offsets of the sections shown before this one, for Previous.
    previous: Vec<u64>,
}
pub(super) struct Loaded {
    pub(super) content: String,
    pub(super) revision: String,
    pub(super) crlf: bool,
    pub(super) bom: bool,
    pub(super) page: Option<Page>,
}
impl Loaded {
    pub(super) fn parse(document: files::Document) -> Result<Self, String> {
        let raw = document.content.as_str();
        let page = document.page.map(|section| Page {
            section,
            previous: Vec::new(),
        });
        if document.revision.is_empty() {
            return Err("The server returned no file revision.".into());
        }
        let revision = document.revision;
        let (content, crlf, bom) = if let Some(Page { section: page, .. }) = &page {
            let end = page
                .offset
                .checked_add(raw.len() as u64)
                .ok_or("Invalid file section.")?;
            if raw.is_empty()
                || raw.len() > 64 * 1024
                || end > page.size
                || page.next != (end < page.size).then_some(end)
            {
                return Err("Invalid file section. Reload the file.".into());
            }
            newport_domain::text_file::validate(raw).map_err(str::to_owned)?;
            // Read-only viewing can normalize mixed endings without risking a save.
            let bom = page.offset == 0 && raw.starts_with('\u{feff}');
            let content = if bom { &raw[3..] } else { raw };
            (
                content.replace("\r\n", "\n").replace('\r', "\n"),
                raw.contains("\r\n"),
                bom,
            )
        } else {
            if raw.len() > newport_domain::text_file::EDIT_LIMIT {
                return Err(
                    "This file exceeds the 16 MiB editing limit. Reopen it in sections.".into(),
                );
            }
            if newport_domain::text_file::has_long_lines(raw) {
                return Err("This file has very long lines. Reopen it in sections.".into());
            }
            decode(raw)?
        };
        Ok(Self {
            content,
            revision,
            crlf,
            bom,
            page,
        })
    }
}
impl CodeWorkspace {
    pub(super) fn document_buffer(
        path: String,
        document: files::Document,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Buffer, String> {
        let loaded = Loaded::parse(document)?;
        let paged = loaded.page.is_some();
        let editor = cx.new(|cx| {
            let mut editor = crate::controls::code_editor(window, cx)
                // Sections may start mid-line; do not claim page-local numbers are file lines.
                .line_number(!paged)
                .searchable(true)
                .replaceable(!paged)
                .soft_wrap(false)
                .scroll_beyond_last_line(Some(0))
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: loaded.content.lines().any(|line| line.starts_with('\t')),
                })
                .placeholder("(Empty file)");
            if !paged
                && loaded.content.len() <= newport_domain::text_file::HIGHLIGHT_LIMIT
                && let Some(language) = git_diff_syntax::language(&path)
            {
                editor = editor.language(language);
            }
            editor.set_value(loaded.content.clone(), window, cx);
            editor.set_readonly(paged, cx);
            editor
        });
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |this, source, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change)
                    && let Some(buffer) = this.buffers.iter_mut().find(|b| b.editor == *source)
                {
                    buffer
                        .changed
                        .set(buffer.page.is_none() && source.read(cx).text() != &buffer.saved);
                    let path = buffer.path.clone();
                    this.finish_language_completion(&path, window, cx);
                    this.schedule_language_sync(&path, cx);
                }
                cx.notify();
            },
        );
        Ok(Buffer {
            path,
            editor,
            saved: loaded.content,
            revision: loaded.revision,
            crlf: loaded.crlf,
            bom: loaded.bom,
            saving: false,
            changed: std::cell::Cell::new(false),
            error: None,
            conflict: None,
            page: loaded.page,
            failure: None,
            unread: false,
            lsp: None,
            lsp_sync: None,
            _subscription: subscription,
        })
    }

    /// A tab restored from the last run, whose file is read when the tab is
    /// first shown. Until then it holds no text, as a failed tab doesn't.
    pub(super) fn unread_buffer(
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Buffer {
        Buffer {
            unread: true,
            ..Self::failed_buffer(path, String::new(), window, cx)
        }
    }

    /// A tab for a file that could not be opened. It selects, closes and
    /// retries like any tab, but holds no text and explains why in its place.
    pub(super) fn failed_buffer(
        path: String,
        failure: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Buffer {
        let editor = cx.new(|cx| {
            let mut editor = crate::controls::code_editor(window, cx);
            editor.set_readonly(true, cx);
            editor
        });
        let subscription = cx.subscribe(&editor, |_, _, _: &InputEvent, _| {});
        Buffer {
            path,
            editor,
            saved: String::new(),
            revision: String::new(),
            crlf: false,
            bom: false,
            saving: false,
            changed: std::cell::Cell::new(false),
            error: None,
            conflict: None,
            page: None,
            failure: Some(failure),
            unread: false,
            lsp: None,
            lsp_sync: None,
            _subscription: subscription,
        }
    }
    fn turn_section(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.active.and_then(|i| self.buffers.get(i)) else {
            return;
        };
        let Some(page) = &buffer.page else {
            return;
        };
        let mut previous = page.previous.clone();
        let offset = if forward {
            let Some(next) = page.section.next else {
                return;
            };
            previous.push(page.section.offset);
            next
        } else {
            let Some(offset) = previous.pop() else {
                return;
            };
            offset
        };
        self.read_section(buffer.path.clone(), Some((offset, previous)), window, cx);
    }
    pub(super) fn read_section(
        &mut self,
        path: String,
        section: Option<(u64, Vec<u64>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(buffer) = self.buffers.iter().find(|b| b.path == path) else {
            return;
        };
        if buffer.saving || !self.available || !self.opening.insert(path.clone()) {
            return;
        }
        let captured = buffer.editor.read(cx).value().to_string();
        let identity = buffer.editor.entity_id();
        let operation = self.read_request(
            &path,
            section.as_ref().map(|_| buffer.revision.clone()),
            section.as_ref().map(|(offset, _)| *offset),
            false,
        );
        self.rpc(operation, window, cx, move |this, result, window, cx| {
            // Entity identity prevents a late response from replacing a reopened tab.
            let Some(index) = this.buffers.iter().position(|b| b.path == path && b.editor.entity_id() == identity) else { return; };
            this.opening.remove(&path);
            let buffer = &mut this.buffers[index];
            if buffer.saving || buffer.editor.read(cx).text() != &captured {
                buffer.error = Some("You edited the file while it was reloading. Your edits were kept; reload again when ready.".into());
                return;
            }
            match result.and_then(document_reply).and_then(|document| Self::document_buffer(path.clone(), document, window, cx)) {
                Ok(mut loaded) => {
                    if let Some((offset, previous)) = section {
                        let Some(page) = &mut loaded.page else { this.buffers[index].error = Some("The file changed. Reload it.".into()); return; };
                        if page.section.offset != offset || loaded.revision != this.buffers[index].revision {
                            this.buffers[index].error = Some("The file changed. Reload it.".into()); return;
                        }
                        page.previous = previous;
                    }
                    // Drop the old editor so page changes do not accumulate undo ropes.
                    this.buffers[index] = loaded;
                    if this.active == Some(index) && this.visible && !this.reviewing {
                        this.buffers[index].editor.update(cx, |e, cx| e.focus(window, cx));
                    }
                }
                Err(error) => this.buffers[index].error = Some(error),
            }
        });
        cx.notify();
    }
    pub(super) fn section_controls(
        &self,
        buffer: &Buffer,
        page: &Page,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.opening.contains(&buffer.path) || !self.available;
        let previous = &page.previous;
        let page = &page.section;
        let end = page.next.unwrap_or(page.size);
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .h(px(36.))
            .flex_shrink_0()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_size(px(crate::ui::FONT_SIZE))
            .text_color(cx.theme().muted_foreground)
            .child(div().flex_1().min_w_0().truncate().child(format!(
                "{} · Bytes {}–{} of {} · Search this section",
                match page.reason {
                    files::PageReason::LongLines => "Read-only · Very long lines",
                    files::PageReason::Size => "Read-only · Over 16 MiB",
                },
                page.offset + 1,
                end,
                page.size
            )))
            .child(
                crate::controls::compact_button("file-previous-section")
                    .label("Previous")
                    .ghost()
                    .disabled(busy || previous.is_empty())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.turn_section(false, window, cx)),
                    ),
            )
            .child(
                crate::controls::compact_button("file-next-section")
                    .label("Next")
                    .ghost()
                    .disabled(busy || page.next.is_none())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.turn_section(true, window, cx)),
                    ),
            )
    }
}
#[cfg(test)]
mod tests {
    use super::super::rpc_fixture::{document, section};
    use super::Loaded;
    #[test]
    fn reject_binary_and_malformed_or_unbounded_documents() {
        for content in ["%PDF-1.7", "hello\0world", "GIF89a"] {
            assert!(Loaded::parse(document(content, "r")).is_err());
        }
        assert!(Loaded::parse(document("text", "")).is_err());
        // Separate the size guard from the pathological long-line guard.
        let oversized = "x\n".repeat(newport_domain::text_file::EDIT_LIMIT / 2) + "x";
        assert!(Loaded::parse(document(&oversized, "r")).is_err());
        let long = "x".repeat(newport_domain::text_file::MAX_EDIT_LINE + 1);
        assert!(Loaded::parse(document(&long, "r")).is_err());
        assert!(Loaded::parse(section("text", "r", 0, Some(5), 10)).is_err());
        assert!(Loaded::parse(section("text", "r", u64::MAX, None, u64::MAX)).is_err());
    }
    #[test]
    fn large_sections_can_display_mixed_endings_without_becoming_editable() {
        let loaded = Loaded::parse(section("a\r\nb\nc\r", "page:r", 0, None, 7)).unwrap();
        assert!(loaded.page.is_some());
        assert_eq!(loaded.content, "a\nb\nc\n");
    }
}
