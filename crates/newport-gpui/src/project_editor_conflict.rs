//! Base/local/remote are separate until the user resolves a conflict.
use super::*;

pub(super) struct Conflict {
    remote: document::Loaded,
    preview: Option<Entity<EditorState>>,
    pub show_remote: bool,
}
impl Conflict {
    pub fn preview(&self) -> Option<&Entity<EditorState>> {
        self.show_remote.then_some(self.preview.as_ref()).flatten()
    }
}
impl CodeWorkspace {
    pub(super) fn retain_conflict(&mut self, index: usize, value: Value) -> Result<(), String> {
        let remote = document::Loaded::parse(value)?;
        if remote.page.is_some() {
            return Err("The remote file now requires section viewing. Your edits are kept; reload after copying the edits you need.".into());
        }
        let retained: usize = self
            .buffers
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .filter_map(|(_, b)| b.conflict.as_ref())
            .map(|c| c.remote.content.len())
            .sum();
        if retained + remote.content.len() > 64 * 1024 * 1024 {
            return Err("Remote snapshots reached the 64 MiB limit. Your edits are kept; resolve another conflict before reviewing this file.".into());
        }
        self.buffers[index].conflict = Some(Conflict {
            remote,
            preview: None,
            show_remote: false,
        });
        self.buffers[index].error = None;
        Ok(())
    }
    pub(super) fn review_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active else {
            return;
        };
        let buffer = &mut self.buffers[index];
        let Some(conflict) = &mut buffer.conflict else {
            return;
        };
        if conflict.show_remote {
            conflict.show_remote = false;
            buffer.editor.update(cx, |e, cx| e.focus(window, cx));
        } else {
            if conflict.preview.is_none() {
                let value = json!({"content":encode(&conflict.remote.content, conflict.remote.crlf, conflict.remote.bom),"revision":conflict.remote.revision});
                match Self::document_buffer(buffer.path.clone(), value, window, cx) {
                    Ok(preview) => {
                        preview.editor.update(cx, |e, cx| e.set_readonly(true, cx));
                        conflict.preview = Some(preview.editor);
                    }
                    Err(error) => {
                        buffer.error = Some(error);
                        return;
                    }
                }
            }
            conflict.show_remote = true;
            conflict
                .preview
                .as_ref()
                .unwrap()
                .update(cx, |e, cx| e.focus(window, cx));
        }
        cx.notify();
    }
    pub(super) fn merge_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active else {
            return;
        };
        let buffer = &mut self.buffers[index];
        let Some(conflict) = &buffer.conflict else {
            return;
        };
        let local = buffer.editor.read(cx).value().to_string();
        let Some(merged) = merge(&buffer.saved, &local, &conflict.remote.content) else {
            buffer.error = Some("These edits overlap or exceed the automatic merge limit. Review the remote version and then choose Use my edits to keep your version or Reload remote to discard it.".into());
            cx.notify();
            return;
        };
        let conflict = buffer.conflict.take().unwrap();
        buffer.saved = conflict.remote.content;
        buffer.revision = conflict.remote.revision;
        buffer.crlf = conflict.remote.crlf;
        buffer.bom = conflict.remote.bom;
        buffer.changed.set(merged != buffer.saved);
        buffer.error = None;
        buffer.editor.update(cx, |editor, cx| {
            let selection = editor.selected_range();
            let scroll = editor.scroll_offset();
            editor.replace_all(merged, window, cx); // retain undo history
            editor.set_selected_range(selection, cx);
            editor.set_scroll_offset(scroll, cx);
            editor.focus(window, cx);
        });
        cx.notify();
    }
    fn keep_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(buffer) = self.active.and_then(|i| self.buffers.get(i)) else {
            return;
        };
        let Some(conflict) = &buffer.conflict else {
            return;
        };
        let path = buffer.path.clone();
        let identity = buffer.editor.entity_id();
        let revision = conflict.remote.revision.clone();
        let owner = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |dialog,_,_| {
            let owner = owner.clone(); let path = path.clone(); let revision = revision.clone();
            dialog.title("Use your edits over this remote version?")
                .description("Your next save will replace the reviewed remote version with your buffer. Any newer remote change will still block the save.")
                .button_props(DialogButtonProps::default().show_cancel(true).ok_text("Use my edits").cancel_text("Keep reviewing"))
                .on_ok(move |_,window,cx| {
                    let _ = owner.update(cx,|this,cx| {
                        let Some(buffer) = this.buffers.iter_mut().find(|b|b.path==path && b.editor.entity_id()==identity) else { return; };
                        if !buffer.conflict.as_ref().is_some_and(|c|c.remote.revision==revision) { return; }
                        let conflict = buffer.conflict.take().unwrap();
                        buffer.crlf = conflict.remote.crlf;
                        buffer.bom = conflict.remote.bom;
                        buffer.saved = conflict.remote.content;
                        buffer.revision = conflict.remote.revision;
                        buffer.changed.set(buffer.editor.read(cx).text()!=&buffer.saved);
                        buffer.error = None;
                        buffer.editor.update(cx, |e,cx|e.focus(window,cx));
                        cx.notify();
                    });
                    true
                })
        });
    }
    pub(super) fn conflict_strip(&self, buffer: &Buffer, cx: &mut Context<Self>) -> Div {
        let remote = buffer.conflict.as_ref().is_some_and(|c| c.show_remote);
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .text_size(px(crate::ui::FONT_SIZE))
            .child(div().flex_1().child(if remote {
                "Remote version · read only"
            } else {
                "Changed remotely · your edits are kept"
            }))
            .child(
                crate::controls::compact_button("code-review-remote")
                    .ghost()
                    .label(if remote {
                        "Back to my edits"
                    } else {
                        "Review remote"
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.review_remote(window, cx))),
            )
            .child(
                crate::controls::compact_button("code-merge-remote")
                    .ghost()
                    .label("Merge changes")
                    .disabled(
                        !self.available || buffer.saving || self.opening.contains(&buffer.path),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.merge_remote(window, cx))),
            )
            .when(remote, |view| {
                view.child(
                    crate::controls::compact_button("code-keep-local")
                        .ghost()
                        .label("Use my edits")
                        .disabled(
                            !self.available || buffer.saving || self.opening.contains(&buffer.path),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.keep_local(window, cx))),
                )
            })
            .child(
                crate::controls::compact_button("code-reload-remote")
                    .ghost()
                    .label("Reload remote")
                    .disabled(
                        !self.available || buffer.saving || self.opening.contains(&buffer.path),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
            )
    }
}
// Conservative, linear-time three-way merge. Each side is represented by one
// changed span; ambiguous/overlapping edits stay unresolved rather than guessing.
fn merge(base: &str, local: &str, remote: &str) -> Option<String> {
    if local == remote {
        return Some(local.into());
    }
    if local == base {
        return Some(remote.into());
    }
    if remote == base {
        return Some(local.into());
    }
    if [base, local, remote].iter().any(|s| s.len() > 1024 * 1024) {
        return None;
    }
    let base: Vec<_> = base.split_inclusive('\n').collect();
    let local: Vec<_> = local.split_inclusive('\n').collect();
    let remote: Vec<_> = remote.split_inclusive('\n').collect();
    fn span<'a>(base: &[&str], next: &'a [&'a str]) -> (usize, usize, &'a [&'a str]) {
        let start = base.iter().zip(next).take_while(|(a, b)| a == b).count();
        let tail = base[start..]
            .iter()
            .rev()
            .zip(next[start..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        (start, base.len() - tail, &next[start..next.len() - tail])
    }
    let mut a = span(&base, &local);
    let mut b = span(&base, &remote);
    if a.0 > b.0 {
        std::mem::swap(&mut a, &mut b);
    }
    if a.1 > b.0 || a.0 == b.0 || (b.0 == b.1 && b.0 == a.1) {
        return None;
    }
    Some(
        base[..a.0]
            .iter()
            .chain(a.2)
            .chain(&base[a.1..b.0])
            .chain(b.2)
            .chain(&base[b.1..])
            .copied()
            .collect(),
    )
}
#[cfg(test)]
mod tests {
    use super::merge;
    #[test]
    fn merge_preserves_both_sides_and_refuses_overlaps() {
        assert_eq!(
            merge("a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n").as_deref(),
            Some("A\nb\nC\n")
        );
        assert_eq!(
            merge("a\nb\nc", "a\nlocal\nb\nc", "a\nb\nremote\nc").as_deref(),
            Some("a\nlocal\nb\nremote\nc")
        );
        assert_eq!(
            merge("a\nb\nc\n", "a\nc\n", "a\nb\nC\n").as_deref(),
            Some("a\nC\n")
        );
        assert!(merge("a\n", "x\na\n", "y\na\n").is_none());
        assert!(merge("a\nb\n", "a\nB\n", "a\nother\n").is_none());
        assert_eq!(merge("base", "remote", "remote").as_deref(), Some("remote"));
        assert!(merge(&"a".repeat(1024 * 1024 + 1), "local", "remote").is_none());
    }
}
