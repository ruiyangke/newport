//! Large files retain just the displayed section, without highlighting or undo history.
use super::*;
#[derive(Clone, serde::Deserialize)]
pub(super) struct Page {
    pub offset: u64,
    pub next: Option<u64>,
    pub size: u64,
    #[serde(default)]
    reason: Option<String>,
    #[serde(skip)]
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
    pub(super) fn parse(value: Value) -> Result<Self, String> {
        let raw = value["content"]
            .as_str()
            .ok_or("The server returned no file content.")?;
        let page: Option<Page> = value
            .get("page")
            .filter(|v| !v.is_null())
            .map(|v| {
                serde_json::from_value(v.clone()).map_err(|_| "Invalid file section.".to_owned())
            })
            .transpose()?;
        let revision = value["revision"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("The server returned no file revision.")?
            .to_owned();
        let (content, crlf, bom) = if let Some(page) = &page {
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
        value: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Buffer, String> {
        let loaded = Loaded::parse(value)?;
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
        let subscription = cx.subscribe(&editor, |this, source, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change)
                && let Some(buffer) = this.buffers.iter_mut().find(|b| b.editor == source)
            {
                buffer
                    .changed
                    .set(buffer.page.is_none() && source.read(cx).text() != &buffer.saved);
                let path = buffer.path.clone();
                this.schedule_language_sync(&path, cx);
            }
            cx.notify();
        });
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
            lsp: None,
            lsp_sync: None,
            _subscription: subscription,
        })
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
            let Some(next) = page.next else {
                return;
            };
            previous.push(page.offset);
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
        let mut args = self.args(&path);
        args["root"] = json!(self.root);
        if let Some((offset, _)) = &section {
            args["offset"] = json!(offset);
            args["revision"] = json!(buffer.revision);
        }
        self.rpc("files_read_text", args, window, cx, move |this, result, window, cx| {
            // Entity identity prevents a late response from replacing a reopened tab.
            let Some(index) = this.buffers.iter().position(|b| b.path == path && b.editor.entity_id() == identity) else { return; };
            this.opening.remove(&path);
            let buffer = &mut this.buffers[index];
            if buffer.saving || buffer.editor.read(cx).text() != &captured {
                buffer.error = Some("You edited the file while it was reloading. Your edits were kept; reload again when ready.".into());
                return;
            }
            match result.and_then(|value| Self::document_buffer(path.clone(), value, window, cx)) {
                Ok(mut loaded) => {
                    if let Some((offset, previous)) = section {
                        let Some(page) = &mut loaded.page else { this.buffers[index].error = Some("The file changed. Reload it.".into()); return; };
                        if page.offset != offset || loaded.revision != this.buffers[index].revision {
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
                match page.reason.as_deref() {
                    Some("long_lines") => "Read-only · Very long lines",
                    Some("size") => "Read-only · Over 16 MiB",
                    _ => "Read-only",
                },
                page.offset + 1,
                end,
                page.size
            )))
            .child(
                crate::controls::compact_button("file-previous-section")
                    .label("Previous")
                    .ghost()
                    .disabled(busy || page.previous.is_empty())
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
    use super::Loaded;
    use serde_json::json;
    #[test]
    fn reject_binary_and_malformed_or_unbounded_documents() {
        for content in ["%PDF-1.7", "hello\0world", "GIF89a"] {
            assert!(Loaded::parse(json!({"content":content,"revision":"r"})).is_err());
        }
        // Separate the size guard from the pathological long-line guard.
        let oversized = "x\n".repeat(newport_domain::text_file::EDIT_LIMIT / 2) + "x";
        assert!(Loaded::parse(json!({"content":oversized,"revision":"r"})).is_err());
        assert!(Loaded::parse(json!({"content":"x".repeat(newport_domain::text_file::MAX_EDIT_LINE + 1),"revision":"r"})).is_err());
        assert!(
            Loaded::parse(
                json!({"content":"text","revision":"r","page":{"offset":0,"next":5,"size":10}})
            )
            .is_err()
        );
        assert!(Loaded::parse(json!({"content":"text","revision":"r","page":{"offset":u64::MAX,"next":null,"size":u64::MAX}})).is_err());
    }
    #[test]
    fn large_sections_can_display_mixed_endings_without_becoming_editable() {
        let loaded = Loaded::parse(json!({"content":"a\r\nb\nc\r","revision":"page:r","page":{"offset":0,"next":null,"size":7}})).unwrap();
        assert!(loaded.page.is_some());
        assert_eq!(loaded.content, "a\nb\nc\n");
    }
}
