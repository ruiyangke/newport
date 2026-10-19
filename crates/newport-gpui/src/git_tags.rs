use super::*;
use base64::Engine as _;
use gpui_kit::component::{
    input::TextareaState,
    select::{SelectEvent, SelectState},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Draft {
    id: uuid::Uuid,
    name: String,
    target: String,
    message: String,
    annotated: bool,
}
impl Draft {
    fn valid(&self) -> bool {
        !self.name.trim().is_empty()
            && self.target.trim().len() == 40
            && self.target.trim().bytes().all(|b| b.is_ascii_hexdigit())
            && (!self.annotated || !self.message.trim().is_empty())
    }
}

#[derive(Clone)]
pub(super) struct DeleteReview {
    name: String,
    oid: String,
    snapshot: String,
}
pub(super) fn tag_identity(tag: &Value) -> Option<(String, String)> {
    let name = tag["name"]["display"].as_str()?;
    let oid = tag["oid"]["hex"].as_str()?;
    (!name.is_empty()
        && !oid.is_empty()
        && tag["symbolicTarget"].is_null()
        && tag["name"]["bytesB64"] == base64::engine::general_purpose::STANDARD.encode(name))
    .then(|| (name.to_owned(), oid.to_owned()))
}
pub(super) struct Form {
    id: uuid::Uuid,
    name: Entity<InputState>,
    message: Entity<TextareaState>,
    target: Entity<InputState>,
    kind: Entity<SelectState<Vec<&'static str>>>,
    annotated: bool,
    snapshot: String,
    _subscriptions: Vec<Subscription>,
}
impl Form {
    pub(super) fn draft(&self, cx: &App) -> Draft {
        Draft {
            id: self.id,
            name: self.name.read(cx).value().to_string(),
            target: self.target.read(cx).value().to_string(),
            message: self.message.read(cx).value().to_string(),
            annotated: self.annotated,
        }
    }
}
fn target_commit<'a>(tab: usize, selected: &'a Value, status: &'a Value) -> Option<&'a str> {
    let oid = match tab {
        1 => &selected["oid"]["hex"],
        4 => &status["metadata"]["head"]["oid"]["hex"],
        _ => return None,
    };
    oid.as_str().filter(|value| !value.is_empty())
}
impl Newport {
    pub(super) fn can_modify_tag(&self) -> bool {
        self.git.tab == 4
            && !self.git_mutation_blocks_write()
            && self.git.repo["capabilities"]["readOnly"] == false
            && tag_identity(&self.git.selected).is_some()
            && self.git.error.is_none()
            && ["git-list", "git-tag-status"].iter().all(|key| {
                self.resources
                    .get(key)
                    .is_some_and(|r| !r.pending && r.error.is_none())
            })
            && self.value("git-tag-status")["snapshot"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
    }
    fn review_delete_tag(&mut self, cx: &mut Context<Self>) {
        if !self.can_modify_tag() || self.git.tag_form.is_some() || self.git.tag_push.is_some() {
            return;
        }
        let Some((name, oid)) = tag_identity(&self.git.selected) else {
            return;
        };
        self.git.tag_delete = Some(DeleteReview {
            name,
            oid,
            snapshot: text(&self.value("git-tag-status"), "snapshot"),
        });
        cx.notify();
    }
    fn delete_tag(&mut self, cx: &mut Context<Self>) {
        if !self.can_modify_tag() {
            return;
        }
        let Some(review) = self.git.tag_delete.clone() else {
            return;
        };
        self.start_git_mutation(
            self.git_repository_id(),
            format!("Delete tag {}", review.name),
            move |backend, server, repo, id| {
                backend.delete_tag(
                    server,
                    repo,
                    &review.snapshot,
                    (&review.name, &review.oid),
                    id,
                )
            },
            cx,
        );
    }

    pub(super) fn can_create_tag(&self) -> bool {
        matches!(self.git.tab, 1 | 4)
            && self.git.integration_review.is_none()
            && !self.git_mutation_blocks_write()
            && self.git.repo["capabilities"]["readOnly"] == false
            && self
                .resources
                .get("git-tag-status")
                .is_some_and(|r| !r.pending && r.error.is_none())
            && self.value("git-tag-status")["snapshot"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            && (self.git.tag_form.is_some()
                || target_commit(
                    self.git.tab,
                    &self.git.selected,
                    &self.value("git-tag-status"),
                )
                .is_some())
    }
    pub(super) fn new_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_create_tag()
            || self.git.tag_form.is_some()
            || self.git.tag_delete.is_some()
            || self.git.tag_push.is_some()
        {
            return;
        }
        let status = self.value("git-tag-status");
        let Some(oid) = target_commit(self.git.tab, &self.git.selected, &status).map(str::to_owned)
        else {
            return;
        };
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Tag name"));
        crate::controls::focus_editor(&name, window, cx);
        let message = cx.new(|cx| TextareaState::new(window, cx).placeholder("Tag message"));
        let target = cx.new(|cx| InputState::new(window, cx).placeholder("Full object ID"));
        target.update(cx, |input, cx| input.set_value(oid, window, cx));
        let kind = cx.new(|cx| {
            SelectState::new(
                vec!["Lightweight", "Annotated"],
                Some(gpui_kit::component::IndexPath::new(0)),
                window,
                cx,
            )
        });
        let id = uuid::Uuid::new_v4();
        let kind_subscription = cx.subscribe(&kind, move |this, _, event, cx| {
            if this.git_mutation_blocks_write() {
                return;
            }
            if let SelectEvent::Confirm(Some(value)) = event
                && let Some(form) = this.git.tag_form.as_mut()
                && form.id == id
            {
                form.annotated = *value == "Annotated";
                cx.notify();
            }
        });
        let subscriptions = vec![
            Self::project_form_input(&name, project_dialog::Kind::Tag, Self::create_tag, cx),
            cx.observe(&message, |_, _, cx| cx.notify()),
            Self::project_form_input(&target, project_dialog::Kind::Tag, Self::create_tag, cx),
            kind_subscription,
        ];
        self.git.detail_open = true;
        self.git.tag_form = Some(Form {
            id,
            name,
            message,
            target,
            kind,
            annotated: false,
            snapshot: text(&status, "snapshot"),
            _subscriptions: subscriptions,
        });
        cx.notify();
    }
    fn create_tag(&mut self, cx: &mut Context<Self>) {
        if !self.can_create_tag() {
            return;
        }
        let Some(form) = &self.git.tag_form else {
            return;
        };
        let draft = form.draft(cx);
        if !draft.valid() {
            return;
        }
        let name = draft.name.trim().to_owned();
        let message = if draft.annotated {
            draft.message.clone()
        } else {
            String::new()
        };
        let oid = draft.target.trim().to_ascii_lowercase();
        let snapshot = form.snapshot.clone();
        self.start_git_mutation(
            self.git_repository_id(),
            format!("Create tag {name}"),
            move |backend, server, repo, id| {
                backend.create_tag(server, repo, &snapshot, (&name, &oid, &message), id)
            },
            cx,
        );
        if let Some(state) = self.git_mutation.as_mut() {
            state.tag_draft = Some(draft);
        }
    }
    pub(super) fn git_tag_trigger(&self, cx: &mut Context<Self>) -> Button {
        crate::controls::button("new-tag")
            .icon(IconName::Plus)
            .label(if self.git.tab == 1 {
                "Tag this commit…"
            } else {
                "New tag"
            })
            .disabled(
                !self.can_create_tag()
                    || self.git.tag_form.is_some()
                    || self.git.tag_delete.is_some()
                    || self.git.tag_push.is_some(),
            )
            .on_click(cx.listener(|this, _, window, cx| this.new_tag(window, cx)))
    }
    pub(super) fn git_tag_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        crate::git_listing::toolbar(&self.git.page, "tag", "tags", self.git_tag_trigger(cx), cx)
    }
    pub(super) fn git_tag_delete_trigger(&self, cx: &mut Context<Self>) -> Button {
        crate::controls::button("review-delete-tag")
            .label("Delete local tag…")
            .disabled(
                !self.can_modify_tag()
                    || self.git.tag_delete.is_some()
                    || self.git.tag_form.is_some()
                    || self.git.tag_push.is_some(),
            )
            .on_click(cx.listener(|this, _, _, cx| this.review_delete_tag(cx)))
    }
    pub(super) fn git_tag_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let body = div().flex().flex_col().gap_3().w_full()
            .when_some(self.git.tag_delete.as_ref(),|view,review|view
                .child(project_dialog::description("tag-delete-description", format!("Delete local tag {} at {}? This cannot be undone in Newport. Remote tags are unchanged.",review.name,review.oid.chars().take(8).collect::<String>())))
                .child(project_dialog::actions()
                    .child(crate::controls::button("cancel-delete-tag").label("Cancel").disabled(self.git_write_pending()).on_click(cx.listener(|this,_,_,cx|{if this.git_write_pending() { return; } this.git.tag_delete=None;cx.notify();})))
                    .child(crate::controls::button("confirm-delete-tag").loading(self.git_write_pending()).danger().label("Delete local tag").disabled(!self.can_modify_tag()).on_click(cx.listener(|this,_,_,cx|this.delete_tag(cx))))))
            .when_some(self.git.tag_form.as_ref(), |view, form| {
                let busy = self.git_mutation_blocks_write();
                view.child(project_dialog::description("tag-create-description", "Creates a local tag. Push it separately to share it with a remote."))
                .child(div().flex().flex_col().gap_1().child(crate::label("Tag name").text_size(px(crate::ui::FONT_SIZE)))
                    .child(crate::controls::input(&form.name).aria_label("Tag name").disabled(busy)))
                .child(div().flex().flex_col().gap_1().child(crate::label("Target object ID").text_size(px(crate::ui::FONT_SIZE)))
                    .child(crate::controls::input(&form.target).aria_label("Target object ID").disabled(busy))
                    .child(project_dialog::description("tag-target-description", "Enter a full 40-character Git object ID.")))
                .child(div().flex().flex_col().gap_1().child(crate::label("Tag type").text_size(px(crate::ui::FONT_SIZE)))
                    .child(crate::controls::select(&form.kind).title_prefix("Tag type: ").disabled(busy)))
                .when(form.annotated, |view| view.child(div().flex().flex_col().gap_1()
                    .child(crate::label("Tag message").text_size(px(crate::ui::FONT_SIZE)))
                    .child(crate::controls::textarea(&form.message).h(px(100.)).aria_label("Tag message").disabled(busy))
                    .child(project_dialog::description("tag-identity-description", "Uses the Git identity configured on the server."))))
                .child(project_dialog::actions()
                    .child(crate::controls::button("cancel-tag").label("Cancel").disabled(self.git_write_pending()).on_click(
                            cx.listener(|this, _, _, cx| {
                                if this.git_write_pending() { return; }
                                this.git.tag_form = None;
                                this.git.detail_open = !this.git.selected.is_null();
                                cx.notify();
                            }),
                        ))
                    .child(crate::controls::button("create-tag").loading(self.git_write_pending())
                                .label("Create tag")
                                .disabled(
                                    !self.can_create_tag()
                                        || !form.draft(cx).valid(),
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.create_tag(cx)))))
            });
        crate::git_listing::editor(
            if self.git.tag_delete.is_some() {
                "Delete local tag"
            } else {
                "New tag"
            },
            body,
            cx,
        )
    }
}

