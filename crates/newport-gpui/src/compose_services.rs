//! Compose service rows. Shared core code assembles definitions and replicas;
//! this module owns their table presentation and scoped navigation/controls.
use super::*;
use gpui_kit::base::ObservedElement;
use newport_domain::compose::{ComposeProject, ServiceRow, service_rows};

#[cfg(test)]
#[path = "compose_services_tests.rs"]
mod tests;

/// Count missing definitions without constructing and sorting all replica rows.
pub(super) fn missing_count(
    group: &domain::ProjectGroup<'_>,
    project: Option<&ComposeProject>,
) -> usize {
    let Some(services) = project.and_then(|project| project.services.as_ref()) else {
        return 0;
    };
    let present: HashSet<_> = group
        .services
        .iter()
        .filter_map(|container| {
            container
                .compose
                .as_ref()
                .map(|labels| labels.service.as_str())
        })
        .collect();
    services
        .iter()
        .filter(|service| !present.contains(service.name.as_str()))
        .count()
}

const PAGE_SIZE: usize = 40;

// Retain row data and focus handles across refreshes; mount only the current page.
struct OwnedRow {
    service: String,
    container: Option<domain::Container>,
    profiles: Vec<String>,
    build: bool,
    defined: bool,
    orphaned: bool,
    running_replicas: usize,
}
impl OwnedRow {
    fn borrowed(&self) -> ServiceRow<'_> {
        ServiceRow {
            service: &self.service,
            container: self.container.as_ref(),
            profiles: &self.profiles,
            build: self.build,
            defined: self.defined,
            orphaned: self.orphaned,
            running_replicas: self.running_replicas,
        }
    }
}
pub(super) struct TableCache {
    key: Value,
    rows: Rc<Vec<OwnedRow>>,
    columns: Rc<ServiceColumns>,
    page: usize,
    focus: Vec<FocusHandle>,
}
impl Default for TableCache {
    fn default() -> Self {
        Self {
            key: Value::Null,
            rows: Rc::new(vec![]),
            columns: Rc::new(ServiceColumns([0.; 6])),
            page: 0,
            focus: vec![],
        }
    }
}

