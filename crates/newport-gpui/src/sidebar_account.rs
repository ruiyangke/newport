//! The account row below Settings: sign in, the wait for the browser, and the
//! signed-in menu. The backend owns the sign-in; this row only asks for it.
use super::*;
use gpui_kit::{Anchor, component::spinner::Spinner};
use newport_client::account::{Action as AccountAction, License, NoticeAction, NoticeKind, Phase};

// Menus fit their content; a long name or email truncates at this width.
const MENU_MAX_WIDTH: f32 = 320.;
/// Hovering the row reveals its menu chevron.
const ROW_GROUP: &str = "sidebar-account-row";
/// A trial this close to its end shows its days left in the warning colour.
const TRIAL_ENDING_DAYS: i64 = 3;

impl Newport {
    pub(super) fn apply_account_effects(
        &mut self,
        effects: newport_client::account::Effects,
        cx: &mut Context<Self>,
    ) -> bool {
        // Open Remote apps follow the license: renewed with each new token,
        // and a warning with the day they close when Pro ends here. The
        // warning wins over the notice that reports the same change.
        let warning = crate::remote_apps::follow_account(self.account.status(), cx)
            .map(Notification::warning);
        let Some(notice) = effects.notice else {
            if let Some(warning) = warning {
                self.notification = Some(warning);
                return true;
            }
            return effects.changed;
        };
        self.terminal_setup_notice(&notice);
        let notification = match notice.kind {
            NoticeKind::Success => Notification::success(notice.message),
            NoticeKind::Info => Notification::info(notice.message),
            NoticeKind::Warning => Notification::warning(notice.message),
            NoticeKind::Error => Notification::error(notice.message),
        };
        self.notification = Some(match notice.action {
            None => notification,
            Some(action) => {
                // The toast's button is the next step, so it stays until used or closed.
                let (label, run) = match action {
                    NoticeAction::TryAgain => ("Try again", AccountAction::SignIn),
                    NoticeAction::ManageComputers => {
                        ("Manage computers", AccountAction::OpenPortal)
                    }
                };
                let owner = cx.entity().downgrade();
                notification.action(move |_, _, cx| {
                    let owner = owner.clone();
                    crate::controls::compact_button("account-notice-action")
                        .label(label)
                        .on_click(cx.listener(move |toast, _, window, cx| {
                            let _ = owner.update(cx, |this, cx| this.run_account(run, cx));
                            toast.dismiss(window, cx);
                        }))
                })
            }
        });
        if let Some(warning) = warning {
            self.notification = Some(warning);
        }
        true
    }

