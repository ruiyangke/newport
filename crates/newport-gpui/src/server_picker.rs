use super::*;
use gpui_kit::component::{
    combobox::{Combobox, ComboboxEvent, ComboboxState},
    searchable_list::{SearchableListItem, SearchableVec},
};

#[derive(Clone, PartialEq)]
pub(super) struct ServerItem {
    id: String,
    name: String,
    endpoint: String,
    search_text: String,
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
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .min_w_0()
            .child(
                Icon::new(IconName::Server)
                    .size(px(16.))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(self.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(self.endpoint.clone()),
                    ),
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

fn items(snapshot: &Value) -> Vec<ServerItem> {
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
        let picker = cx.new(|cx| {
            Picker::new(
                SearchableVec::new(Vec::<ServerItem>::new()),
                vec![],
                window,
                cx,
            )
            .searchable(true)
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
        let items = items(&self.snapshot);
        if self.picker_items == items && self.picker_selection == self.server {
            return;
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
    pub(super) fn server_selector(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        let count = self.picker_items.len();
        let blocked =
            self.server_form.is_some() || self.modal_open || !self.configuration_available();
        div()
            .h(px(56.))
            .flex_shrink_0()
            .w_full()
            .mb(px(if compact { 8. } else { 20. }))
            .child(
                Combobox::new(&self.server_picker)
                    .accessibility_label("Choose server")
                    .placeholder("Choose a server")
                    .search_placeholder("Find a server…")
                    .menu_width(px(336.))
                    .menu_max_h(px(320.))
                    .disabled(blocked)
                    .empty(move |_, cx| {
                        empty::compact(
                            if count == 0 {
                                "No saved servers yet. Use Add server to get started."
                            } else {
                                "No matching servers. Try another search."
                            },
                            cx,
                        )
                    })
                    .appearance(false)
                    .h(px(56.))
                    .flex_shrink_0()
                    .w_full()
                    .px(px(0.))
                    .render_trigger(|trigger, _, cx| {
                        let selected = trigger.selection().first().map(|(_, item)| item);
                        div()
                            .h(px(56.))
                            .w_full()
                            .px(px(8.))
                            .rounded(px(6.))
                            .when(trigger.is_open(), |el| {
                                el.bg(theme::sidebar_selection(cx.theme().is_dark()))
                            })
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(26.))
                                    .flex_shrink_0()
                                    .flex()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::Server)
                                            .size(px(16.))
                                            .text_color(theme::sidebar_muted(cx.theme().is_dark())),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .truncate()
                                            .child(
                                                selected
                                                    .map(|s| s.name.clone())
                                                    .unwrap_or_else(|| "Choose a server".into()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(theme::sidebar_foreground(
                                                cx.theme().is_dark(),
                                            ))
                                            .truncate()
                                            .child(
                                                selected
                                                    .map(|s| s.endpoint.clone())
                                                    .unwrap_or_else(|| "Your SSH workspace".into()),
                                            ),
                                    ),
                            )
                            .child(Icon::new(IconName::ChevronsUpDown).size(px(12.)))
                    })
                    .footer(move |_, cx| {
                        let view = view.clone();
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(
                                crate::controls::button("picker-add-server")
                                    .ghost()
                                    .disabled(blocked)
                                    .label("Add server…")
                                    .icon(IconName::Plus)
                                    .on_click(move |_, window, cx| {
                                        let _ = view.update(cx, |this, cx| {
                                            this.edit_server(None, window, cx)
                                        });
                                    }),
                            )
                            .child(
                                div()
                                    .pr(px(8.))
                                    .text_size(px(11.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{count} server{}",
                                        if count == 1 { "" } else { "s" }
                                    )),
                            )
                    }),
            )
            .into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::{display_name, items};
    use gpui_kit::component::searchable_list::SearchableListItem;
    use serde_json::json;
    #[test]
    fn unnamed_servers_use_the_reference_user_and_host_label() {
        let server = json!({"id":"a", "name":"", "sshUser":"dev", "sshHost":"example.org"});
        assert_eq!(display_name(&server), "dev@example.org");
        let list = items(&json!({"config":{"servers":[server]}}));
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
        let list = items(&json!({"config":{"servers":[
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
        let list = items(&json!({"config":{"servers":[
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