impl ContainersPanel {
    pub(super) fn compose_services_table(
        &self,
        group: &domain::ProjectGroup<'_>,
        project: Option<&ComposeProject>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let actions = project.is_some_and(|project| project.cli.is_some());
        let key = json!([
            group.name,
            self.client
                .resource("snapshot")
                .map_or(0, |r| r.data_revision),
            project,
            cx.theme().font_family.to_string()
        ]);
        let mut cache = self.service_table.borrow_mut();
        if cache.key != key {
            let rows = service_rows(group, project);
            cache.columns = Rc::new(ServiceColumns::measure(&rows, actions, window, cx));
            let keys = |row: &OwnedRow| {
                row.container
                    .as_ref()
                    .map_or_else(|| format!("missing:{}", row.service), |c| c.id.clone())
            };
            let previous: std::collections::HashMap<_, _> = cache
                .rows
                .iter()
                .map(keys)
                .zip(cache.focus.iter().cloned())
                .collect();
            let anchor = cache.rows.get(cache.page * PAGE_SIZE).map(keys);
            let same_project = cache.key[0] == key[0];
            let rows: Vec<_> = rows
                .into_iter()
                .map(|row| OwnedRow {
                    service: row.service.to_owned(),
                    container: row.container.cloned(),
                    profiles: row.profiles.to_vec(),
                    build: row.build,
                    defined: row.defined,
                    orphaned: row.orphaned,
                    running_replicas: row.running_replicas,
                })
                .collect();
            let focus: Vec<_> = rows
                .iter()
                .map(|row| {
                    previous
                        .get(&keys(row))
                        .cloned()
                        .unwrap_or_else(|| cx.focus_handle())
                })
                .collect();
            cache.page = if same_project {
                anchor
                    .as_ref()
                    .and_then(|key| rows.iter().position(|row| keys(row) == *key))
                    .map_or(cache.page, |index| index / PAGE_SIZE)
                    .min(rows.len().saturating_sub(1) / PAGE_SIZE)
            } else {
                0
            };
            cache.rows = Rc::new(rows);
            cache.focus = focus;
            cache.key = key;
        }
        let rows = cache.rows.clone();
        let columns = cache.columns.clone();
        let page = cache.page;
        let focus = cache.focus.clone();
        drop(cache);
        let header = div()
            .id("compose-services-head")
            .role(accesskit::Role::Row)
            .aria_row_index(1)
            .flex()
            .items_center()
            .pb(px(4.))
            .text_size(px(crate::ui::FONT_SIZE))
            .text_color(cx.theme().muted_foreground)
            .child(
                columns
                    .service("compose-head-service")
                    .role(accesskit::Role::ColumnHeader)
                    .aria_label("Service")
                    .child("Service"),
            )
            .child(
                columns
                    .state("compose-head-state")
                    .role(accesskit::Role::ColumnHeader)
                    .aria_label("State")
                    .child("State"),
            )
            .child(
                columns
                    .cpu("compose-head-cpu")
                    .role(accesskit::Role::ColumnHeader)
                    .aria_label("CPU")
                    .child("CPU"),
            )
            .child(
                columns
                    .memory("compose-head-memory")
                    .role(accesskit::Role::ColumnHeader)
                    .aria_label("Memory")
                    .child("Memory"),
            )
            .child(
                columns
                    .ports("compose-head-ports")
                    .role(accesskit::Role::ColumnHeader)
                    .aria_label("Ports")
                    .child("Ports"),
            )
            .when(actions, |row| {
                row.child(
                    columns
                        .actions("compose-head-actions")
                        .role(accesskit::Role::ColumnHeader)
                        .aria_label("Actions"),
                )
            });
        let mut table = div()
            .id("compose-services-table")
            .debug_selector(|| "compose-services-table".into())
            .role(accesskit::Role::Table)
            .aria_row_count(rows.len() + 1)
            .aria_column_count(if actions { 6 } else { 5 })
            .aria_label(format!("Services in {}", group.name))
            .w_full()
            .min_w(px(columns.total()))
            .text_size(px(crate::ui::FONT_SIZE))
            .child(header);
        let start = page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(rows.len());
        for index in start..end {
            table = table.child(
                div()
                    .tab_group()
                    .tab_index(index as isize)
                    .tab_stop(false)
                    .track_focus(&focus[index])
                    .child(self.compose_service_row(
                        group.name,
                        rows[index].borrowed(),
                        index + 2,
                        actions,
                        &columns,
                        cx,
                    )),
            );
        }
        let project = group.name.to_owned();
        // The detail pane owns vertical scrolling. Keep only horizontal overflow
        // here so wheel gestures over a service row move the entire page.
        div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .when(rows.len() > PAGE_SIZE, |view| {
                view.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .mb(px(8.))
                        .child(
                            label(format!("{}–{end} of {} services", start + 1, rows.len()))
                                .text_size(px(crate::ui::FONT_SIZE)),
                        )
                        .child(
                            crate::controls::pagination("compose-service-pages")
                                .current_page(page + 1)
                                .total_pages(rows.len().div_ceil(PAGE_SIZE))
                                .on_click(cx.listener(move |this, page: &usize, _, cx| {
                                    if this.compose_project_selected(&project) {
                                        let mut cache = this.service_table.borrow_mut();
                                        cache.page = page
                                            .saturating_sub(1)
                                            .min(cache.rows.len().saturating_sub(1) / PAGE_SIZE);
                                        cx.notify();
                                    }
                                })),
                        ),
                )
            })
            .child(
                div()
                    .id("compose-services-scroll")
                    .debug_selector(|| "compose-services-scroll".into())
                    .w_full()
                    .min_w_0()
                    .overflow_x_scroll()
                    .child(table),
            )
            .into_any_element()
    }

    fn compose_service_row(
        &self,
        project: &str,
        row: ServiceRow<'_>,
        row_index: usize,
        actions: bool,
        columns: &ServiceColumns,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = row.container.map_or_else(
            || format!("compose-service-missing-{}", row.service),
            |container| format!("compose-service-{}", container.id),
        );
        let mut view = div()
            .id(SharedString::from(key.clone()))
            .debug_selector({
                let key = key.clone();
                move || key.clone()
            })
            .role(accesskit::Role::Row)
            .aria_row_index(row_index)
            .flex()
            .items_center()
            .py(px(3.))
            .border_t_1()
            .border_color(cx.theme().border);
        let Some(container) = row.container else {
            let note = missing_note(&row);
            view = view
                .aria_label(format!("{} · {note}", row.service))
                .child(
                    columns
                        .service(format!("{key}-name"))
                        .aria_label(row.service.to_owned())
                        .text_color(cx.theme().muted_foreground)
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .child(dot(domain::Tone::Muted, cx))
                        .child(
                            table_text(format!("{key}-label"), row.service.to_owned())
                                .flex_shrink_0()
                                .whitespace_nowrap(),
                        ),
                )
                .child(
                    columns
                        .missing_state(format!("{key}-state"))
                        .aria_label(note.clone())
                        .text_color(cx.theme().muted_foreground)
                        .child(note),
                );
            if actions {
                let (project, service) = (project.to_owned(), row.service.to_owned());
                let blocked = !self.client.can_compose_run(&project);
                view = view.child(
                    columns.actions(format!("{key}-actions")).child(
                        crate::controls::compact_button(SharedString::from(format!("{key}-up")))
                            .outline()
                            .px(px(8.))
                            .content_style(
                                div().text_size(px(crate::ui::FONT_SIZE)).style().clone(),
                                gpui_kit::component::Size::Size(px(crate::controls::ICON_SIZE)),
                            )
                            .label("Up")
                            .accessibility_label(format!("Up {service}"))
                            .hover_hint(format!("Create and start {service}"))
                            .disabled(blocked)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.compose_project_selected(&project)
                                    && this.client.can_compose_run(&project)
                                {
                                    this.propose_compose(service_up(&service, false, false, false));
                                    cx.notify();
                                }
                            })),
                    ),
                );
            }
            return view.test_support().into_any_element();
        };
        let status = domain::state_of(container);
        let mut name = div()
            .flex()
            .w_full()
            .whitespace_nowrap()
            .items_center()
            .gap(px(7.))
            .child(dot(status.tone, cx))
            .child(table_text(format!("{key}-label"), row.service.to_owned()).flex_shrink_0());
        if let Some(number) = container
            .compose
            .as_ref()
            .and_then(|labels| labels.number)
            .filter(|number| *number > 1)
        {
            name = name.child(
                table_text(format!("{key}-replica"), format!("#{number}"))
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground),
            );
        }
        let tooltip = if row.orphaned {
            format!("{} · Not in the current Compose file", container.name)
        } else {
            container.name.clone()
        };
        let service_label = container
            .compose
            .as_ref()
            .and_then(|labels| labels.number)
            .filter(|number| *number > 1)
            .map_or_else(
                || row.service.to_owned(),
                |number| format!("{} #{number}", row.service),
            );
        let (navigation_project, id, container_name, selection) = (
            project.to_owned(),
            container.id.clone(),
            container.name.clone(),
            domain::container_key(container),
        );
        let [cpu, memory, published] = service_values(container);
        view = view
            .aria_label(format!("{service_label} · {tooltip}"))
            .child(
                columns
                    .service(format!("{key}-name"))
                    .aria_label(service_label)
                    .child(
                        crate::controls::compact_button(SharedString::from(format!("{key}-open")))
                            .ghost()
                            .px_0()
                            .w_full()
                            .justify_start()
                            .when(row.orphaned || container.name != row.service, |button| {
                                button.hover_hint(tooltip)
                            })
                            .accessibility_label(format!("Open container {}", container.name))
                            .child(name)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.compose_project_selected(&navigation_project)
                                    && this.client.snapshot().is_some_and(|snapshot| {
                                        snapshot.containers.iter().any(|container| {
                                            container.id == id && container.name == container_name
                                        })
                                    })
                                {
                                    this.select(selection.clone(), window, cx);
                                }
                            })),
                    ),
            )
            .child(
                columns
                    .state(format!("{key}-state"))
                    .aria_label(status.label)
                    .text_color(tone_color(status.tone, cx))
                    .child(status.label),
            )
            .child(
                columns
                    .cpu(format!("{key}-cpu"))
                    .aria_label(cpu.clone())
                    .child(table_text(format!("{key}-cpu-value"), cpu)),
            )
            .child(
                columns
                    .memory(format!("{key}-memory"))
                    .aria_label(memory.clone())
                    .child(table_text(format!("{key}-memory-value"), memory)),
            )
            .child(
                columns
                    .ports(format!("{key}-ports"))
                    .aria_label(published.clone())
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(crate::ui::FONT_SIZE))
                    .text_color(cx.theme().muted_foreground)
                    .child(published),
            );
        if actions {
            view = view.child(
                columns
                    .actions(format!("{key}-actions"))
                    .child(self.compose_service_menu(project, row.service, row.build, &key, cx)),
            );
        }
        view.test_support().into_any_element()
    }

    fn compose_service_menu(
        &self,
        project: &str,
        service: &str,
        build: bool,
        key: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (project, service) = (project.to_owned(), service.to_owned());
        let blocked = !self.client.can_compose_run(&project);
        let weak = cx.weak_entity();
        crate::controls::icon_button(SharedString::from(format!("{key}-more")))
            .ghost()
            .icon(IconName::Ellipsis)
            .accessibility_label(format!("More actions for {service}"))
            .disabled(blocked)
            .dropdown_menu(move |mut menu, _, cx| {
                let enabled = weak.upgrade().is_some_and(|panel| {
                    let panel = panel.read(cx);
                    panel.compose_project_selected(&project)
                        && panel.client.can_compose_run(&project)
                });
                for (label, action) in [
                    ("Recreate", Some(service_up(&service, false, false, true))),
                    (
                        "Pull & recreate",
                        Some(service_up(&service, false, true, true)),
                    ),
                    (
                        "Rebuild",
                        build.then(|| service_up(&service, true, false, true)),
                    ),
                ] {
                    if let Some(action) = action {
                        let (weak, project) = (weak.clone(), project.clone());
                        menu = menu.item(PopupMenuItem::new(label).disabled(!enabled).on_click(
                            move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    if this.compose_project_selected(&project)
                                        && this.client.can_compose_run(&project)
                                    {
                                        this.propose_compose(action.clone());
                                        cx.notify();
                                    }
                                });
                            },
                        ));
                    }
                }
                let (weak, project, service) = (weak.clone(), project.clone(), service.clone());
                menu.separator()
                    .item(PopupMenuItem::new("Scale…").disabled(!enabled).on_click(
                        move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                if this.compose_project_selected(&project) {
                                    this.propose_scale(
                                        project.clone(),
                                        service.clone(),
                                        window,
                                        cx,
                                    );
                                }
                            });
                        },
                    ))
            })
            .into_any_element()
    }
}