    /// Signing out gives up this computer's place on the account, so it asks
    /// first. Like other destructive confirmations, Return never confirms.
    fn confirm_sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = cx.entity().downgrade();
        let title = "Sign out of Newport on this computer?";
        let mut detail =
            "This computer gives up its place on your account. You can sign in again any time."
                .to_owned();
        if crate::remote_apps::active_sessions(cx) > 0 {
            detail.push_str(" Open remote apps will close.");
        }
        window.open_dialog(cx, move |dialog, window, _| {
            let owner = owner.clone();
            dialog
                .title(crate::controls::dialog_title(title))
                .w(px(440.))
                .max_w((window.viewport_size().width - px(32.)).max(px(240.)))
                .p(px(24.))
                .rounded(px(crate::ui::RADIUS))
                .border_0()
                .centered(true)
                .overlay_closable(false)
                .on_ok(|_, _, _| false)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .pt_3()
                        .child(crate::label(detail.clone()))
                        .child(
                            crate::project_dialog::actions()
                                .child(
                                    crate::controls::form_button("cancel-sign-out")
                                        .label("Cancel")
                                        .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    crate::controls::form_button("confirm-sign-out")
                                        .danger()
                                        .label("Sign out")
                                        .on_click(move |_, window, cx| {
                                            let _ = owner.update(cx, |this, cx| {
                                                // A signed-out computer runs nothing on the
                                                // account: end open Remote apps once the
                                                // sign-out has started.
                                                if this.account.run(AccountAction::SignOut) {
                                                    crate::remote_apps::stop_all(cx);
                                                    cx.notify();
                                                }
                                            });
                                            window.close_dialog(cx);
                                        }),
                                ),
                        ),
                )
        });
    }

    pub(super) fn run_account(&mut self, action: AccountAction, cx: &mut Context<Self>) {
        if self.account.run(action) {
            cx.notify();
        }
    }

    pub(super) fn account_row(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let height = px(if compact { 40. } else { 36. });
        let status = self.account.status();
        let busy = self.account.pending().is_some() || status.signing_out;
        let disabled = self.server_form.is_some() || self.modal_open || busy;
        let dark = cx.theme().is_dark();
        let email: SharedString = status.email.clone().unwrap_or_default().into();
        let name: Option<SharedString> = status.name.clone().map(Into::into);
        // The row and its initial lead with the person's name when there is one.
        let shown = name.clone().unwrap_or_else(|| email.clone());
        let plan = plan(status.license.as_ref(), chrono::Utc::now());
        let (leading, label, tooltip): (AnyElement, SharedString, SharedString) = match status.phase
        {
            // Hold the row's place so the sidebar doesn't shift when it loads.
            Phase::Loading => return div().h(height).flex_shrink_0().into_any_element(),
            Phase::SignedOut => (
                Icon::new(IconName::CircleUser)
                    .size(px(crate::controls::ICON_SIZE))
                    .text_color(navigation::icon_color(false, cx))
                    .into_any_element(),
                "Sign in".into(),
                "Sign in".into(),
            ),
            Phase::Waiting => (
                Spinner::new()
                    .with_size(px(crate::controls::ICON_SIZE))
                    .color(navigation::icon_color(false, cx))
                    .into_any_element(),
                "Finish in your browser".into(),
                "Finish signing in in your browser".into(),
            ),
            Phase::SignedIn => (
                avatar(&shown, cx).into_any_element(),
                shown.clone(),
                shown.clone(),
            ),
        };
        let waiting = status.phase == Phase::Waiting;
        let signed_in = status.phase == Phase::SignedIn;
        let button = crate::controls::button("account")
            .accessibility_label(tooltip.clone())
            .disabled(disabled)
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(11.))
                    .when(compact, |row| row.justify_center())
                    // The avatar is wider than an icon; keep labels aligned with Settings.
                    .child(
                        div()
                            .w(px(crate::controls::ICON_SIZE))
                            .flex()
                            .justify_center()
                            .flex_shrink_0()
                            .child(leading),
                    )
                    .when(!compact, |row| {
                        row.child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .when(waiting, |label| {
                                    label.text_color(cx.theme().muted_foreground)
                                })
                                .child(label),
                        )
                        // Free is the default, so only Trial and Pro earn a tag here.
                        .when(
                            signed_in && !matches!(plan.tier, Tier::Free | Tier::FreeSeat),
                            |row| row.child(plan.tag(dark, cx)),
                        )
                        // The row opens a menu, unlike Settings above it.
                        .when(signed_in, |row| {
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .opacity(0.)
                                    .group_hover(ROW_GROUP, |chevron| chevron.opacity(1.))
                                    .child(
                                        Icon::new(IconName::ChevronsUpDown)
                                            .size(px(crate::ui::SMALL_ICON))
                                            .text_color(cx.theme().muted_foreground),
                                    ),
                            )
                        })
                    }),
            )
            .when(compact, |button| button.tooltip(tooltip))
            .custom(navigation::sidebar_page_style(false, cx))
            .h(height)
            .min_h(height)
            .max_h(height)
            .w_full()
            .rounded_none()
            .border_0()
            .px(px(if compact { 0. } else { 17. }))
            // Leave room for the inline Cancel beside the waiting label.
            .when(waiting && !compact, |button| button.pr(px(40.)))
            .when(!compact, |button| button.justify_start());
        let weak = cx.entity().downgrade();
        let row = match status.phase {
            Phase::SignedOut => button
                .on_click(cx.listener(|this, _, _, cx| this.run_account(AccountAction::SignIn, cx)))
                .into_any_element(),
            Phase::Waiting => button
                .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                    let (again, cancel) = (weak.clone(), weak.clone());
                    menu.max_w(px(MENU_MAX_WIDTH))
                        .item(PopupMenuItem::new("Open the browser again").on_click(
                            move |_, _, cx| {
                                let _ = again.update(cx, |this, cx| {
                                    this.run_account(AccountAction::SignIn, cx)
                                });
                            },
                        ))
                        .item(
                            PopupMenuItem::new("Cancel sign-in").on_click(move |_, _, cx| {
                                let _ = cancel.update(cx, |this, cx| {
                                    this.run_account(AccountAction::CancelSignIn, cx)
                                });
                            }),
                        )
                })
                .into_any_element(),
            Phase::SignedIn => {
                let opened = weak.clone();
                button
                    .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                        let (upgrade, manage, sign_out, seat) =
                            (weak.clone(), weak.clone(), weak.clone(), weak.clone());
                        // A trial can upgrade too, without waiting for it to end.
                        let free = matches!(plan.tier, Tier::Free | Tier::Trial);
                        let free_seat = plan.tier == Tier::FreeSeat;
                        let payment_failed = plan.payment_failed;
                        let billing = weak.clone();
                        let (email, name, plan) = (email.clone(), name.clone(), plan.clone());
                        // No leading icons, so the header and the items share one edge.
                        menu.max_w(px(MENU_MAX_WIDTH))
                            .item(
                                PopupMenuItem::element(move |_, cx| {
                                    account_header(&email, name.as_ref(), &plan, cx)
                                })
                                .disabled(true),
                            )
                            .separator()
                            // Seats are moved on the account page.
                            .when(free_seat, |menu| {
                                menu.item(browser_item("Use Pro on this computer").on_click(
                                    move |_, _, cx| {
                                        let _ = seat.update(cx, |this, cx| {
                                            this.run_account(AccountAction::OpenPortal, cx)
                                        });
                                    },
                                ))
                            })
                            .when(payment_failed, |menu| {
                                menu.item(browser_item("Update payment").on_click(
                                    move |_, _, cx| {
                                        let _ = billing.update(cx, |this, cx| {
                                            this.run_account(AccountAction::OpenBilling, cx)
                                        });
                                    },
                                ))
                            })
                            .when(free, |menu| {
                                menu.item(browser_item("Upgrade to Pro").on_click(
                                    move |_, _, cx| {
                                        let _ = upgrade.update(cx, |this, cx| {
                                            this.run_account(AccountAction::OpenBilling, cx)
                                        });
                                    },
                                ))
                            })
                            .item(browser_item("Manage account").on_click(move |_, _, cx| {
                                let _ = manage.update(cx, |this, cx| {
                                    this.run_account(AccountAction::OpenPortal, cx)
                                });
                            }))
                            .item(
                                PopupMenuItem::new("Sign out").on_click(move |_, window, cx| {
                                    let _ = sign_out
                                        .update(cx, |this, cx| this.confirm_sign_out(window, cx));
                                }),
                            )
                    })
                    // The menu shows the plan, so ask the account site when it opens.
                    .on_open_change(move |open, _, cx| {
                        if *open {
                            let _ = opened.update(cx, |this, _| this.account.refresh());
                        }
                    })
                    .into_any_element()
            }
            Phase::Loading => unreachable!("handled above"),
        };
        div()
            .relative()
            .w_full()
            .group(ROW_GROUP)
            .child(row)
            .when(waiting && !compact, |wrapper| {
                wrapper.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(px(10.))
                        .flex()
                        .items_center()
                        .child(
                            crate::controls::compact_icon_button("account-cancel-sign-in")
                                .icon(IconName::Close)
                                .accessibility_label("Cancel sign-in")
                                .tooltip("Cancel sign-in")
                                .custom(navigation::sidebar_page_style(false, cx))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.run_account(AccountAction::CancelSignIn, cx)
                                })),
                        ),
                )
            })
            .into_any_element()
    }
}

