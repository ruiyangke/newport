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
fn folders(list: &Value, hidden: bool) -> Vec<Value> {
    let mut entries = rows(&list["entries"]);
    entries.retain(|entry| {
        entry["kind"] == "directory"
            && (hidden || !text(entry, "name").starts_with('.'))
            && chosen_path(&text(entry, "path"), None).is_some()
    });
    entries.sort_by_cached_key(|entry| text(entry, "name").to_lowercase());
    entries
}
impl Newport {
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
    fn cancel_folder_read(&mut self) {
        if let Some(operation) = self
            .project_folder
            .as_mut()
            .and_then(|chooser| chooser.operation.take())
        {
            let _ = std::thread::Builder::new()
                .name("folder-cancel".into())
                .spawn(move || {
                    let _ = backend::Backend::discover()
                        .call("files_cancel", json!({"operation":operation}));
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
        self.request(
            "project-folder",
            "files_list",
            json!({"id":self.server,"operation":operation,"path":path}),
        );
        cx.notify();
    }
    fn choose_project_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chooser) = self.project_folder.as_ref() else {
            return;
        };
        let Some(resource) = self
            .resources
            .get("project-folder")
            .filter(|r| !r.pending && r.error.is_none())
        else {
            return;
        };
        if chooser.server != self.server {
            return;
        }
        let leaf = chooser.leaf.read(cx).value();
        let Some(path) = chosen_path(
            &text(&resource.value, "path"),
            chooser.child.then_some(leaf.as_str()),
        ) else {
            return;
        };
        chooser
            .target
            .update(cx, |input, cx| input.set_value(path, window, cx));
        self.return_from_project_folder(window, cx);
    }
    pub(super) fn project_folder_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(chooser) = &self.project_folder else {
            return div().into_any_element();
        };
        let listing = self.value("project-folder");
        let directory = text(&listing, "path");
        let ready = self
            .resources
            .get("project-folder")
            .is_some_and(|r| !r.pending && r.error.is_none());
        let leaf = chooser.leaf.read(cx).value();
        let valid =
            ready && chosen_path(&directory, chooser.child.then_some(leaf.as_str())).is_some();
        let entries = folders(&listing, chooser.hidden);
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
                                let p =
                                    parent(&text(&this.value("project-folder"), "path")).to_owned();
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
                                let path = text(&entries[i], "path");
                                div().h(px(32.)).child(
                                    Button::new(("folder", i))
                                        .accessibility_label(text(&entries[i], "name"))
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
                                                        .child(text(&entries[i], "name")),
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
    use serde_json::json;
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
        let list = json!({"entries":[{"kind":"directory","name":".hidden","path":"/.hidden"},{"kind":"file","name":"file","path":"/file"},{"kind":"directory","name":"app","path":"/app"}]});
        assert_eq!(folders(&list, false).len(), 1);
        assert_eq!(folders(&list, true).len(), 2);
    }
}
