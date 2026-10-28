//! Standalone server setup design preview. All server data and operations are simulated.
use crate::*;

#[derive(Default)]
struct Preview {
    step: usize,
    elapsed: u32,
    installing: bool,
    failed: bool,
    dark: bool,
    tools: bool,
    paused: bool,
    browser: bool,
    clipboard: bool,
}
impl Preview {
    fn busy(&self) -> bool {
        (self.step == 0 && self.elapsed < 3) || self.installing
    }
    fn tick(&mut self) {
        if self.failed || self.paused || !self.busy() {
            return;
        }
        self.elapsed += 1;
        if self.installing && self.elapsed >= 23 {
            self.installing = false;
            self.step = 2;
            self.elapsed = 0;
        }
    }
    fn consent(&mut self) {
        if self.step == 1 && !self.installing {
            self.installing = true;
            self.elapsed = 0;
        }
    }
    fn title(&self) -> &'static str {
        if self.failed {
            return if self.step == 0 {
                "Couldn’t inspect this server"
            } else {
                "Needle installation stopped"
            };
        }
        match self.step {
            0 if self.busy() => "Inspecting your server",
            0 => "Server inspection complete",
            1 if self.installing && self.elapsed >= 20 => "Installing Needle",
            1 if self.installing => "Copying Needle to your server",
            1 => "May we install Needle?",
            2 => "Make this server feel local",
            _ => "Your server is ready",
        }
    }
    fn description(&self) -> &'static str {
        if self.failed {
            return "The SSH connection was interrupted. Check your connection, then try again.";
        }
        match self.step {
            0 if self.busy() => {
                "Checking the connection and collecting basic system information. Nothing is being installed."
            }
            0 => "Your server is ready for the next step. Nothing has been installed.",
            1 if self.installing => {
                "Keep Newport open. Setup will continue when the transfer and verification finish."
            }
            1 => {
                "Needle is a small tool that lets Newport collect live CPU, memory, network, and storage readings."
            }
            2 => "Choose how remote commands connect to your computer.",
            _ => {
                "Needle is installed. You can change your integration choices later in Integrations."
            }
        }
    }
    fn fact(label: &str, value: &str, cx: &App) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .py(px(9.))
            .text_size(px(crate::ui::FONT_SIZE))
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_owned()),
            )
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(value.to_owned()),
            )
            .into_any_element()
    }
    fn content(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex().flex_col().gap(px(12.));
        if self.failed {
            return body.text_size(px(crate::ui::FONT_SIZE)).line_height(relative(1.5)).text_color(cx.theme().muted_foreground)
                .child(if self.installing { "Try again to resend the incomplete transfer. Integrations haven’t been enabled." } else { "Nothing has been installed. You can try the inspection again." }).into_any_element();
        }
        match self.step {
            0 => {
                let ready = !self.busy();
                let mut facts = div()
                    .flex()
                    .flex_col()
                    .px(px(16.))
                    .py(px(6.))
                    .rounded(px(crate::ui::RADIUS))
                    .bg(cx.theme().sidebar);
                for (label, value) in [
                    ("System", "Ubuntu 24.04 LTS · x86_64"),
                    ("Processor", "4 cores"),
                    ("Memory", "8 GiB"),
                    ("Available storage", "42 GiB"),
                ] {
                    facts = facts.child(Self::fact(
                        label,
                        if ready { value } else { "Checking…" },
                        cx,
                    ));
                }
                body = body.child(facts).child(
                    div()
                        .text_size(px(crate::ui::FONT_SIZE))
                        .text_color(cx.theme().muted_foreground)
                        .child(if ready {
                            "Inspection complete. Needle is not installed yet."
                        } else {
                            "Reading system information over SSH…"
                        }),
                );
            }
            1 if self.installing => {
                let fraction = (self.elapsed as f32 / 20.).min(1.);
                body = body
                    .py(px(12.))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(px(crate::ui::FONT_SIZE))
                            .child(if self.elapsed < 20 {
                                "Transferring Needle"
                            } else {
                                "Verifying installation"
                            })
                            .child(format!("{:.0}%", fraction * 100.)),
                    )
                    .child(
                        div()
                            .id("needle-upload")
                            .role(Role::ProgressIndicator)
                            .aria_label(format!("Needle transfer: {:.0}%", fraction * 100.))
                            .w_full()
                            .h(px(4.))
                            .rounded_full()
                            .bg(cx.theme().muted)
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(fraction))
                                    .rounded_full()
                                    .bg(cx.theme().primary),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(px(crate::ui::FONT_SIZE))
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{:.1} of 5.0 MiB", fraction * 5.))
                            .child(if self.elapsed < 20 {
                                "256 KiB/s"
                            } else {
                                "Transfer complete"
                            }),
                    );
            }
            1 => {
                body = body.child(div().flex().flex_col().px(px(16.)).py(px(6.)).rounded(px(crate::ui::RADIUS)).bg(cx.theme().sidebar)
                    .child(Self::fact("Tool", "Needle", cx))
                    .child(Self::fact("Size", "5.0 MiB · sample", cx))
                    .child(Self::fact("Transfer", "Over your SSH connection", cx)))
                    .child(div().text_size(px(crate::ui::FONT_SIZE)).line_height(relative(1.5)).text_color(cx.theme().muted_foreground)
                        .child("Installed only with your permission. Browser and clipboard access stay off until you choose them in the next step."));
            }
            2 => {
                body = body
                    .child(self.integration_option(true, cx))
                    .child(self.integration_option(false, cx))
                    .child(
                        div()
                            .text_size(px(crate::ui::FONT_SIZE))
                            .text_color(cx.theme().muted_foreground)
                            .child("Optional. You can change these later in Integrations."),
                    );
            }
            _ => {
                body = body
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .px(px(16.))
                            .py(px(6.))
                            .rounded(px(crate::ui::RADIUS))
                            .bg(cx.theme().sidebar)
                            .child(Self::fact("Needle", "Installed", cx))
                            .child(Self::fact(
                                "Browser",
                                if self.browser { "Enabled" } else { "Off" },
                                cx,
                            ))
                            .child(Self::fact(
                                "Clipboard",
                                if self.clipboard { "Enabled" } else { "Off" },
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(crate::ui::FONT_SIZE))
                            .text_color(cx.theme().muted_foreground)
                            .child("Preview complete. No server was changed."),
                    );
            }
        }
        body.into_any_element()
    }
    fn integration_option(&self, browser: bool, cx: &mut Context<Self>) -> AnyElement {
        let checked = if browser {
            self.browser
        } else {
            self.clipboard
        };
        let label = if browser {
            "Open server links in your browser"
        } else {
            "Share your clipboard with this server"
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.))
            .py(px(8.))
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_1()
                    .text_size(px(crate::ui::FONT_SIZE))
                    .child(label),
            )
            .child(
                crate::controls::switch(if browser {
                    "setup-browser"
                } else {
                    "setup-clipboard"
                })
                .accessibility_label(label)
                .checked(checked)
                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                    if browser {
                        this.browser = *checked;
                    } else {
                        this.clipboard = *checked;
                    }
                    cx.notify();
                })),
            )
            .into_any_element()
    }
    fn actions(&self, cx: &mut Context<Self>) -> AnyElement {
        // Mirrors the setup flow: 28px buttons, primary action first.
        let mut row = div().flex().flex_wrap().items_center().gap(px(6.));
        if self.failed {
            row = row.child(
                crate::controls::button("setup-retry")
                    .primary()
                    .label("Try again")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.failed = false;
                        this.elapsed = 0;
                        cx.notify();
                    })),
            );
        } else if self.busy() {
            row = row.child(
                crate::controls::button("setup-waiting")
                    .disabled(true)
                    .label(if self.installing {
                        "Installing…"
                    } else {
                        "Inspecting…"
                    }),
            );
        } else {
            match self.step {
                0 => {
                    row = row.child(
                        crate::controls::button("setup-continue")
                            .primary()
                            .label("Continue")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.step = 1;
                                this.elapsed = 0;
                                cx.notify();
                            })),
                    )
                }
                1 => {
                    row = row
                        .child(
                            crate::controls::button("setup-consent")
                                .primary()
                                .label("Allow")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.consent();
                                    cx.notify();
                                })),
                        )
                        .child(
                            crate::controls::button("setup-decline")
                                .ghost()
                                .label("Skip")
                                .on_click(|_, _, cx| cx.quit()),
                        )
                }
                2 => {
                    row = row.child(
                        crate::controls::button("setup-finish")
                            .primary()
                            .label("Save and finish")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.step = 3;
                                cx.notify();
                            })),
                    )
                }
                _ => {
                    row = row.child(
                        crate::controls::button("setup-restart")
                            .label("Restart preview")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let dark = this.dark;
                                *this = Self {
                                    dark,
                                    ..Self::default()
                                };
                                cx.notify();
                            })),
                    )
                }
            }
        }
        row.into_any_element()
    }
}
impl Render for Preview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.content(cx);
        let actions = self.actions(cx);
        div()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .font_family(crate::theme::UI_FONT)
            .flex()
            .flex_col()
            .child(
                div()
                    .p(px(16.))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                Icon::new(IconName::Laptop)
                                    .size(px(20.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.))
                                    .child(
                                        div()
                                            .text_size(px(crate::ui::FONT_SIZE))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("demo-server"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(crate::ui::FONT_SIZE))
                                            .text_color(cx.theme().muted_foreground)
                                            .child("developer · SSH"),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(crate::ui::FONT_SIZE))
                            .text_color(cx.theme().muted_foreground)
                            .child(if self.step < 3 {
                                format!("Step {} of 3", self.step + 1)
                            } else {
                                "Complete".into()
                            }),
                    ),
            )
            .child(
                div()
                    .id("setup-page")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(16.))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(24.))
                            .pt(px(12.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .id("setup-title")
                                            .role(Role::Status)
                                            .test_support()
                                            .aria_label(self.title())
                                            .text_size(px(crate::ui::PAGE_TITLE))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(self.title()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(crate::ui::FONT_SIZE))
                                            .line_height(relative(1.6))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(self.description()),
                                    ),
                            )
                            .child(content),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .p(px(16.))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(actions),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .p(px(16.))
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(crate::ui::FONT_SIZE))
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Preview · sample server · no changes are made"),
                            )
                            .child(
                                crate::controls::button("preview-tools")
                                    .label("Preview tools")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.tools = !this.tools;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(self.tools, |v| {
                        v.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(6.))
                                .child(
                                    crate::controls::button("preview-complete")
                                        .label("Complete operation")
                                        .disabled(!self.busy() || self.failed)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if this.busy() && !this.failed {
                                                this.elapsed = if this.installing { 22 } else { 2 };
                                                let paused = this.paused;
                                                this.paused = false;
                                                this.tick();
                                                this.paused = paused;
                                            }
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    crate::controls::button("preview-failure")
                                        .label("Simulate failure")
                                        .disabled(!self.busy() || self.failed)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.failed = true;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    crate::controls::button("preview-pause")
                                        .label(if self.paused {
                                            "Resume simulation"
                                        } else {
                                            "Pause simulation"
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.paused = !this.paused;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    crate::controls::button("preview-theme")
                                        .label(if self.dark {
                                            "Light theme"
                                        } else {
                                            "Dark theme"
                                        })
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.dark = !this.dark;
                                            theme::install(this.dark, cx);
                                            cx.notify();
                                        })),
                                ),
                        )
                    }),
            )
    }
}

