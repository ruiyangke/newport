//! Remote, read-only directory selection scoped to one form and server.
use super::*;
pub(super) struct Chooser {
    target: Entity<InputState>,
    address: Entity<InputState>,
    leaf: Entity<InputState>,
    child: bool,
    hidden: bool,
    server: String,
    operation: Option<uuid::Uuid>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}
fn parent(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() && path.starts_with('/') {
        return "/";
    }
    trimmed
        .rsplit_once('/')
        .map(|(p, _)| if p.is_empty() { "/" } else { p })
        .unwrap_or(".")
}
fn chosen_path(directory: &str, leaf: Option<&str>) -> Option<String> {
    if !directory.starts_with('/') || directory.contains('\0') {
        return None;
    }
    match leaf {
        None => Some(directory.to_owned()),
        Some(leaf)
            if !leaf.is_empty()
                && !matches!(leaf, "." | "..")
                && !leaf.contains('/')
                && !leaf.chars().any(char::is_control) =>
        {
            Some(format!("{}/{}", directory.trim_end_matches('/'), leaf))
        }
        _ => None,
    }
}
fn folders(list: Option<&desktop::files::Listing>, hidden: bool) -> Vec<desktop::files::Entry> {
    let mut entries = list.map_or_else(Vec::new, |list| list.entries.clone());
    entries.retain(|entry| {
        entry.kind == desktop::files::EntryKind::Directory
            && (hidden || !entry.name.starts_with('.'))
            && chosen_path(&entry.path, None).is_some()
    });
    entries.sort_by_cached_key(|entry| entry.name.to_lowercase());
    entries
}
impl Newport {
    /// The folder chooser's listing, while its read shows it.
    fn project_folder_listing(&self) -> Option<&desktop::files::Listing> {
        self.owned("project-folder", &self.project_folder_listing)
    }
    /// The settled listing's directory, once read without an error.
    fn project_folder_directory(&self) -> Option<&str> {
        self.resources
            .get("project-folder")
            .filter(|r| !r.pending && r.error.is_none())
            .and(self.project_folder_listing())
            .map(|listing| listing.path.as_str())
    }
    /// Settles the folder chooser's listing.
    pub(super) fn complete_project_folder(
        &mut self,
        generation: u64,
        result: Result<desktop::Reply, newport_client::request_error::RequestError>,
    ) -> bool {
        let result = result
            .and_then(|reply| match reply {
                desktop::Reply::Files(reply) => match *reply {
                    desktop::files::Reply::List(listing) => Ok(*listing),
                    _ => Err("The backend answered a folder listing with another reply.".into()),
                },
                _ => Err("The backend answered a folder listing with another reply.".into()),
            })
            .map(|listing| {
                let changed = self.project_folder_listing.as_ref() != Some(&listing);
                (listing, changed)
            });
        self.complete_owned("project-folder", generation, result, |this| {
            &mut this.project_folder_listing
        })
    }
    pub(super) fn begin_project_folder(
        &mut self,
        target: Entity<InputState>,
        child: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_project_folder();
        let current = target.read(cx).value().to_string();
        let initial = if current.starts_with('/') {
            if child {
                parent(&current).to_owned()
            } else {
                current.clone()
            }
        } else {
            ".".into()
        };
        let address = cx.new(|cx| InputState::new(window, cx).default_value(initial.clone()));
        crate::controls::focus_editor(&address, window, cx);
        let leaf = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(if child {
                    current.rsplit('/').next().unwrap_or("")
                } else {
                    ""
                })
                .placeholder("New folder name")
        });
        let mut subscriptions: Vec<_> = [&address, &leaf]
            .into_iter()
            .map(|input| cx.observe(input, |_, _, cx| cx.notify()))
            .collect();
        subscriptions.extend([
            cx.subscribe_in(&address, window, |this, input, event, window, cx| {
                if matches!(
                    event,
                    gpui_kit::component::input::InputEvent::PressEnter {
                        secondary: false,
                        shift: false
                    }
                ) && this
                    .project_folder
                    .as_ref()
                    .is_some_and(|chooser| chooser.address == *input)
                {
                    let path = input.read(cx).value().to_string();
                    this.navigate_project_folder(path, window, cx);
                }
            }),
            cx.subscribe_in(&leaf, window, |this, input, event, window, cx| {
                if matches!(
                    event,
                    gpui_kit::component::input::InputEvent::PressEnter {
                        secondary: false,
                        shift: false
                    }
                ) && this
                    .project_folder
                    .as_ref()
                    .is_some_and(|chooser| chooser.leaf == *input)
                {
                    this.choose_project_folder(window, cx);
                }
            }),
        ]);
        self.project_folder = Some(Chooser {
            target,
            address,
            leaf,
            child,
            hidden: false,
            server: self.server.clone(),
            operation: None,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        });
        self.navigate_project_folder(initial, window, cx);
    }
    /// The chooser opens on the account's home as "."; once listed, the
    /// address shows the real path.
    pub(super) fn sync_project_folder_address(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chooser) = &self.project_folder else {
            return;
        };
        if chooser.address.read(cx).value() != "." {
            return;
        }
        let Some(path) = self
            .project_folder_directory()
            .filter(|path| path.starts_with('/'))
            .map(str::to_owned)
        else {
            return;
        };
        chooser
            .address
            .update(cx, |input, cx| input.set_value(path, window, cx));
    }
    fn cancel_folder_read(&mut self) {
        if let Some(operation) = self
            .project_folder
            .as_mut()
            .and_then(|chooser| chooser.operation.take())
        {
            let _ = std::thread::Builder::new()
                .name("folder-cancel".into())
                .spawn(move || {
                    let _ = backend::Backend::discover().execute(desktop::Operation::Files(
                        Box::new(desktop::files::Operation::Cancel(Box::new(
                            desktop::files::CancelArgs { operation },
                        ))),
                    ));
                });
        }
        self.resources.invalidate("project-folder");
    }
    pub(super) fn close_project_folder(&mut self) {
        self.cancel_folder_read();
        self.project_folder = None;
    }
    pub(super) fn return_from_project_folder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self
            .project_folder
            .as_ref()
            .map(|chooser| chooser.target.clone());
        self.close_project_folder();
        if let Some(target) = target {
            crate::controls::focus_editor(&target, window, cx);
        }
        cx.notify();
    }
    fn navigate_project_folder(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if path.is_empty() || path.contains('\0') {
            return;
        }
        self.cancel_folder_read();
        let Some(chooser) = self.project_folder.as_mut() else {
            return;
        };
        if chooser.server != self.server {
            return;
        }
        chooser
            .address
            .update(cx, |input, cx| input.set_value(path.clone(), window, cx));
        chooser.scroll = UniformListScrollHandle::new();
        let operation = uuid::Uuid::new_v4();
        chooser.operation = Some(operation);
        self.request_typed(
            "project-folder",
            desktop_server(&self.server).map(|server| {
                desktop::Operation::Files(Box::new(desktop::files::Operation::List(Box::new(
                    desktop::files::ListArgs {
                        server,
                        operation,
                        path,
                        expected: None,
                        refresh: false,
                        project_root: None,
                    },
                ))))
            }),
        );
        cx.notify();
    }
    fn choose_project_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chooser) = self.project_folder.as_ref() else {
            return;
        };
        let Some(directory) = self.project_folder_directory() else {
            return;
        };
        if chooser.server != self.server {
            return;
        }
        let leaf = chooser.leaf.read(cx).value();
        let Some(path) = chosen_path(directory, chooser.child.then_some(leaf.as_str())) else {
            return;
        };
        chooser
            .target
            .update(cx, |input, cx| input.set_value(path, window, cx));
        self.name_added_project(window, cx);
        self.return_from_project_folder(window, cx);
    }
    pub(super) fn project_folder_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(chooser) = &self.project_folder else {
            return div().into_any_element();
        };
        let listing = self.project_folder_listing();
        let directory = listing.map_or_else(String::new, |listing| listing.path.clone());
        let ready = self.project_folder_directory().is_some();
        let leaf = chooser.leaf.read(cx).value();
        let valid =
            ready && chosen_path(&directory, chooser.child.then_some(leaf.as_str())).is_some();
        let entries = folders(listing, chooser.hidden);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .p_5()
            .gap_3()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if chooser.child {
                        "Choose destination folder"
                    } else {
                        "Choose repository folder"
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        crate::controls::button("folder-parent")
                            .label("Up")
                            .disabled(!ready || directory == "/")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let p = parent(
                                    this.project_folder_listing()
                                        .map_or("", |listing| listing.path.as_str()),
                                )
                                .to_owned();
                                this.navigate_project_folder(p, window, cx);
                            })),
                    )
                    .child(div().flex_1().child(
                        crate::controls::input(&chooser.address).aria_label("Server folder path"),
                    ))
                    .child(
                        crate::controls::button("folder-go")
                            .label("Go")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(c) = &this.project_folder {
                                    let p = c.address.read(cx).value().to_string();
                                    this.navigate_project_folder(p, window, cx);
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(label(directory)))
                    .child(
                        crate::controls::button("folder-hidden")
                            .label(if chooser.hidden {
                                "Hide hidden folders"
                            } else {
                                "Show hidden folders"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(c) = this.project_folder.as_mut() {
                                    c.hidden = !c.hidden;
                                    c.scroll = UniformListScrollHandle::new();
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(self.status("project-folder", cx))
            .when(ready && entries.is_empty(), |v| {
                v.child(empty::compact("No subfolders in this directory.", cx))
            })
            .child(
                uniform_list(
                    "project-folders",
                    entries.len(),
                    cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|i| {
                                let path = entries[i].path.clone();
                                div().h(px(32.)).child(
                                    Button::new(("folder", i))
                                        .accessibility_label(entries[i].name.clone())
                                        .child(
                                            div()
                                                .w_full()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(Icon::new(IconName::Folder))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .child(entries[i].name.clone()),
                                                ),
                                        )
                                        .w_full()
                                        .ghost()
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.navigate_project_folder(path.clone(), window, cx)
                                        })),
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&chooser.scroll)
                .flex_1()
                .min_h(px(100.)),
            )
            .when(chooser.child, |v| {
                v.child(label("New folder name (must not already exist)"))
                    .child(crate::controls::input(&chooser.leaf).aria_label("New folder name"))
            })
            .child(
                project_dialog::actions()
                    .child(
                        crate::controls::button("folder-cancel")
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.return_from_project_folder(window, cx);
                            })),
                    )
                    .child(
                        crate::controls::button("folder-choose")
                            .label(if chooser.child {
                                "Use destination"
                            } else {
                                "Use this folder"
                            })
                            .disabled(!valid)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose_project_folder(window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::{chosen_path, folders, parent};
    use newport_protocol::desktop::files::{Entry, EntryKind, Listing};
    #[test]
    fn folder_selection_preserves_unicode_and_rejects_traversal_or_invalid_paths() {
        assert_eq!(
            chosen_path("/home/é", Some("app space")),
            Some("/home/é/app space".into())
        );
        assert_eq!(chosen_path("/", Some("app")), Some("/app".into()));
        for leaf in ["", ".", "..", "a/b", "bad\0"] {
            assert!(chosen_path("/home", Some(leaf)).is_none());
        }
        assert!(chosen_path("relative", None).is_none());
        assert_eq!(parent("/home/é/"), "/home");
        assert_eq!(parent("/"), "/");
        let entry = |kind, name: &str| Entry {
            name: name.into(),
            path: format!("/{name}"),
            kind,
            size: None,
            modified: None,
            permissions: None,
        };
        let list = Listing {
            path: "/".into(),
            entries: vec![
                entry(EntryKind::Directory, ".hidden"),
                entry(EntryKind::File, "file"),
                entry(EntryKind::Directory, "app"),
            ],
        };
        assert_eq!(folders(Some(&list), false).len(), 1);
        assert_eq!(folders(Some(&list), true).len(), 2);
        assert!(folders(None, true).is_empty());
    }
}
