//! Offline native accessibility probe. No backend, profile or user data.
//! Inspect this window with an accessibility client, then focus each field.
//! The console reports whether the accessibility focus follows keyboard focus.
use gpui_kit::{
    component::input::{Input, InputState, Textarea, TextareaState},
    prelude::*,
    *,
};

struct Probe {
    single: Entity<InputState>,
    multiple: Entity<TextareaState>,
    last_report: Option<String>,
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(tree) = window.debug_a11y_tree_json() {
            let tree: serde_json::Value = serde_json::from_str(&tree).expect("accessibility tree");
            let focus = tree["gpui_focus"].as_str().unwrap_or_default();
            let report = serde_json::json!({
                "keyboard_focus": format!("{:?}", window.focused(cx)),
                "accessibility_focus": tree["nodes"][focus],
                "single_focused": self.single.read(cx).focus_handle(cx).is_focused(window),
                "multiple_focused": self.multiple.read(cx).focus_handle(cx).is_focused(window),
            })
            .to_string();
            if self.last_report.as_ref() != Some(&report) {
                println!("{report}");
                self.last_report = Some(report);
            }
        }
        div()
            .size_full()
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(Input::new(&self.single).aria_label("Fixture search"))
            .child(
                Textarea::new(&self.multiple)
                    .aria_label("Fixture preview")
                    .h(px(150.)),
            )
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let (_, view) = gpui_kit::open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Newport input focus probe".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| Probe {
                    single: cx.new(|cx| InputState::new(window, cx).placeholder("Fixture search")),
                    multiple: cx
                        .new(|cx| TextareaState::new(window, cx).placeholder("Fixture preview")),
                    last_report: None,
                })
            },
        )
        .expect("open offline input probe");
        // Inspect the completed preceding frame rather than a partial tree.
        // A weak entity lets closing the window terminate the observer.
        let view = view.downgrade();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                if view.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.activate(true);
    });
}