pub fn run() -> Result<(), String> {
    let app = gpui_kit::application().with_assets(assets::Assets);
    let error = std::rc::Rc::new(std::cell::RefCell::new(None));
    let startup_error = error.clone();
    app.run(move |cx| {
        gpui_kit::init(cx);
        theme::install(false, cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if let Err(err) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(560.), px(580.)),
                    cx,
                ))),
                window_min_size: Some(size(px(380.), px(480.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Newport — Server Setup Preview".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                window.set_rem_size(px(14.));
                let view = cx.new(|cx: &mut Context<Preview>| {
                    cx.spawn(async move |view, cx| {
                        loop {
                            cx.background_executor().timer(Duration::from_secs(1)).await;
                            if view
                                .update(cx, |this, cx| {
                                    this.tick();
                                    cx.notify();
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                    .detach();
                    Preview::default()
                });
                cx.new(|cx| gpui_kit::base::Root::new(view, window, cx))
            },
        ) {
            *startup_error.borrow_mut() = Some(err.to_string());
            cx.quit();
        }
        cx.activate(true);
    });
    error.borrow_mut().take().map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::Preview;
    use crate::theme;
    use gpui_kit::{self as gpui, Role, TestAppContext, px, size};
    #[test]
    fn inspection_and_installation_never_bypass_consent() {
        let mut preview = Preview::default();
        for _ in 0..100 {
            preview.tick();
        }
        assert_eq!(
            (preview.step, preview.elapsed, preview.installing),
            (0, 3, false)
        );
        preview.step = 1;
        for _ in 0..100 {
            preview.tick();
        }
        assert!(!preview.installing);
        assert_eq!(preview.step, 1);
        preview.consent();
        preview.failed = true;
        for _ in 0..30 {
            preview.tick();
        }
        assert_eq!(preview.elapsed, 0);
        preview.failed = false;
        for _ in 0..23 {
            preview.tick();
        }
        assert_eq!(preview.step, 2);
        for _ in 0..100 {
            preview.tick();
        }
        assert_eq!(preview.step, 2);
        assert!(!preview.browser && !preview.clipboard);
    }
    #[gpui::test]
    fn preview_walks_through_inspection_consent_and_optional_integrations(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::install(false, cx);
        });
        let (_, visual) = cx.add_window_view(|_, _| Preview::default());
        visual.simulate_resize(size(px(520.), px(850.)));
        for (button, expected) in [
            ("Preview tools", "Inspecting your server"),
            ("Simulate failure", "Couldn’t inspect this server"),
            ("Try again", "Inspecting your server"),
            ("Complete operation", "Server inspection complete"),
            ("Continue", "May we install Needle?"),
            ("Allow", "Copying Needle to your server"),
            ("Simulate failure", "Needle installation stopped"),
            ("Try again", "Copying Needle to your server"),
            ("Complete operation", "Make this server feel local"),
            ("Dark theme", "Make this server feel local"),
            ("Save and finish", "Your server is ready"),
        ] {
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let target = visual.update(|window, _| {
                gpui_kit::base::test_support::snapshots(window)
                    .into_iter()
                    .find(|node| node.label() == Some(button) && node.role() == Some(Role::Button))
                    .unwrap()
                    .bounds()
            });
            visual.simulate_click(target.center(), Default::default());
            visual.update(|window, cx| {
                window.draw(cx).clear(cx);
                assert!(
                    gpui_kit::base::test_support::snapshots(window)
                        .iter()
                        .any(|node| node.label() == Some(expected)),
                    "missing {expected}"
                );
            });
        }
    }
}
