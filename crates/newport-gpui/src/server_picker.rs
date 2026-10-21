use super::*;
use gpui_kit::component::{
    combobox::{Combobox, ComboboxEvent, ComboboxState},
    searchable_list::{SearchableListItem, SearchableVec},
};

/// Where a server's connection stands, as far as this window knows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Reach {
    Connected,
    Failed,
    Unknown,
}

#[derive(Clone, PartialEq)]
pub(super) struct ServerItem {
    id: String,
    name: String,
    endpoint: String,
    search_text: String,
    reach: Reach,
}

/// Search earns its place only once the list is long enough to hunt in.
pub(super) const SEARCH_ABOVE: usize = 6;

/// A small dot saying whether the server is reachable: filled green when
/// connected, red when the last attempt failed, a hollow ring when unknown.
pub(super) fn reach_dot(reach: Reach, cx: &App) -> Div {
    let dark = cx.theme().is_dark();
    let dot = div().size(px(7.)).flex_shrink_0().rounded_full();
    match reach {
        Reach::Connected => dot.bg(theme::success(dark)),
        Reach::Failed => dot.bg(theme::error(dark)),
        Reach::Unknown => dot.border_1().border_color(cx.theme().muted_foreground),
    }
}
impl SearchableListItem for ServerItem {
    type Value = String;
    fn title(&self) -> SharedString {
        format!("{} · {}", self.name, self.endpoint).into()
    }
    fn value(&self) -> &String {
        &self.id
    }
    fn matches(&self, query: &str) -> bool {
        query
            .to_lowercase()
            .split_whitespace()
            .all(|word| self.search_text.contains(word))
    }
    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        // One line per server: whether it is reachable, its name, and where
        // it is -- the endpoint quieter and giving way first when narrow.
        div()
            .flex()
            .items_center()
            .gap(px(9.))
            .min_w_0()
            .py(px(2.))
            .child(reach_dot(self.reach, cx))
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(px(150.))
                    .text_size(px(crate::ui::FONT_SIZE))
                    .font_weight(FontWeight::MEDIUM)
                    .truncate()
                    .child(self.name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .text_color(cx.theme().muted_foreground)
                    .truncate()
                    .child(self.endpoint.clone()),
            )
    }
}
pub(super) type Picker = ComboboxState<SearchableVec<ServerItem>>;

