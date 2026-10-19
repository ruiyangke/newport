//! First run: no saved servers yet. Offers the hosts in `~/.ssh/config` for
//! import, or the server form, and says what Newport does.
use crate::*;
use std::collections::{BTreeSet, VecDeque};

const HOSTS: &str = "ssh-config-hosts";

#[derive(Default)]
pub(super) struct Welcome {
    /// Ticked aliases; seeded with the suggested hosts once the config is read.
    chosen: Option<BTreeSet<String>>,
    /// Imports run one at a time so a long config can't flood the work queue.
    queue: VecDeque<Value>,
    current: Option<String>,
    imported: usize,
    failed: Vec<String>,
}

impl Newport {
    pub(super) fn welcome_visible(&self) -> bool {
        self.page != Page::Settings
            && self.server.is_empty()
            && !self.snapshot.is_null()
            && self.snapshot["config"]["servers"]
                .as_array()
                .is_none_or(|servers| servers.is_empty())
    }

    pub(super) fn poll_welcome(&mut self) -> bool {
        // A refresh can select the first imported server mid-import, which
        // hides this screen; the remaining imports still finish.
        let visible = self.welcome_visible();
        if !visible && !self.importing() {
            return false;
        }
        let mut changed = false;
        if visible && self.resources.get(HOSTS).is_none() {
            self.request(HOSTS, "ssh_config_hosts", json!({}));
            changed = true;
        }
        if self.welcome.chosen.is_none()
            && let Some(resource) = self.resources.get(HOSTS).filter(|r| !r.pending)
        {
            self.welcome.chosen = Some(
                config_hosts(&resource.value)
                    .filter(|host| host["suggested"] == true)
                    .filter_map(|host| host["alias"].as_str().map(str::to_owned))
                    .collect(),
            );
            changed = true;
        }
        if let Some(key) = self.welcome.current.clone() {
            let resource = self.resources.get(&key);
            if resource.is_some_and(|r| r.pending) {
                return changed;
            }
            let alias = key.trim_start_matches("server-import:").to_owned();
            // A dropped reading must not stall the queue; the snapshot shows
            // whether that server was saved.
            match resource.map(|r| r.error.clone()) {
                Some(None) => self.welcome.imported += 1,
                Some(Some(error)) => self.welcome.failed.push(format!("{alias}: {error}")),
                None => self.welcome.failed.push(format!("{alias}: interrupted")),
            }
            self.welcome.current = None;
            changed = true;
        }
        if let Some(server) = self.welcome.queue.pop_front() {
            let key = format!(
                "server-import:{}",
                server["name"].as_str().unwrap_or_default()
            );
            self.resources.invalidate(&key);
            self.request(
                &key,
                "save_server",
                json!({"server":server,"password":null}),
            );
            self.welcome.current = Some(key);
            return true;
        }
        if changed
            && self.welcome.current.is_none()
            && self.welcome.imported + self.welcome.failed.len() > 0
        {
            self.finish_import();
        }
        changed
    }

    fn finish_import(&mut self) {
        let imported = std::mem::take(&mut self.welcome.imported);
        let failed = std::mem::take(&mut self.welcome.failed);
        let servers = |n: usize| format!("{n} {}", if n == 1 { "server" } else { "servers" });
        self.notification = Some(match (imported, failed.as_slice()) {
            (_, []) => Notification::success(format!("Imported {}.", servers(imported))),
            (0, [only]) => Notification::error(format!("Couldn’t import {only}.")),
            (0, _) => Notification::error(format!(
                "Couldn’t import {}. {}.",
                servers(failed.len()),
                failed.join(" · ")
            )),
            (_, _) => Notification::warning(format!(
                "Imported {}. Couldn’t import {}.",
                servers(imported),
                failed.join(" · ")
            )),
        });
        // The first saved server is selected when the snapshot arrives, which
        // replaces this screen with that server's setup.
        self.request("snapshot", "snapshot", json!({}));
    }

    fn import_chosen(&mut self) {
        let Some(chosen) = self.welcome.chosen.clone() else {
            return;
        };
        let Some(resource) = self.resources.get(HOSTS) else {
            return;
        };
        self.welcome.queue = config_hosts(&resource.value)
            .filter(|host| host["importable"] == true)
            .filter(|host| host["alias"].as_str().is_some_and(|a| chosen.contains(a)))
            .map(|host| {
                json!({
                    "id": uuid::Uuid::new_v4(),
                    "name": host["alias"],
                    "sshHost": host["host"],
                    "sshUser": host["user"],
                    "sshPort": host["port"],
                    "identityFile": host["identityFile"],
                    "agentSource": null,
                    "agentKeyFingerprint": null,
                    "authMethod": "publicKey",
                })
            })
            .collect();
        self.welcome.imported = 0;
        self.welcome.failed.clear();
        self.poll_welcome();
    }

    fn importing(&self) -> bool {
        self.welcome.current.is_some() || !self.welcome.queue.is_empty()
    }

