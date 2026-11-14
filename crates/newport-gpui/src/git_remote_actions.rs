//! Destructive remote actions keep exact reviewed object/configuration identities.
use super::*;
use base64::Engine as _;
use newport_protocol::git::{Action, Remote, RemoteReference};
#[derive(Clone)]
pub(super) struct Review {
    action: Action,
    reference: String,
    snapshot: String,
    source: Option<(String, String)>,
    pub(super) stale: bool,
}
fn target(entry: Option<&RemoteReference>) -> Option<(String, String, bool)> {
    let entry = entry?;
    let tag = match entry.kind.as_str() {
        "branch" => false,
        "tag" => true,
        _ => return None,
    };
    if entry.symbolic_target.is_some() {
        return None;
    }
    let reference = entry.reference.display.as_str();
    if entry.reference.bytes_b64 != base64::engine::general_purpose::STANDARD.encode(reference) {
        return None;
    }
    let name = reference.strip_prefix(if tag { "refs/tags/" } else { "refs/heads/" })?;
    let oid = entry.oid.hex.as_str();
    if name.is_empty()
        || !matches!(oid.len(), 40 | 64)
        || !oid.bytes().all(|b| b.is_ascii_hexdigit())
        || oid.bytes().all(|b| b == b'0')
    {
        return None;
    }
    Some((name.to_owned(), oid.to_owned(), tag))
}
fn action(
    remote: &Remote,
    entry: Option<&RemoteReference>,
    source: Option<&(String, String)>,
) -> Option<Action> {
    let (name, oid, tag) = target(entry)?;
    let remote_name = Some(remote.name.as_str()).filter(|s| !s.is_empty())?;
    let token = Some(remote.token.as_str()).filter(|s| !s.is_empty())?;
    if let Some((branch, local_oid)) = source {
        if tag {
            return None;
        }
        Some(Action::PushWithLease {
            remote: remote_name.into(),
            expected_token: token.into(),
            branch: branch.clone(),
            expected_oid: local_oid.clone(),
            destination_branch: name,
            expected_remote_oid: oid,
        })
    } else if tag {
        Some(Action::TagDeleteRemote {
            remote: remote_name.into(),
            expected_token: token.into(),
            name,
            expected_oid: oid,
        })
    } else {
        Some(Action::BranchDeleteRemote {
            remote: remote_name.into(),
            expected_token: token.into(),
            branch: name,
            expected_oid: oid,
        })
    }
}
impl Review {
    fn remote_identity(&self) -> (&str, &str) {
        match &self.action {
            Action::PushWithLease {
                remote,
                expected_remote_oid,
                ..
            } => (remote, expected_remote_oid),
            Action::BranchDeleteRemote {
                remote,
                expected_oid,
                ..
            }
            | Action::TagDeleteRemote {
                remote,
                expected_oid,
                ..
            } => (remote, expected_oid),
            _ => ("", ""),
        }
    }

