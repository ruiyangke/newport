//! Tag publishing keeps the selected object and remote configuration under review.
use super::*;
use crate::git_remote_picker::RemotePicker;

pub(super) struct Form {
    name: String,
    oid: String,
    snapshot: String,
    picker: Entity<RemotePicker>,
    _subscription: Subscription,
}
fn push_action(name: &str, oid: &str, remote: &Value) -> Option<Value> {
    let remote_name = remote["name"].as_str().filter(|s| !s.is_empty())?;
    let token = remote["token"].as_str().filter(|s| !s.is_empty())?;
    if name.is_empty() || oid.is_empty() {
        return None;
    }
    Some(
        json!({"kind":"tag.push","name":name,"expectedOid":oid,"remote":remote_name,"expectedToken":token}),
    )
}
impl Newport {
    fn prepare_tag_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_modify_tag()
            || self.git.tag_push.is_some()
            || self.git.tag_form.is_some()
            || self.git.tag_delete.is_some()
        {
            return;
        }
        let Some((name, oid)) = git_tags::tag_identity(&self.git.selected) else {
            return;
        };
        let server = self.server.clone();
        let repo = self.git_repository_id();
        let picker = cx.new(|cx| RemotePicker::new(server, repo, window, cx));
        let subscription = cx.observe(&picker, |_, _, cx| cx.notify());
        self.git.tag_push = Some(Form {
            name,
            oid,
            snapshot: text(&self.value("git-tag-status"), "snapshot"),
            picker,
            _subscription: subscription,
        });
        self.git.detail_open = true;
        cx.notify();
    }
    pub(super) fn sync_tag_push(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(form) = &self.git.tag_push {
            let enabled = !self.git_mutation_blocks_write();
            form.picker
                .update(cx, |picker, cx| picker.set_enabled(enabled, cx));
        }
    }
    fn can_push_tag(&self, cx: &App) -> bool {
        self.can_modify_tag()
            && self.git.tag_push.as_ref().is_some_and(|form| {
                form.snapshot == text(&self.value("git-tag-status"), "snapshot")
                    && git_tags::tag_identity(&self.git.selected)
                        == Some((form.name.clone(), form.oid.clone()))
                    && form
                        .picker
                        .read(cx)
                        .remote()
                        .and_then(|remote| push_action(&form.name, &form.oid, remote))
                        .is_some()
            })
    }
    fn push_tag(&mut self, cx: &mut Context<Self>) {
        if !self.can_push_tag(cx) {
            return;
        }
        let form = self.git.tag_push.as_ref().unwrap();
        let remote = form.picker.read(cx).remote().unwrap();
        let action = push_action(&form.name, &form.oid, remote).unwrap();
        let snapshot = form.snapshot.clone();
        let title = format!("Push tag {} to {}", form.name, text(remote, "name"));
        self.start_git_mutation(
            self.git_repository_id(),
            title,
            move |backend, server, repo, id| {
                backend.apply_reviewed_action(server, repo, &snapshot, action, id)
            },
            cx,
        );
    }
    pub(super) fn git_tag_push_trigger(&self, cx: &mut Context<Self>) -> Button {
        crate::project_ui::button("review-push-tag")
            .label("Push tag…")
            .disabled(
                !self.can_modify_tag()
                    || self.git.tag_push.is_some()
                    || self.git.tag_delete.is_some()
                    || self.git.tag_form.is_some(),
            )
            .on_click(cx.listener(|this, _, window, cx| this.prepare_tag_push(window, cx)))
    }
    pub(super) fn git_tag_push_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.git.tag_push else {
            return div().into_any_element();
        };
        let body = div().flex().flex_col().gap_3()
            .child(project_dialog::description("tag-push-identity", format!("{} · {}", form.name, form.oid.chars().take(12).collect::<String>())))
            .child(project_dialog::description("tag-push-description", "Push this exact tag to the selected remote. An existing remote tag will not be overwritten."))
            .child(form.picker.clone())
            .child(project_dialog::actions()
                .child(crate::project_ui::button("cancel-push-tag").label("Cancel").disabled(self.git_write_pending()).on_click(cx.listener(|this, _, _, cx| {this.git.tag_push=None; cx.notify();})))
                .child(crate::project_ui::button("confirm-push-tag").loading(self.git_write_pending()).label("Push tag").disabled(!self.can_push_tag(cx)).on_click(cx.listener(|this, _, _, cx| this.push_tag(cx)))));
        crate::git_listing::editor("Push tag", body, cx)
    }
}
#[cfg(test)]
mod tests {
    use super::push_action;
    use serde_json::json;
    #[test]
    fn publish_keeps_exact_tag_object_and_remote_configuration() {
        let remote =
            json!({"name":"backup","token":"reviewed","url":"/source","pushUrl":"/destination"});
        let action = push_action("v1", "tag-object-not-peeled-commit", &remote).unwrap();
        assert_eq!(
            action,
            json!({"kind":"tag.push","name":"v1","expectedOid":"tag-object-not-peeled-commit","remote":"backup","expectedToken":"reviewed"})
        );
        assert!(push_action("v1", "object", &json!({"name":"origin"})).is_none());
        assert!(push_action("", "object", &remote).is_none());
    }
}