/// A menu item that opens the account site. Kit link items carry the
/// external-link mark and an accessible label; the backend owns the URL, so
/// callers replace the href handler with `on_click`.
fn browser_item(label: &'static str) -> PopupMenuItem {
    PopupMenuItem::link(label, "")
}

/// The menu's first row: who is signed in, with the plan tag beside the name.
/// The second line holds the email (when a name leads) and the plan's one fact.
fn account_header(email: &SharedString, name: Option<&SharedString>, plan: &Plan, cx: &App) -> Div {
    let has_detail = !plan.detail.is_empty();
    let detail_color = if plan.urgent {
        theme::warning(cx.theme().is_dark())
    } else {
        cx.theme().muted_foreground
    };
    div()
        .min_w_0()
        .py(px(4.))
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(cx.theme().foreground)
                        .child(name.unwrap_or(email).clone()),
                )
                .child(plan.tag(cx.theme().is_dark(), cx)),
        )
        .when(name.is_some() || has_detail, |header| {
            header.child(
                div()
                    .min_w_0()
                    .flex()
                    .gap(px(4.))
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .when(name.is_some(), |line| {
                        line.child(div().min_w_0().truncate().child(email.clone()))
                    })
                    .when(name.is_some() && has_detail, |line| line.child("·"))
                    .when(has_detail, |line| {
                        line.child(
                            div()
                                .flex_shrink_0()
                                .text_color(detail_color)
                                .child(plan.detail.clone()),
                        )
                    }),
            )
        })
}