    pub(super) fn welcome(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let resource = self.resources.get(HOSTS);
        let hosts: Vec<Value> = resource
            .map(|r| config_hosts(&r.value).cloned().collect())
            .unwrap_or_default();
        let reading = resource.is_none_or(|r| r.pending && r.value.is_null());
        let error = resource.and_then(|r| r.error.clone());
        let path = resource
            .and_then(|r| r.value["path"].as_str())
            .unwrap_or("~/.ssh/config")
            .to_owned();
        let chosen = self.welcome.chosen.clone().unwrap_or_default();
        let count = hosts
            .iter()
            .filter(|h| h["importable"] == true)
            .filter(|h| h["alias"].as_str().is_some_and(|a| chosen.contains(a)))
            .count();
        let importing = self.importing();
        let offer_import = reading || error.is_some() || !hosts.is_empty();

        let title = crate::controls::page_title(
            "Welcome to Newport",
            Some("Your servers in one window: forwarded ports, terminals, files, services, and containers over SSH.".into()),
            cx,
        );

        let list = div()
            .id("welcome-hosts")
            .debug_selector(|| "welcome-hosts".into())
            .max_h(px(264.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .children(hosts.iter().enumerate().map(|(index, host)| {
                let alias = host["alias"].as_str().unwrap_or_default().to_owned();
                let importable = host["importable"] == true;
                let address = format!(
                    "{}@{}{}",
                    host["user"].as_str().unwrap_or_default(),
                    host["host"].as_str().unwrap_or_default(),
                    match host["port"].as_u64() {
                        Some(22) | None => String::new(),
                        Some(port) => format!(":{port}"),
                    }
                );
                let note = host["note"].as_str().map(str::to_owned);
                let checked = importable && chosen.contains(&alias);
                div()
                    .min_h(px(34.))
                    .py(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div().flex_1().min_w_0().child(
                            crate::controls::checkbox(("welcome-host", index))
                                .label(alias.clone())
                                .checked(checked)
                                .disabled(!importable || importing)
                                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                    let chosen = this.welcome.chosen.get_or_insert_default();
                                    if *checked {
                                        chosen.insert(alias.clone());
                                    } else {
                                        chosen.remove(&alias);
                                    }
                                    cx.notify();
                                })),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .min_w_0()
                            .max_w(relative(0.6))
                            .child(
                                div()
                                    .truncate()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(address),
                            )
                            .when_some(note, |v, note| {
                                v.child(
                                    div()
                                        .truncate()
                                        .text_size(px(crate::ui::FONT_SIZE))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(note),
                                )
                            }),
                    )
            }));

        let card = div()
            .id("welcome-import")
            .debug_selector(|| "welcome-import".into())
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(crate::ui::RADIUS))
            .px(px(16.))
            .py(px(14.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(crate::ui::FONT_SIZE))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Import from your SSH config"),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(crate::ui::FONT_SIZE))
                            .text_color(cx.theme().muted_foreground)
                            .child(if hosts.is_empty() {
                                path.clone()
                            } else {
                                format!(
                                    "{path} · {} {}",
                                    hosts.len(),
                                    if hosts.len() == 1 { "host" } else { "hosts" }
                                )
                            }),
                    ),
            )
            .when(reading, |v| {
                v.child(crate::empty::compact(format!("Reading {path}…"), cx))
            })
            .when_some(error, |v, error| {
                v.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1().child(crate::controls::alert(
                            "welcome-hosts-error",
                            error,
                            cx,
                        )))
                        .child(
                            crate::controls::button("welcome-hosts-retry")
                                .label("Try again")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.resources.invalidate(HOSTS);
                                    this.welcome.chosen = None;
                                    this.request(HOSTS, "ssh_config_hosts", json!({}));
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when(!hosts.is_empty(), |v| {
                v.child(list).child(
                    div().flex().justify_end().child(
                        crate::controls::button("welcome-import-button")
                            .primary()
                            .icon(IconName::Download)
                            .label(if importing {
                                "Importing…".to_owned()
                            } else if count == 1 {
                                "Import 1 server".to_owned()
                            } else {
                                format!("Import {count} servers")
                            })
                            .loading(importing)
                            .disabled(count == 0 || importing)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.import_chosen();
                                cx.notify();
                            })),
                    ),
                )
            });

        let manual = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .child(
                crate::controls::button("welcome-add-server")
                    .icon(IconName::Plus)
                    .label(if offer_import {
                        "Add server manually"
                    } else {
                        "Add server"
                    })
                    .when(!offer_import, |b| b.primary())
                    .disabled(importing)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.edit_server(None, window, cx)),
                    ),
            )
            .child(
                div()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .text_color(cx.theme().muted_foreground)
                    .child(if offer_import {
                        "Host, user, and an SSH key or password."
                    } else {
                        "No hosts in ~/.ssh/config. Enter a host, user, and an SSH key or password."
                    }),
            );

        let feature = |icon: IconName, name: &'static str, text: &'static str| {
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .child(
                    Icon::new(icon)
                        .size(px(16.))
                        .mt(px(2.))
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .line_height(relative(1.6))
                        .child(div().font_weight(FontWeight::MEDIUM).child(name))
                        .child(div().text_color(cx.theme().muted_foreground).child(text)),
                )
        };
        let features = div()
            .pt(px(16.))
            .border_t_1()
            .border_color(cx.theme().border)
            .grid()
            .grid_cols(2)
            .gap(px(16.))
            .child(feature(
                IconName::Cable,
                "Forward ports",
                "Open remote web apps on localhost.",
            ))
            .child(feature(
                IconName::Terminal,
                "Terminal and files",
                "Shells and a file browser for each server.",
            ))
            .child(feature(
                IconName::Server,
                "Services and containers",
                "systemd units and Docker, live.",
            ))
            .child(feature(
                IconName::FolderGit2,
                "Projects",
                "Edit code and commit on the server.",
            ));

        div()
            .id("welcome")
            .debug_selector(|| "welcome".into())
            .role(accesskit::Role::Main)
            .aria_label("Welcome to Newport")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .p(px(24.))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(560.))
                            .flex()
                            .flex_col()
                            .gap(px(20.))
                            .child(title)
                            .when(offer_import, |v| v.child(card))
                            .child(manual)
                            .child(features)
                            .child(
                                div()
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Servers are saved on this Mac. The first time you open one, Newport asks before installing its small helper there."),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn config_hosts(value: &Value) -> impl Iterator<Item = &Value> {
    value["hosts"].as_array().into_iter().flatten()
}