impl Form {
    pub(super) fn initial_focus(&self, cx: &App) -> FocusHandle {
        self.name.read(cx).focus_handle(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::{tag_identity, target_commit};
    use serde_json::{Value, json};
    #[test]
    fn tag_draft_validates_target_and_explicit_annotation() {
        let mut draft = super::Draft {
            id: uuid::Uuid::new_v4(),
            name: "v1".into(),
            target: "A".repeat(40),
            message: String::new(),
            annotated: false,
        };
        assert!(draft.valid());
        let original = draft.clone();
        draft.target = "B".repeat(40);
        assert_ne!(
            draft, original,
            "Changing target must preserve a newer draft after completion"
        );
        draft.target = "HEAD".into();
        assert!(!draft.valid());
        draft.target = " aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ".into();
        draft.annotated = true;
        assert!(!draft.valid());
        draft.message = "Release notes\n\nMultiline detail".into();
        assert!(draft.valid());
        let annotated = draft.clone();
        draft.annotated = false;
        assert_ne!(draft, annotated, "Changing tag type changes draft identity");
        draft.name = "  ".into();
        assert!(!draft.valid());
    }
    #[test]
    fn delete_tag_requires_lossless_direct_identity() {
        let tag = json!({"name":{"display":"v1","bytesB64":"djE="},"oid":{"hex":"tag-object"},"peeledOid":{"hex":"commit"}});
        assert_eq!(tag_identity(&tag), Some(("v1".into(), "tag-object".into())));
        let mut invalid = tag.clone();
        invalid["name"]["bytesB64"] = json!("/w==");
        assert!(tag_identity(&invalid).is_none());
        let mut symbolic = tag;
        symbolic["symbolicTarget"] = json!({"display":"refs/tags/other"});
        assert!(tag_identity(&symbolic).is_none());
    }
    #[test]
    fn history_target_is_selected_commit_not_head() {
        let selected = json!({"oid":{"hex":"older"}});
        let status = json!({"metadata":{"head":{"oid":{"hex":"head"}}}});
        assert_eq!(target_commit(1, &selected, &status), Some("older"));
        assert_eq!(target_commit(4, &selected, &status), Some("head"));
        assert_eq!(target_commit(1, &Value::Null, &status), None);
        assert_eq!(target_commit(4, &selected, &Value::Null), None);
        assert_eq!(target_commit(3, &selected, &status), None);
    }
}