/// One initial on a neutral circle, as on the account site.
fn avatar(email: &str, cx: &App) -> Div {
    let initial = email
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_default();
    div()
        .size(px(20.))
        .flex_shrink_0()
        .rounded_full()
        .bg(theme::avatar_fill(cx.theme().is_dark()))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().foreground)
        .child(initial)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tier {
    Free,
    /// A Pro or trial account whose seat for this computer is free.
    FreeSeat,
    Trial,
    Pro,
}

/// What the account's plan shows: a tag beside the email and, in the menu, one fact.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Plan {
    tier: Tier,
    detail: SharedString,
    /// The detail needs attention soon: a trial in its last days, or a
    /// failed payment.
    urgent: bool,
    /// The last payment failed; the menu offers to fix it.
    payment_failed: bool,
}

impl Plan {
    fn tag(&self, dark: bool, cx: &App) -> Div {
        let (label, fill, ink) = match self.tier {
            Tier::Free => ("Free", cx.theme().muted, cx.theme().muted_foreground),
            Tier::FreeSeat => ("Free seat", cx.theme().muted, cx.theme().muted_foreground),
            Tier::Trial => ("Trial", theme::trial_fill(dark), theme::warning(dark)),
            Tier::Pro => ("Pro", theme::pro_fill(dark), theme::pro_ink(dark)),
        };
        crate::controls::status_tag(label, fill, ink)
    }
}

fn plan(license: Option<&License>, now: chrono::DateTime<chrono::Utc>) -> Plan {
    let parse = |value: &Option<String>| {
        value
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
    };
    let date = |value: &Option<String>| {
        parse(value).map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%-d %b %Y")
                .to_string()
        })
    };
    let trial_days = license
        .filter(|license| license.state == "trial")
        .and_then(|license| parse(&license.ends_at))
        .map(|ends| (ends.with_timezone(&chrono::Utc) - now).num_hours())
        .map(|hours| ((hours + 23) / 24).max(1));
    let free_seat = license.is_some_and(|license| license.seat.as_deref() == Some("free"));
    let (tier, detail) = match license.map(|license| (license.state.as_str(), license)) {
        Some(("active" | "grace", _)) if free_seat => {
            (Tier::FreeSeat, "Pro is on your other computers".to_owned())
        }
        // A trial's seat can't move, so this computer can only upgrade.
        Some(("trial", _)) if free_seat => {
            (Tier::Free, "Your trial is on another computer".to_owned())
        }
        Some(("active" | "grace", license)) if license.plan.as_deref() == Some("comp") => {
            (Tier::Pro, "Complimentary".to_owned())
        }
        // The last payment failed: Pro runs until the grace period ends,
        // then stops unless the payment goes through.
        Some(("grace", license)) => (
            Tier::Pro,
            date(&license.valid_until)
                .map(|stops| format!("Payment failed · Pro stops {stops}"))
                .unwrap_or_else(|| "Payment failed".to_owned()),
        ),
        Some(("active", license)) => (
            Tier::Pro,
            // A renewing plan needs no date; one that won't renew says when it stops.
            date(&license.ends_at)
                .map(|ends| format!("Ends {ends}"))
                .unwrap_or_default(),
        ),
        Some(("trial", _)) => (
            Tier::Trial,
            match trial_days {
                Some(1) => "1 day left".to_owned(),
                Some(days) => format!("{days} days left"),
                None => String::new(),
            },
        ),
        Some(("expired" | "revoked", _)) => (Tier::Free, "Newport Pro ended".to_owned()),
        _ => (Tier::Free, String::new()),
    };
    let payment_failed = license.is_some_and(|license| {
        license.state == "grace" && license.plan.as_deref() != Some("comp") && !free_seat
    });
    Plan {
        tier,
        detail: detail.into(),
        urgent: payment_failed || trial_days.is_some_and(|days| days <= TRIAL_ENDING_DAYS),
        payment_failed,
    }
}