    fn title(&self) -> String {
        let action = match &self.action {
            Action::PushWithLease { .. } => "Replace remote branch",
            Action::BranchDeleteRemote { .. } => "Delete remote branch",
            Action::TagDeleteRemote { .. } => "Delete remote tag",
            _ => "Review remote reference",
        };
        format!("{action} {}", self.reference)
    }
}
impl Newport {
    pub(super) fn remote_ref_dialog_title(&self) -> String {
        self.git
            .remote_ref_review
            .as_ref()
            .map(Review::title)
            .unwrap_or_else(|| "Remote branches and tags".into())
    }
    fn can_review_remote_ref(&self) -> bool {
        let Some(browser) = &self.git.remote_refs else {
            return false;
        };
        self.page == Page::Projects
            && self.git.tab == 6
            && self.git.writable()
            && !self.git_mutation_blocks_write()
            && browser.error.is_none()
            && browser.debounce.is_none()
            && ["git-remote-status", "git-remote-refs-first"]
                .iter()
                .all(|key| {
                    self.resources
                        .get(key)
                        .is_some_and(|r| !r.pending && r.error.is_none())
                })
            && !self
                .resources
                .get("git-remote-refs-more")
                .is_some_and(|r| r.pending || r.error.is_some())
            && self
                .git_status("git-remote-status")
                .is_some_and(|status| !status.snapshot.is_empty())
    }
    fn remote_head(&self) -> Option<&newport_protocol::git::Head> {
        self.git_status("git-remote-status")
            .map(|status| &status.metadata.head)
    }
    fn prepare_remote_ref(&mut self, push: bool, cx: &mut Context<Self>) {
        if !self.can_review_remote_ref() {
            return;
        }
        let browser = self.git.remote_refs.as_ref().unwrap();
        let source = if push {
            let Some(source) = git_pull::push_source(self.remote_head()) else {
                return;
            };
            Some(source)
        } else {
            None
        };
        let Some(action) = action(&browser.remote, browser.selected.as_ref(), source.as_ref())
        else {
            return;
        };
        self.git.remote_ref_review = Some(Review {
            action,
            reference: browser
                .selected
                .as_ref()
                .map_or_else(|| "—".into(), |selected| selected.reference.display.clone()),
            snapshot: self.remote_status_snapshot(),
            source,
            stale: false,
        });
        cx.notify();
    }
    fn remote_review_current(&self, review: &Review) -> bool {
        !review.stale
            && review.snapshot == self.remote_status_snapshot()
            && review.source.as_ref().is_none_or(|source| {
                git_pull::push_source(self.remote_head()).as_ref() == Some(source)
            })
    }
    fn submit_remote_ref(&mut self, cx: &mut Context<Self>) {
        if !self.can_review_remote_ref() {
            return;
        }
        let Some(review) = self.git.remote_ref_review.clone() else {
            return;
        };
        if !self.remote_review_current(&review) {
            return;
        }
        let title = format!(
            "{} {}",
            if review.source.is_some() {
                "Replace"
            } else {
                "Delete"
            },
            review.reference
        );
        self.start_git_mutation(
            self.git_repository_id(),
            title,
            move |backend, server, repo, id| {
                backend.apply_reviewed_action(server, repo, &review.snapshot, review.action, id)
            },
            cx,
        );
        if let Some(state) = self.git_mutation.as_mut() {
            state.remote_refs_write = true;
        }
    }
    pub(super) fn remote_ref_actions_view(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.git.remote_ref_review.is_some() && !self.project_dialog_rendering {
            return div().into_any_element();
        }
        let Some(browser) = &self.git.remote_refs else {
            return div().into_any_element();
        };
        let can_review = self.can_review_remote_ref();
        let target = target(browser.selected.as_ref());
        let mut view = div().flex().flex_col().gap_2().px_4().py_2();
        if let Some(review) = &self.git.remote_ref_review {
            let push = review.source.is_some();
            let stale = !self.remote_review_current(review);
            view=view.child(div().id("remote-reference-title").role(accesskit::Role::Heading).aria_label(review.title()).font_weight(FontWeight::SEMIBOLD).child(review.title()))
                .child(project_dialog::description("remote-reference-consequence", if push{"This can remove commits from shared remote history. The push is refused if the remote branch changed."}else{"This removes the reference from the shared remote repository. Local branches and tags are kept. Deletion is refused if the remote reference changed."}))
                .child(project_dialog::description("remote-reference-identity", format!("Remote: {} · Expected object: {}",review.remote_identity().0,review.remote_identity().1)))
                .when_some(review.source.as_ref(),|view,(branch,oid)|view.child(project_dialog::description("remote-reference-source", format!("Replace with {branch} · {oid}"))))
                .when(stale,|view|view.child(project_dialog::description("remote-reference-stale", "The reviewed state changed. Refresh remote references and review again.")))
                .child(project_dialog::actions()
                    .child(crate::controls::button("cancel-remote-reference").label("Cancel").disabled(self.git_mutation_blocks_write()).on_click(cx.listener(|this,_,_,cx|{this.git.remote_ref_review=None;cx.notify();})))
                    .child(crate::controls::button("submit-remote-reference").loading(self.git_write_pending()).label(if push {"Replace remote branch"} else if matches!(review.action, Action::TagDeleteRemote { .. }) {"Delete remote tag"} else {"Delete remote branch"}).danger().disabled(!can_review || stale).on_click(cx.listener(|this,_,_,cx|this.submit_remote_ref(cx)))));
        } else {
            view = view.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        crate::controls::button("review-delete-remote-reference")
                            .label("Delete reference…")
                            .disabled(!can_review || target.is_none())
                            .on_click(
                                cx.listener(|this, _, _, cx| this.prepare_remote_ref(false, cx)),
                            ),
                    )
                    .child(
                        crate::controls::button("review-replace-remote-reference")
                            .label("Push with lease…")
                            .disabled(
                                !can_review
                                    || !target.is_some_and(|(_, _, tag)| !tag)
                                    || git_pull::push_source(self.remote_head()).is_none(),
                            )
                            .on_click(
                                cx.listener(|this, _, _, cx| this.prepare_remote_ref(true, cx)),
                            ),
                    ),
            );
        }
        view.into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::target;
    use newport_protocol::git::{Path, Remote, RemoteReference};
    fn action(
        remote: &Remote,
        entry: &RemoteReference,
        source: Option<&(String, String)>,
    ) -> Option<serde_json::Value> {
        super::action(remote, Some(entry), source)
            .map(|action| serde_json::to_value(action).unwrap())
    }
    fn reference(name: &str, kind: &str, oid: &str) -> RemoteReference {
        RemoteReference {
            reference: Path::new(name.as_bytes()),
            kind: kind.into(),
            oid: crate::git_fixture::object(oid),
            symbolic_target: None,
        }
    }
    #[test]
    fn remote_actions_capture_exact_objects_and_exclude_symbolic_or_lossy_refs() {
        let oid = "a".repeat(40);
        let mut entry = reference("refs/heads/topic", "branch", &oid);
        let remote = Remote {
            name: "origin".into(),
            url: None,
            push_url: None,
            token: "config".into(),
        };
        let source = ("main".into(), "b".repeat(40));
        let push = action(&remote, &entry, Some(&source)).unwrap();
        assert_eq!(push["destinationBranch"], "topic");
        assert_eq!(push["expectedRemoteOid"], oid);
        assert_eq!(push["expectedOid"], source.1);
        assert_eq!(
            action(&remote, &entry, None).unwrap()["kind"],
            "branch.delete_remote"
        );
        entry.symbolic_target = Some(Path::new(b"refs/heads/main"));
        assert!(target(Some(&entry)).is_none());
        entry.symbolic_target = None;
        entry.reference.bytes_b64 = "/w==".into();
        assert!(target(Some(&entry)).is_none());
        let mut tag = reference("refs/tags/v1", "tag", &"c".repeat(40));
        assert_eq!(
            action(&remote, &tag, None).unwrap()["kind"],
            "tag.delete_remote"
        );
        assert!(action(&remote, &tag, Some(&source)).is_none());
        tag.kind = "peeled_tag".into();
        assert!(target(Some(&tag)).is_none());
        assert!(target(None).is_none());
    }
}
