//! Commit identity and message presentation shared by historical inspections.
use super::*;
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
pub(super) struct State {
    expanded: bool,
    overflow: Rc<Cell<bool>>,
    copied: Option<(String, Instant)>,
}
impl State {
    fn copied(&self, oid: &str, now: Instant) -> bool {
        self.copied.as_ref().is_some_and(|(id, at)| {
            id == oid && now.saturating_duration_since(*at) < Duration::from_millis(1500)
        })
    }
}

fn split_message(message: &str) -> (&str, &str) {
    let (subject, body) = message.split_once('\n').unwrap_or((message, ""));
    (
        subject.trim_end_matches('\r'),
        body.trim_start_matches(['\r', '\n']).trim_end(),
    )
}

pub(super) fn history_subject(entry: &Value) -> String {
    let message = entry["message"]["display"].as_str().unwrap_or_default();
    let (subject, _) = split_message(message);
    if subject.is_empty() {
        "Empty commit message".into()
    } else {
        subject.into()
    }
}

pub(super) fn relative_time(value: &Value, now: i64) -> String {
    let Some(seconds) = value.as_i64() else {
        return "Date unavailable".into();
    };
    let Some(millis) = seconds.checked_mul(1000) else {
        return "Date unavailable".into();
    };
    if chrono::DateTime::from_timestamp_millis(millis).is_none() {
        return "Date unavailable".into();
    }
    let minutes = ((now as f64 - seconds as f64) / 60.).round();
    if minutes < 1. {
        "just now".into()
    } else if minutes < 60. {
        format!("{minutes:.0} min ago")
    } else {
        let hours = (minutes / 60.).round();
        let days = (hours / 24.).round();
        if hours < 24. {
            format!("{hours:.0} h ago")
        } else if days == 1. {
            "yesterday".into()
        } else if days < 7. {
            format!("{days:.0} days ago")
        } else {
            crate::localized_time::detail_date(millis)
        }
    }
}

pub(super) fn history_subtitle(entry: &Value) -> String {
    format!(
        "{} · {} · {}",
        text(&entry["author"], "name"),
        relative_time(&entry["time"], chrono::Utc::now().timestamp()),
        short_oid(&text(&entry["oid"], "hex"))
    )
}
pub(super) fn history_row_identity(entry: &Value, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .gap(px(5.))
        .text_size(px(crate::ui::FONT_SIZE))
        .line_height(px(17.))
        .text_color(cx.theme().muted_foreground)
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(text(&entry["author"], "name")),
        )
        .child(div().flex_shrink_0().child("·"))
        .child(div().flex_shrink_0().child(relative_time(
            &entry["time"],
            chrono::Utc::now().timestamp(),
        )))
        .child(
            div()
                .debug_selector(|| "history-row-oid".into())
                .ml_auto()
                .pl(px(6.))
                .flex_shrink_0()
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(crate::ui::FONT_SIZE))
                .child(short_oid(&text(&entry["oid"], "hex"))),
        )
}
fn short_oid(oid: &str) -> String {
    oid.chars().take(7).collect()
}

fn message_body(body: String, state: &State, background: Hsla) -> Div {
    let overflow = state.overflow.clone();
    div()
        .relative()
        .flex()
        .flex_col()
        .min_w_0()
        .overflow_hidden()
        .when(!state.expanded, |view| view.max_h(px(72.)))
        .on_children_prepainted(move |bounds, window, _| {
            let next = bounds.first().is_some_and(|b| b.size.height > px(72.5));
            if overflow.replace(next) != next {
                window.refresh();
            }
        })
        .child(
            div()
                .id("git-commit-body-text")
                .role(accesskit::Role::Paragraph)
                .aria_label(body.clone())
                .flex_shrink_0()
                .w_full()
                .text_size(px(crate::ui::FONT_SIZE))
                .line_height(px(18.))
                .child(body),
        )
        // Collapsed-message mask: the first 55% stays opaque,
        // then the remaining text fades into the inspector surface.
        .when(!state.expanded && state.overflow.get(), |view| {
            view.child(
                div()
                    .debug_selector(|| "git-commit-body-fade".into())
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(72. * 0.45))
                    .bg(gpui::linear_gradient(
                        180.,
                        gpui::linear_color_stop(background.opacity(0.), 0.),
                        gpui::linear_color_stop(background, 1.),
                    )),
            )
        })
}