#[cfg(test)]
mod tests {
    use super::{License, Tier, plan};

    fn license(
        state: &str,
        plan: Option<&str>,
        renews: Option<&str>,
        ends: Option<&str>,
    ) -> License {
        License {
            state: state.into(),
            plan: plan.map(Into::into),
            renews_at: renews.map(Into::into),
            ends_at: ends.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn a_failed_payment_says_when_pro_stops() {
        let now = chrono::Utc::now();
        let grace = License {
            valid_until: Some("2027-10-02T12:00:00Z".into()),
            ..license("grace", Some("pro"), None, None)
        };
        let shown = plan(Some(&grace), now);
        assert_eq!(shown.tier, Tier::Pro);
        assert!(
            shown.detail.starts_with("Payment failed · Pro stops"),
            "{}",
            shown.detail
        );
        assert!(shown.urgent && shown.payment_failed);
        // A complimentary plan has no payment to fail.
        let comp = plan(Some(&license("grace", Some("comp"), None, None)), now);
        assert!(!comp.payment_failed);
    }

    #[test]
    fn plans_follow_the_account_state() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-19T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let shown = |license: Option<&License>| {
            let plan = plan(license, now);
            (plan.tier, plan.detail.to_string())
        };
        let renews = license("active", Some("pro"), Some("2027-10-02T12:00:00Z"), None);
        assert_eq!(shown(Some(&renews)), (Tier::Pro, String::new()));
        let ending = license("active", Some("pro"), None, Some("2026-11-02T12:00:00Z"));
        assert_eq!(shown(Some(&ending)), (Tier::Pro, "Ends 2 Nov 2026".into()));
        let comp = license("grace", Some("comp"), None, None);
        assert_eq!(shown(Some(&comp)), (Tier::Pro, "Complimentary".into()));
        let trial = license("trial", Some("trial"), None, Some("2026-10-28T13:00:00Z"));
        assert_eq!(shown(Some(&trial)), (Tier::Trial, "10 days left".into()));
        assert!(!plan(Some(&trial), now).urgent);
        let ending_trial = license("trial", Some("trial"), None, Some("2026-10-21T13:00:00Z"));
        assert_eq!(
            shown(Some(&ending_trial)),
            (Tier::Trial, "3 days left".into())
        );
        assert!(
            plan(Some(&ending_trial), now).urgent,
            "the last 3 days are urgent"
        );
        assert!(!plan(Some(&ending), now).urgent);
        let last = license("trial", Some("trial"), None, Some("2026-10-19T13:00:00Z"));
        assert_eq!(shown(Some(&last)), (Tier::Trial, "1 day left".into()));
        let ended = license("expired", None, None, None);
        assert_eq!(
            shown(Some(&ended)),
            (Tier::Free, "Newport Pro ended".into())
        );
        let none = license("none", None, None, None);
        assert_eq!(shown(Some(&none)), (Tier::Free, String::new()));
        let mut seat = license("active", Some("pro"), Some("2027-10-02T12:00:00Z"), None);
        seat.seat = Some("free".into());
        assert_eq!(
            shown(Some(&seat)),
            (Tier::FreeSeat, "Pro is on your other computers".into())
        );
        assert_eq!(shown(None), (Tier::Free, String::new()));
    }

    mod view {
        use crate::Newport;
        use gpui_kit::{
            AppContext as _, Entity, TestAppContext, VisualTestContext,
            base::test_support::snapshots, px, size,
        };
        use newport_client::account::Account;
        use serde_json::{Value, json};
        use std::{cell::RefCell, rc::Rc};

        /// `free` is a signed-in account without Newport Pro; `named` is a
        /// Pro account whose sign-in provider gave a name.
        fn status(phase: &str) -> Value {
            let free = phase == "free";
            let named = phase == "named";
            let trial = phase == "trial";
            let grace = phase == "grace";
            let signed_in = free || named || trial || grace || phase == "signedIn";
            let license = if free {
                json!({"state":"none","plan":null})
            } else if trial {
                json!({"state":"trial","plan":"trial","seat":"trial","endsAt":"2027-10-02T12:00:00Z"})
            } else if grace {
                json!({"state":"grace","plan":"pro","seat":"pro","validUntil":"2027-10-02T12:00:00Z"})
            } else {
                json!({"state":"active","plan":"pro","renewsAt":"2027-10-02T12:00:00Z"})
            };
            json!({
                "status": if signed_in { "signedIn" } else { phase },
                "email": signed_in.then_some("ada@example.com"),
                "name": named.then_some("Ada Lovelace"),
                "license": signed_in.then_some(license),
                "signingOut": false,
                "notice": null,
            })
        }

        type Calls = Rc<RefCell<Vec<&'static str>>>;

        /// The real shell with an account that answers from `phase`.
        fn shell<'a>(
            cx: &'a mut TestAppContext,
            phase: Rc<RefCell<&'static str>>,
        ) -> (Entity<Newport>, Calls, &'a mut VisualTestContext) {
            cx.update(|cx| {
                gpui_kit::component::init(cx);
                crate::theme::install(false, cx);
                cx.set_reduce_motion(true);
            });
            let calls: Calls = Default::default();
            let mut owner = None;
            let (_, cx) = cx.add_window_view(|window, cx| {
                let view = cx.new(|cx| {
                    let mut this = crate::page_perf_tests::owner(window, cx);
                    let (recorded, phase) = (calls.clone(), phase.clone());
                    this.account = Account::with_replies(move |method| {
                        recorded.borrow_mut().push(method);
                        if method == "account_sign_in" {
                            *phase.borrow_mut() = "waiting";
                        }
                        Ok(status(*phase.borrow()))
                    });
                    this
                });
                owner = Some(view.clone());
                gpui_kit::base::Root::new(view, window, cx)
            });
            cx.simulate_resize(size(px(1000.), px(750.)));
            let owner = owner.unwrap();
            settle(&owner, cx);
            (owner, calls, cx)
        }

        fn settle(owner: &Entity<Newport>, cx: &mut VisualTestContext) {
            // Two polls: one dispatches the read, the next accepts it.
            for _ in 0..2 {
                owner.update(cx, |this, cx| {
                    let effects = this.account.poll();
                    this.apply_account_effects(effects, cx);
                    cx.notify();
                });
            }
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn labels(cx: &mut VisualTestContext) -> Vec<String> {
            cx.update(|window, _| {
                snapshots(window)
                    .into_iter()
                    .filter_map(|e| e.label().map(str::to_owned))
                    .collect()
            })
        }

        fn click(cx: &mut VisualTestContext, label: &str) {
            let center = cx.update(|window, _| {
                snapshots(window)
                    .into_iter()
                    .find(|e| e.label() == Some(label))
                    .unwrap_or_else(|| panic!("missing {label}"))
                    .bounds()
                    .center()
            });
            cx.simulate_click(center, Default::default());
        }

        #[gpui_kit::test]
        fn signing_in_from_the_sidebar_waits_on_the_browser(cx: &mut TestAppContext) {
            let phase = Rc::new(RefCell::new("signedOut"));
            let (owner, calls, cx) = shell(cx, phase);
            assert!(
                labels(cx).contains(&"Sign in".to_owned()),
                "{:?}",
                labels(cx)
            );
            click(cx, "Sign in");
            settle(&owner, cx);
            assert!(calls.borrow().contains(&"account_sign_in"));
            let labels = labels(cx);
            assert!(
                labels.contains(&"Finish signing in in your browser".to_owned()),
                "{labels:?}"
            );
            assert!(!labels.contains(&"Sign in".to_owned()));
        }

        #[gpui_kit::test]
        fn a_signed_in_computer_opens_its_account_menu(cx: &mut TestAppContext) {
            let phase = Rc::new(RefCell::new("signedIn"));
            let (owner, _, cx) = shell(cx, phase);
            let labels_before = labels(cx);
            assert!(
                labels_before.contains(&"ada@example.com".to_owned()),
                "{labels_before:?}"
            );
            assert!(labels_before.contains(&"Settings".to_owned()));
            click(cx, "ada@example.com");
            settle(&owner, cx);
            let menu = labels(cx);
            assert!(menu.contains(&"Sign out".to_owned()), "{menu:?}");
            assert!(!menu.iter().any(|label| label.contains("Upgrade to Pro")));
        }

        #[gpui_kit::test]
        fn a_trial_can_upgrade_and_a_failed_payment_can_be_fixed(cx: &mut TestAppContext) {
            let phase = Rc::new(RefCell::new("trial"));
            let (owner, _, cx) = shell(cx, phase);
            click(cx, "ada@example.com");
            settle(&owner, cx);
            let menu = labels(cx);
            assert!(
                menu.iter().any(|label| label.contains("Upgrade to Pro")),
                "{menu:?}"
            );
            assert!(
                !menu.iter().any(|label| label.contains("Update payment")),
                "{menu:?}"
            );
        }

        #[gpui_kit::test]
        fn a_failed_payment_says_when_pro_stops_and_offers_billing(cx: &mut TestAppContext) {
            let phase = Rc::new(RefCell::new("grace"));
            let (owner, _, cx) = shell(cx, phase);
            click(cx, "ada@example.com");
            settle(&owner, cx);
            let menu = labels(cx);
            assert!(
                menu.iter().any(|label| label.contains("Update payment")),
                "{menu:?}"
            );
        }

        #[gpui_kit::test]
        fn a_free_account_is_offered_the_upgrade(cx: &mut TestAppContext) {
            let phase = Rc::new(RefCell::new("free"));
            let (owner, _, cx) = shell(cx, phase);
            click(cx, "ada@example.com");
            settle(&owner, cx);
            let menu = labels(cx);
            assert!(
                menu.iter().any(|label| label.contains("Upgrade to Pro")),
                "{menu:?}"
            );
        }

        #[gpui_kit::test]
        fn a_provider_name_leads_the_row(cx: &mut TestAppContext) {
            let phase = Rc::new(RefCell::new("named"));
            let (_, _, cx) = shell(cx, phase);
            let labels = labels(cx);
            assert!(labels.contains(&"Ada Lovelace".to_owned()), "{labels:?}");
            assert!(!labels.contains(&"ada@example.com".to_owned()));
        }

        #[gpui_kit::test]
        fn signing_out_asks_first(cx: &mut TestAppContext) {
            let phase = Rc::new(RefCell::new("signedIn"));
            let (owner, calls, cx) = shell(cx, phase);
            click(cx, "ada@example.com");
            settle(&owner, cx);
            click(cx, "Sign out");
            settle(&owner, cx);
            assert!(
                !calls.borrow().contains(&"account_sign_out"),
                "nothing before confirming"
            );
            assert!(
                labels(cx).contains(&"Cancel".to_owned()),
                "{:?}",
                labels(cx)
            );
            click(cx, "Sign out");
            settle(&owner, cx);
            assert!(calls.borrow().contains(&"account_sign_out"));
        }

        #[gpui_kit::test]
        fn a_waiting_sign_in_can_be_cancelled_from_the_row(cx: &mut TestAppContext) {
            let phase = Rc::new(RefCell::new("waiting"));
            let (owner, calls, cx) = shell(cx, phase);
            click(cx, "Cancel sign-in");
            settle(&owner, cx);
            assert!(calls.borrow().contains(&"account_cancel_sign_in"));
        }
    }
}
