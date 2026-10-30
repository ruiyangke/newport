//! Caption buttons for Windows, where Newport draws its own title bar.
//!
//! The buttons are native caption areas: Windows handles their clicks, shows
//! Snap Layouts over Maximize, and closing goes through the same request as
//! the system button. They follow the Windows 11 caption metrics and glyphs.
use super::*;
use std::sync::OnceLock;

/// Windows 11 caption buttons are 46px wide and as tall as the title bar.
const WIDTH: f32 = 46.;
/// The Close button's hover and pressed fills.
const CLOSE_HOVER: u32 = 0xc42b1c;
const CLOSE_PRESSED: u32 = 0xb0271a;

#[derive(Clone, Copy)]
enum Caption {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Caption {
    fn glyph(self) -> &'static str {
        match self {
            Self::Minimize => "\u{e921}",
            Self::Maximize => "\u{e922}",
            Self::Restore => "\u{e923}",
            Self::Close => "\u{e8bb}",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Minimize => "Minimize",
            Self::Maximize => "Maximize",
            Self::Restore => "Restore",
            Self::Close => "Close",
        }
    }
    fn area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize | Self::Restore => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }
}

/// The caption buttons; none in full screen, where Windows offers no caption.
pub(super) fn caption_buttons(window: &Window, cx: &App) -> Option<AnyElement> {
    if window.is_fullscreen() {
        return None;
    }
    Some(buttons(
        window.is_maximized(),
        window.is_window_active(),
        icon_font(cx),
        cx,
    ))
}

/// Segoe Fluent Icons ships with Windows 11; Windows 10 has the same glyphs
/// in Segoe MDL2 Assets.
fn icon_font(cx: &App) -> &'static str {
    static FONT: OnceLock<&'static str> = OnceLock::new();
    FONT.get_or_init(|| {
        if cx
            .text_system()
            .all_font_names()
            .iter()
            .any(|name| name == "Segoe Fluent Icons")
        {
            "Segoe Fluent Icons"
        } else {
            "Segoe MDL2 Assets"
        }
    })
}

fn buttons(maximized: bool, active: bool, font: &'static str, cx: &App) -> AnyElement {
    let glyph_color = if active {
        cx.theme().foreground
    } else {
        cx.theme().muted_foreground
    };
    div()
        .id("window-caption-buttons")
        .debug_selector(|| "window-caption-buttons".into())
        .flex()
        .flex_shrink_0()
        .h_full()
        .children(
            [
                Caption::Minimize,
                if maximized {
                    Caption::Restore
                } else {
                    Caption::Maximize
                },
                Caption::Close,
            ]
            .map(|caption| {
                let close = matches!(caption, Caption::Close);
                div()
                    .id(caption.label())
                    .debug_selector(move || format!("window-caption-{}", caption.label()))
                    .role(accesskit::Role::Button)
                    .aria_label(caption.label())
                    .window_control_area(caption.area())
                    .w(px(WIDTH))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(font)
                    .text_size(px(10.))
                    .text_color(glyph_color)
                    .map(|button| {
                        if close {
                            button
                                .hover(|style| {
                                    style.bg(rgb(CLOSE_HOVER)).text_color(gpui_kit::white())
                                })
                                .active(|style| {
                                    style.bg(rgb(CLOSE_PRESSED)).text_color(gpui_kit::white())
                                })
                        } else {
                            let hover = theme::row_hover(cx);
                            button
                                .hover(move |style| style.bg(hover))
                                .active(move |style| style.bg(hover.opacity(0.66)))
                        }
                    })
                    .child(caption.glyph())
            }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    struct Host(bool);
    impl Render for Host {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .h(px(crate::ui::TITLEBAR_HEIGHT))
                .child(div().flex_1())
                .child(buttons(self.0, true, "Segoe Fluent Icons", cx))
        }
    }

    fn draw(maximized: bool, cx: &mut TestAppContext) -> &mut VisualTestContext {
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            crate::theme::install(true, cx);
        });
        let (_, visual) = cx.add_window_view(|_, _| Host(maximized));
        visual.simulate_resize(gpui::size(px(400.), px(200.)));
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual
    }

    #[gpui::test]
    fn caption_buttons_fill_the_title_bar_at_windows_widths(cx: &mut TestAppContext) {
        let visual = draw(false, cx);
        let close = visual.debug_bounds("window-caption-Close").unwrap();
        assert_eq!(
            close.size,
            gpui::size(px(WIDTH), px(crate::ui::TITLEBAR_HEIGHT))
        );
        assert_eq!(close.right(), px(400.));
        assert!(visual.debug_bounds("window-caption-Minimize").is_some());
        assert!(visual.debug_bounds("window-caption-Maximize").is_some());
        assert!(visual.debug_bounds("window-caption-Restore").is_none());
    }

    #[gpui::test]
    fn a_maximized_window_offers_restore(cx: &mut TestAppContext) {
        let visual = draw(true, cx);
        assert!(visual.debug_bounds("window-caption-Restore").is_some());
        assert!(visual.debug_bounds("window-caption-Maximize").is_none());
    }
}