fn service_up(service: &str, build: bool, pull: bool, force: bool) -> Value {
    json!({"kind":"up","services":[service],"build":build,"pull":pull,"forceRecreate":force})
}
fn missing_note(row: &ServiceRow<'_>) -> String {
    if row.profiles.is_empty() {
        "Not created".to_owned()
    } else {
        format!("Not created · profile {}", row.profiles.join(", "))
    }
}
fn service_values(container: &Container) -> [String; 3] {
    let ports = domain::published_ports(container)
        .into_iter()
        .map(domain::port_label)
        .collect::<Vec<_>>()
        .join(", ");
    [
        container
            .cpu
            .map(domain::format_cpu)
            .unwrap_or_else(|| "—".into()),
        container
            .memory
            .map(|value| bytes(&json!(value)))
            .unwrap_or_else(|| "—".into()),
        if ports.is_empty() {
            "—".into()
        } else {
            ports
        },
    ]
}

/// Kit's composable table is flex-based and GPUI has no shared intrinsic table
/// tracks. Shape the displayed values to share natural column widths across
/// rows, including headers, badges and the missing-service colspan. Extra space
/// grows data columns proportionally; below their natural width the table
/// scrolls horizontally, keeping complete service names and ports reachable.
struct ServiceColumns([f32; 6]);
impl ServiceColumns {
    fn measure(rows: &[ServiceRow<'_>], actions: bool, window: &mut Window, cx: &App) -> Self {
        let sans = font(cx.theme().font_family.clone());
        let mono = font(cx.theme().mono_font_family.clone());
        let mut numeric = sans.clone();
        numeric.features = tabular_figures();
        let measure = |text: &str, size: f32, font: &Font| {
            f32::from(
                window
                    .text_system()
                    .shape_line(
                        text.to_owned().into(),
                        px(size),
                        &[TextRun {
                            len: text.len(),
                            font: font.clone(),
                            ..Default::default()
                        }],
                        None,
                    )
                    .width,
            )
            .ceil()
        };
        let mut widths = [
            measure("Service", crate::ui::FONT_SIZE, &sans) + 10.,
            measure("State", crate::ui::FONT_SIZE, &sans) + 10.,
            measure("CPU", crate::ui::FONT_SIZE, &sans),
            measure("Memory", crate::ui::FONT_SIZE, &sans),
            measure("Ports", crate::ui::FONT_SIZE, &sans) + 14.,
            if actions {
                (measure("Up", crate::ui::FONT_SIZE, &sans) + 18.)
                    .max(crate::controls::CONTROL_HEIGHT)
            } else {
                0.
            },
        ];
        let mut missing_width = 0_f32;
        for row in rows {
            let replica_width = row
                .container
                .and_then(|container| container.compose.as_ref())
                .and_then(|labels| labels.number)
                .filter(|number| *number > 1)
                .map_or(0., |number| {
                    7. + measure(&format!("#{number}"), crate::ui::FONT_SIZE, &sans)
                });
            widths[0] = widths[0].max(
                measure(row.service, crate::ui::FONT_SIZE, &sans) + 7. + 7. + replica_width + 10.,
            );
            if let Some(container) = row.container {
                let [cpu, memory, ports] = service_values(container);
                widths[1] = widths[1].max(
                    measure(
                        domain::state_of(container).label,
                        crate::ui::FONT_SIZE,
                        &sans,
                    ) + 10.,
                );
                widths[2] = widths[2].max(measure(&cpu, crate::ui::FONT_SIZE, &numeric));
                widths[3] = widths[3].max(measure(&memory, crate::ui::FONT_SIZE, &numeric));
                widths[4] = widths[4].max(measure(&ports, crate::ui::FONT_SIZE, &mono) + 14.);
            } else {
                missing_width = missing_width
                    .max(measure(&missing_note(row), crate::ui::FONT_SIZE, &sans) + 10.);
            }
        }
        // A missing service spans State through Ports in the reference table.
        // Let its profile text contribute to those tracks without shifting the
        // service column or the action button relative to the other rows.
        let span: f32 = widths[1..5].iter().sum();
        if missing_width > span {
            for width in &mut widths[1..5] {
                *width *= missing_width / span;
            }
        }
        Self(widths.map(f32::ceil))
    }
    fn total(&self) -> f32 {
        self.0.iter().sum()
    }
    fn column(&self, id: impl Into<SharedString>, column: usize) -> ObservedElement<Stateful<Div>> {
        cell(id)
            .flex_basis(px(self.0[column]))
            .min_w(px(self.0[column]))
            .flex_grow(self.0[column])
            .flex_shrink_0()
            .whitespace_nowrap()
    }
    fn service(&self, id: impl Into<SharedString>) -> ObservedElement<Stateful<Div>> {
        self.column(id, 0).pr(px(10.))
    }
    fn state(&self, id: impl Into<SharedString>) -> ObservedElement<Stateful<Div>> {
        self.column(id, 1).pr(px(10.))
    }
    fn cpu(&self, id: impl Into<SharedString>) -> ObservedElement<Stateful<Div>> {
        self.column(id, 2)
            .flex()
            .justify_end()
            .text_right()
            .font_features(tabular_figures())
    }
    fn memory(&self, id: impl Into<SharedString>) -> ObservedElement<Stateful<Div>> {
        self.column(id, 3)
            .flex()
            .justify_end()
            .text_right()
            .font_features(tabular_figures())
    }
    fn ports(&self, id: impl Into<SharedString>) -> ObservedElement<Stateful<Div>> {
        self.column(id, 4).pl(px(14.))
    }
    fn actions(&self, id: impl Into<SharedString>) -> ObservedElement<Stateful<Div>> {
        self.column(id, 5).flex_grow(0.).flex().justify_end()
    }
    fn missing_state(&self, id: impl Into<SharedString>) -> ObservedElement<Stateful<Div>> {
        let width = self.0[1..5].iter().sum::<f32>();
        cell(id)
            .flex_basis(px(width))
            .min_w(px(width))
            .flex_grow(width)
            .flex_shrink_0()
            .pr(px(10.))
            .whitespace_nowrap()
    }
}
fn tabular_figures() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}
fn cell(id: impl Into<SharedString>) -> ObservedElement<Stateful<Div>> {
    div()
        .id(id.into())
        .role(accesskit::Role::Cell)
        .min_w_0()
        .test_support()
}
fn table_text(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
) -> ObservedElement<Stateful<Div>> {
    div().id(id.into()).child(text.into()).test_support()
}