/// Match src/types.ts displayName: only an empty name falls back; whitespace
/// is an explicit name, and the fallback includes the SSH user.
pub(super) fn display_name(server: &Value) -> String {
    let name = text(server, "name");
    if name.is_empty() {
        format!("{}@{}", text(server, "sshUser"), text(server, "sshHost"))
    } else {
        name
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn compare_labels(a: &str, b: &str, server_name: bool) -> std::cmp::Ordering {
    use cocoa::{
        base::nil,
        foundation::{NSRange, NSString},
    };
    use objc::{class, msg_send, rc::autoreleasepool, runtime::Object, sel, sel_impl};
    // Mirror localeCompare's numeric/base name comparison, then its ordinary
    // locale-aware endpoint/ID tie-breaks. NSString ranges are UTF-16 lengths.
    // The pool owns both temporary strings and the current-locale object.
    autoreleasepool(|| unsafe {
        let left = NSString::alloc(nil).init_str(a);
        let right = NSString::alloc(nil).init_str(b);
        let _: *mut Object = msg_send![left, autorelease];
        let _: *mut Object = msg_send![right, autorelease];
        let locale: *mut Object = msg_send![class!(NSLocale), currentLocale];
        // NSCaseInsensitiveSearch | NSNumericSearch | NSDiacriticInsensitiveSearch.
        let options: usize = if server_name { 1 | 64 | 128 } else { 0 };
        let result: isize = msg_send![left,
            compare:right options:options
            range:NSRange::new(0, a.encode_utf16().count() as u64) locale:locale
        ];
        result.cmp(&0)
    })
}

#[cfg(not(target_os = "macos"))]
fn compare_labels(a: &str, b: &str, server_name: bool) -> std::cmp::Ordering {
    if server_name {
        a.to_lowercase().cmp(&b.to_lowercase())
    } else {
        a.cmp(b)
    }
}

fn items(snapshot: &Value, reach: impl Fn(&str) -> Reach) -> Vec<ServerItem> {
    let mut items: Vec<_> = rows(&snapshot["config"]["servers"])
        .iter()
        .map(|server| {
            let host = text(server, "sshHost");
            let host = if host.contains(':') && !host.starts_with('[') {
                format!("[{host}]")
            } else {
                host
            };
            let port = server["sshPort"].as_u64().unwrap_or(22);
            let endpoint = format!(
                "{}@{}{}",
                text(server, "sshUser"),
                host,
                if port == 22 {
                    String::new()
                } else {
                    format!(":{port}")
                }
            );
            let name = display_name(server);
            ServerItem {
                search_text: format!(
                    "{name} {} {} {port} {endpoint}",
                    text(server, "sshHost"),
                    text(server, "sshUser")
                )
                .to_lowercase(),
                reach: reach(&text(server, "id")),
                id: text(server, "id"),
                name,
                endpoint,
            }
        })
        .collect();
    items.sort_by(|a, b| {
        compare_labels(&a.name, &b.name, true)
            .then_with(|| compare_labels(&a.endpoint, &b.endpoint, false))
            .then_with(|| compare_labels(&a.id, &b.id, false))
    });
    items
}
impl Newport {
    pub(super) fn init_picker(window: &mut Window, cx: &mut Context<Self>) -> Entity<Picker> {
        Self::new_picker(false, window, cx)
    }
    fn new_picker(searchable: bool, window: &mut Window, cx: &mut Context<Self>) -> Entity<Picker> {
        let picker = cx.new(|cx| {
            Picker::new(
                SearchableVec::new(Vec::<ServerItem>::new()),
                vec![],
                window,
                cx,
            )
            .searchable(searchable)
            .multiple(false)
        });
        cx.subscribe(&picker, |this, _, event, cx| {
            if let ComboboxEvent::Activate(values) = event
                && let Some(id) = values.first()
                && this.server_form.is_none()
                && !this.modal_open
                && this.configuration_available()
                && rows(&this.snapshot["config"]["servers"])
                    .iter()
                    .any(|server| server["id"] == *id)
                && (this.server != *id || this.page == Page::Settings)
            {
                this.save_workspace_selection();
                this.suspend_connections();
                this.suspend_project_library();
                this.reset();
                this.server = id.clone();
                this.restore_git = this.git_destinations.get(&this.server).cloned();
                if this.page == Page::Settings {
                    this.page = this.workspace_page;
                }
                this.restore_connections();
                this.save_workspace_selection();
                this.refresh_navigation();
                cx.notify();
            }
        })
        .detach();
        picker
    }
    pub(super) fn sync_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = &self.snapshot;
        let items = items(snapshot, |id| {
            match crate::connectivity::scoped(id, snapshot, cx).map(|state| state.phase) {
                Some(crate::connectivity::Phase::Connected) => Reach::Connected,
                Some(crate::connectivity::Phase::Failed) => Reach::Failed,
                _ => Reach::Unknown,
            }
        });
        if self.picker_items == items && self.picker_selection == self.server {
            return;
        }
        // The kit fixes searchability when a picker is built, so crossing the
        // threshold builds a new one; the old one is closed and dropped.
        let searchable = items.len() > SEARCH_ABOVE;
        if searchable != (self.picker_items.len() > SEARCH_ABOVE) {
            self.server_picker.update(cx, |picker, cx| picker.close(cx));
            self.server_picker = Self::new_picker(searchable, window, cx);
        }
        self.picker_items = items.clone();
        self.picker_selection = self.server.clone();
        let selection = self.server.clone();
        let picker = self.server_picker.clone();
        cx.defer_in(window, move |_, window, cx| {
            picker.update(cx, |picker, cx| {
                picker.set_items(SearchableVec::new(items), window, cx);
                picker.set_selected_values(&[selection], window, cx);
            });
        });
    }
    pub(super) fn server_selector(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        let count = self.picker_items.len();
        let blocked =
            self.server_form.is_some() || self.modal_open || !self.configuration_available();
        let current = self.selected_server().map(display_name);
        let deleting = self
            .resources
            .get(&format!("server-delete:{}", self.server))
            .is_some_and(|r| r.pending);
        // A title, not a form field: the server's name with a small chevron,
        // marked only on hover or while open. Sized to the name, within bounds.
        let width = current.as_deref().map_or(132., |name| {
            (name.chars().count() as f32 * 8. + 44.).clamp(72., 260.)
        });
        div()
            .h(px(crate::controls::CONTROL_HEIGHT))
            .flex_shrink_0()
            .w(px(width))
            .child(
                Combobox::new(&self.server_picker)
                    .placeholder("Choose a server")
                    .search_placeholder("Find a server")
                    .menu_width(px(300.))
                    .menu_max_h(px(360.))
                    .disabled(blocked)
                    .empty(move |_, cx| {
                        empty::compact(
                            if count == 0 {
                                "No saved servers yet. Add one to get started."
                            } else {
                                "No matching servers."
                            },
                            cx,
                        )
                    })
                    .appearance(false)
                    .h(px(crate::controls::CONTROL_HEIGHT))
                    .flex_shrink_0()
                    .w_full()
                    .px(px(0.))
                    .render_trigger(|trigger, _, cx| {
                        let selected = trigger.selection().first().map(|(_, item)| item);
                        div()
                            .id("server-picker-trigger")
                            .h(px(crate::controls::CONTROL_HEIGHT))
                            .w_full()
                            .min_w_0()
                            .px(px(8.))
                            .rounded(px(crate::ui::RADIUS))
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .hover(|el| el.bg(theme::row_hover(cx)))
                            .when(trigger.is_open(), |el| el.bg(theme::row_hover(cx)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(
                                        selected
                                            .map(|s| s.name.clone())
                                            .unwrap_or_else(|| "Choose a server".into()),
                                    ),
                            )
                            .child(
                                Icon::new(IconName::ChevronDown)
                                    .size(px(crate::ui::SMALL_ICON))
                                    .text_color(cx.theme().muted_foreground),
                            )
                    })
                    .footer(move |_, _| {
                        let edit_view = view.clone();
                        let add_view = view.clone();
                        // Switching is what the list is for; the footer holds
                        // the two things done from here: add another, or edit
                        // the one in use. Deleting lives in the edit dialog,
                        // next to everything else about that server.
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                crate::controls::button("picker-add-server")
                                    .ghost()
                                    .w_full()
                                    .disabled(blocked)
                                    .accessibility_label("Add server…")
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.))
                                            .child(
                                                Icon::new(IconName::Plus)
                                                    .size(px(crate::controls::ICON_SIZE)),
                                            )
                                            .child("Add server…"),
                                    )
                                    .on_click(move |_, window, cx| {
                                        let _ = add_view.update(cx, |this, cx| {
                                            this.server_picker
                                                .update(cx, |picker, cx| picker.close(cx));
                                            this.edit_server(None, window, cx)
                                        });
                                    }),
                            )
                            .when_some(current.clone(), |footer, name| {
                                footer.child(
                                    crate::controls::button("picker-edit-server")
                                        .ghost()
                                        .w_full()
                                        .disabled(blocked || deleting)
                                        .accessibility_label("Edit server")
                                        .child(
                                            div()
                                                .w_full()
                                                .min_w_0()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.))
                                                .child(
                                                    Icon::new(IconName::Settings2)
                                                        .size(px(crate::controls::ICON_SIZE)),
                                                )
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .truncate()
                                                        .child(format!("Edit {name}…")),
                                                ),
                                        )
                                        .on_click(move |_, window, cx| {
                                            let _ = edit_view.update(cx, |this, cx| {
                                                this.server_picker
                                                    .update(cx, |picker, cx| picker.close(cx));
                                                if let Some(server) =
                                                    this.selected_server().cloned()
                                                {
                                                    this.edit_server(Some(server), window, cx);
                                                }
                                            });
                                        }),
                                )
                            })
                    }),
            )
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::{Reach, display_name, items};
    fn items_unreached(snapshot: &serde_json::Value) -> Vec<super::ServerItem> {
        items(snapshot, |_| Reach::Unknown)
    }
    use gpui_kit::component::searchable_list::SearchableListItem;
    use serde_json::json;
    #[test]
    fn unnamed_servers_use_the_reference_user_and_host_label() {
        let server = json!({"id":"a", "name":"", "sshUser":"dev", "sshHost":"example.org"});
        assert_eq!(display_name(&server), "dev@example.org");
        let list = items_unreached(&json!({"config":{"servers":[server]}}));
        assert_eq!(list[0].name, "dev@example.org");
        assert!(list[0].matches("dev@example.org"));
        assert_eq!(
            display_name(&json!({"name":"  ", "sshUser":"dev", "sshHost":"example.org"})),
            "  "
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn server_names_use_numeric_case_and_accent_insensitive_ordering() {
        use super::compare_labels;
        use std::cmp::Ordering;
        assert_eq!(compare_labels("node2", "node10", true), Ordering::Less);
        assert_eq!(compare_labels("NODE2", "node2", true), Ordering::Equal);
        assert_eq!(compare_labels("nóde2", "node2", true), Ordering::Equal);
        let list = items_unreached(&json!({"config":{"servers":[
            {"id":"10", "name":"node10", "sshUser":"dev", "sshHost":"host"},
            {"id":"2", "name":"node2", "sshUser":"dev", "sshHost":"host"}
        ]}}));
        assert_eq!(
            list.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            ["2", "10"]
        );
    }
    #[test]
    fn server_search_preserves_identity_and_formats_endpoints() {
        let list = items_unreached(&json!({"config":{"servers":[
            {"id":"b","name":"Same","sshHost":"::1","sshUser":"dev","sshPort":2222},
            {"id":"a","name":"Same","sshHost":"example.org","sshUser":"other","sshPort":22}
        ]}}));
        assert_eq!(list.len(), 2);
        let ipv6 = list.iter().find(|item| item.id == "b").unwrap();
        assert_eq!(ipv6.endpoint, "dev@[::1]:2222");
        assert!(ipv6.matches("DEV@"));
        assert!(list.iter().any(|item| item.matches("example.org")));
        assert_ne!(list[0].value(), list[1].value());
        let standard = list.iter().find(|item| item.id == "a").unwrap();
        assert!(standard.matches("  SAME  OTHER 22 "));
        assert!(standard.matches("EXAMPLE other"));
        assert!(!standard.matches("same missing"));
        assert!(standard.matches("  "));
    }
}