impl Newport {
    pub(super) fn git_commit_header(
        &self,
        detail: &Value,
        parents: &[Value],
        parent: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let workspace = self.code_workspace().is_some();
        let message = text(&detail["message"], "display");
        let (subject, body) = split_message(&message);
        let oid = text(&detail["oid"], "hex");
        let copied = self.git.history.header.copied(&oid, Instant::now());
        let exact = detail["time"]
            .as_i64()
            .and_then(|n| n.checked_mul(1000))
            .map(crate::localized_time::full_date)
            .unwrap_or_else(|| "Date unavailable".into());
        let email = text(&detail["author"], "email");
        let parents = if parents.is_empty() {
            rows(&detail["parents"])
        } else {
            parents.to_vec()
        };
        let parent_label = parents
            .get(parent)
            .map(|p| format!("Compared with parent {}", short_oid(&text(p, "hex"))))
            .unwrap_or_else(|| "Initial commit · compared with an empty tree".into());
        let selected_parent = parents
            .get(parent)
            .map(|p| short_oid(&text(p, "hex")))
            .unwrap_or_default();
        let multiple_parents = parents.len() > 1;
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .min_w_0()
            .px_3()
            .py_2()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .id("git-commit-subject")
                                    .role(accesskit::Role::Heading)
                                    .aria_label(
                                        if subject.is_empty() {
                                            "Empty commit message"
                                        } else {
                                            subject
                                        }
                                        .to_owned(),
                                    )
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .line_height(px(20.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(
                                        if subject.is_empty() {
                                            "Empty commit message"
                                        } else {
                                            subject
                                        }
                                        .to_owned(),
                                    ),
                            )
                            .when(!body.is_empty(), |view| {
                                view.when(!workspace || self.git.history.header.expanded, |view| {
                                    view.child(
                                        div()
                                            .id("git-commit-body-scroll")
                                            .max_h(px(240.))
                                            .overflow_y_scroll()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(message_body(
                                                body.to_owned(),
                                                &self.git.history.header,
                                                cx.theme().background,
                                            )),
                                    )
                                })
                                .when(
                                    workspace
                                        || self.git.history.header.overflow.get()
                                        || self.git.history.header.expanded,
                                    |view| {
                                        view.child(
                                            crate::controls::compact_button("git-commit-expand")
                                                .link()
                                                .self_start()
                                                .text_color(cx.theme().primary)
                                                .px_0()
                                                .label(if self.git.history.header.expanded {
                                                    "Hide message"
                                                } else {
                                                    "Show message"
                                                })
                                                .accessibility_label(
                                                    if self.git.history.header.expanded {
                                                        "Collapse commit message"
                                                    } else {
                                                        "Expand commit message"
                                                    },
                                                )
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.git.history.header.expanded =
                                                        !this.git.history.header.expanded;
                                                    cx.notify();
                                                })),
                                        )
                                    },
                                )
                            })
                            .when(
                                detail["messageTruncated"] == true
                                    && (!workspace || self.git.history.header.expanded),
                                |view| {
                                    view.child(project_loading::picker_detail(
                                        "full commit message",
                                        Some("The commit message is truncated.".into()),
                                        self.resources
                                            .get("git-history-detail")
                                            .is_some_and(|read| read.pending),
                                    ))
                                },
                            ),
                    )
                    .when(self.git.tab == 3, |view| {
                        view.child(self.git_stash_selected_actions(cx))
                    })
                    .when(self.git.tab == 1, |view| {
                        view.child(self.git_history_actions(cx))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x_3()
                    .gap_y_1()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .id("git-commit-author")
                            .role(accesskit::Role::Label)
                            .aria_label(format!("{} · {}", text(&detail["author"], "name"), email))
                            .text_color(cx.theme().foreground)
                            .child(text(&detail["author"], "name"))
                            .hover_hint(email.clone()),
                    )
                    .child(
                        div()
                            .id("git-commit-date")
                            .role(accesskit::Role::Label)
                            .aria_label(exact.clone())
                            .child(relative_time(
                                &detail["time"],
                                chrono::Utc::now().timestamp(),
                            ))
                            .hover_hint(exact.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .child(short_oid(&oid)),
                            )
                            .child(
                                crate::controls::compact_icon_button("git-commit-copy")
                                    .ghost()
                                    .icon(if copied {
                                        IconName::Check
                                    } else {
                                        IconName::Copy
                                    })
                                    .accessibility_label(if copied {
                                        "Commit ID copied"
                                    } else {
                                        "Copy commit ID"
                                    })
                                    .hover_path(
                                        if copied {
                                            "Commit ID copied"
                                        } else {
                                            "Copy commit ID"
                                        },
                                        oid.clone(),
                                    )
                                    .disabled(oid.is_empty())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            oid.clone(),
                                        ));
                                        let token = (oid.clone(), Instant::now());
                                        this.git.history.header.copied = Some(token.clone());
                                        cx.notify();
                                        cx.spawn(async move |view, cx| {
                                            cx.background_executor()
                                                .timer(Duration::from_millis(1500))
                                                .await;
                                            let _ = view.update(cx, |this, cx| {
                                                if this.git.history.header.copied.as_ref()
                                                    == Some(&token)
                                                {
                                                    this.git.history.header.copied = None;
                                                    cx.notify();
                                                }
                                            });
                                        })
                                        .detach();
                                    })),
                            ),
                    )
                    .when(multiple_parents, |view| {
                        view.child(
                            crate::controls::compact_button("git-history-parent")
                                .label(format!("Parent {} · {}", parent + 1, selected_parent))
                                .accessibility_label("Compare with parent")
                                .dropdown_menu({
                                    let weak = cx.entity().downgrade();
                                    let owner = text(&detail["oid"], "hex");
                                    move |mut menu, _, _| {
                                        for (index, parent) in parents.iter().enumerate() {
                                            let weak = weak.clone();
                                            let owner = owner.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(format!(
                                                    "Parent {} · {}",
                                                    index + 1,
                                                    short_oid(&text(parent, "hex"))
                                                ))
                                                .on_click(move |_, _, cx| {
                                                    let _ = weak.update(cx, |this, cx| {
                                                        this.git_history_parent(&owner, index);
                                                        cx.notify();
                                                    });
                                                }),
                                            );
                                        }
                                        menu
                                    }
                                }),
                        )
                    })
                    .when(!multiple_parents, |view| {
                        view.child(
                            div()
                                .id("git-commit-base")
                                .role(accesskit::Role::Label)
                                .aria_label(parent_label.clone())
                                .child(parent_label),
                        )
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{State, message_body, relative_time, split_message};
    use gpui_kit::{self as gpui, Context, IntoElement, Render, Window, div, prelude::*, px};
    use serde_json::json;
    #[test]
    fn copy_feedback_expires_even_while_history_is_suspended() {
        let now = std::time::Instant::now();
        let state = State {
            copied: Some(("commit".into(), now)),
            ..Default::default()
        };
        assert!(state.copied("commit", now));
        assert!(!state.copied("another", now));
        assert!(!state.copied("commit", now + std::time::Duration::from_millis(1500)));
    }

    #[test]
    fn subject_and_body_keep_content_indentation() {
        assert_eq!(
            split_message("Subject\r\n\r\n  indented\n\nnext\n"),
            ("Subject", "  indented\n\nnext")
        );
        assert_eq!(split_message("Subject"), ("Subject", ""));
        assert_eq!(split_message(""), ("", ""));
    }
    #[test]
    fn relative_dates_match_history_rounding_and_reject_invalid_values() {
        let now = 1_800_000_000;
        for (ago, expected) in [
            (0, "just now"),
            (60, "1 min ago"),
            (3590, "1 h ago"),
            (86_400, "yesterday"),
            (172_800, "2 days ago"),
        ] {
            assert_eq!(relative_time(&json!(now - ago), now), expected);
        }
        assert_eq!(relative_time(&json!(null), now), "Date unavailable");
        assert_eq!(relative_time(&json!(i64::MAX), now), "Date unavailable");
        assert_ne!(
            relative_time(&json!(now - 10 * 86_400), now),
            "Date unavailable"
        );
    }

    struct Body {
        state: State,
        text: String,
        width: f32,
    }
    impl Render for Body {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(self.width)).child(
                message_body(self.text.clone(), &self.state, gpui::rgb(0xffffff).into())
                    .debug_selector(|| "message-body".into()),
            )
        }
    }
    #[gpui::test]
    fn body_disclosure_uses_wrapped_height_and_remeasures_width(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| Body {
            state: State::default(),
            text: "Short body.".into(),
            width: 200.,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(!view.read_with(cx, |v, _| v.state.overflow.get()));
        assert!(cx.debug_bounds("git-commit-body-fade").is_none());
        view.update(cx, |v, cx| {
            v.text =
                "A single source line with words that wrap across the narrow inspector. ".repeat(8);
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(view.read_with(cx, |v, _| v.state.overflow.get()));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let fade = cx.debug_bounds("git-commit-body-fade").unwrap();
        assert!((fade.size.height - px(72. * 0.45)).abs() <= px(0.5));
        assert_eq!(
            fade.bottom(),
            cx.debug_bounds("message-body").unwrap().bottom()
        );
        assert_eq!(
            cx.debug_bounds("message-body").unwrap().size.height,
            px(72.)
        );
        view.update(cx, |v, cx| {
            v.state.expanded = true;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("message-body").unwrap().size.height > px(72.));
        assert!(cx.debug_bounds("git-commit-body-fade").is_none());
        view.update(cx, |v, cx| {
            v.state.expanded = false;
            v.width = 4000.;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(!view.read_with(cx, |v, _| v.state.overflow.get()));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("git-commit-body-fade").is_none());
    }
}
