//! Newport's existing CSS design tokens, expressed in logical pixels for GPUI.
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, px, rgb};

// GPUI resolves this to the native UI family on each platform. Apple's private
// family name bypasses that mapping and falls back to another font on Windows.
pub const UI_FONT: &str = ".SystemUIFont";

pub fn install(dark: bool, cx: &mut App) {
    Theme::change(
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    // Kit controls paint resolved tokens, while application views also read the
    // palette. The supported update path synchronizes both and the Base theme.
    Theme::update(cx, |theme| {
        theme.font_family = UI_FONT.into();
        theme.font_size = px(14.);
        theme.radius = px(5.);
        theme.radius_lg = px(8.);
        let c = &mut theme.colors;
        c.background = rgb(if dark { 0x1e1f21 } else { 0xffffff }).into();
        c.foreground = rgb(if dark { 0xeeeeef } else { 0x25262a }).into();
        c.muted_foreground = rgb(if dark { 0xa9abb3 } else { 0x626670 }).into();
        c.border = rgb(if dark { 0x3c3b40 } else { 0xdedde1 }).into();
        c.input = c.border;
        // React's Switch uses the border token for its off track and a white
        // thumb in both appearances, independent of the page background.
        c.switch = c.border;
        c.switch_thumb = rgb(0xffffff).into();
        c.sidebar = rgb(if dark { 0x343337 } else { 0xf5f4f6 }).into();
        c.sidebar_foreground = c.foreground;
        c.sidebar_accent = rgb(if dark { 0x454349 } else { 0xdad9dd }).into();
        c.sidebar_accent_foreground = c.foreground;
        c.sidebar_border = c.border;
        c.accent = rgb(if dark { 0x37383d } else { 0xe8e7eb }).into();
        c.accent_foreground = c.foreground;
        c.primary = rgb(if dark { 0x4b96f6 } else { 0x1768d7 }).into();
        c.primary_foreground = rgb(if dark { 0x101b29 } else { 0xffffff }).into();
        c.primary_hover = c.primary.opacity(0.8);
        c.primary_active = c.primary_hover;
        // These are resolved component fields, not live aliases of `primary`.
        c.button_primary = c.primary;
        c.button_primary_foreground = c.primary_foreground;
        c.button_primary_hover = c.primary_hover;
        c.button_primary_active = c.primary_active;
        c.ring = rgb(if dark { 0x8e9ab2 } else { 0x8192ac }).into();
        c.popover = rgb(if dark { 0x222730 } else { 0xffffff }).into();
        c.popover_foreground = c.foreground;
        c.secondary = c.background;
        c.secondary_foreground = c.foreground;
        // Flat native tabs join the panel; these resolved colors do not follow
        // background/foreground automatically when the app palette changes.
        c.tab_active = c.background;
        c.tab_active_foreground = c.foreground;
        c.tab_foreground = c.muted_foreground;
        // Match the Tauri dialog backdrop in both appearances.
        c.overlay = gpui_kit::hsla(0., 0., 0., if dark { 0.42 } else { 0.22 });

        // Syntax themes supply token colors, not a separate editor surface.
        // Keep the fixed gutter opaque for horizontal scrolling, but match it
        // to the code canvas so its existing spacing reads as one workspace.
        let editor = &mut std::sync::Arc::make_mut(&mut theme.highlight_theme).style;
        editor.editor_background = Some(c.background);
        editor.editor_gutter_background = Some(c.background);
        editor.editor_foreground = Some(c.foreground);
        editor.editor_line_number = Some(c.muted_foreground);
        editor.editor_active_line_number = Some(c.foreground);
        editor.editor_active_line =
            Some(
                c.background
                    .blend(c.foreground.opacity(if dark { 0.04 } else { 0.035 })),
            );
    });
}

/// Neutral feedback shared by list and table rows; selection keeps its own tint.
pub fn row_hover(cx: &App) -> gpui_kit::Hsla {
    let theme = Theme::global(cx);
    theme
        .foreground
        .opacity(if theme.is_dark() { 0.06 } else { 0.04 })
}

pub fn sidebar_foreground(dark: bool) -> gpui_kit::Hsla {
    rgb(if dark { 0xeeeeef } else { 0x191a1e }).into()
}
pub fn sidebar_muted(dark: bool) -> gpui_kit::Hsla {
    rgb(if dark { 0xa9abb3 } else { 0x494c55 }).into()
}
pub fn sidebar_selection(dark: bool) -> gpui_kit::Hsla {
    sidebar_foreground(dark).opacity(if dark { 0.13 } else { 0.09 })
}

pub fn success(dark: bool) -> gpui_kit::Hsla {
    rgb(if dark { 0x79cea9 } else { 0x197451 }).into()
}
pub fn error(dark: bool) -> gpui_kit::Hsla {
    rgb(if dark { 0xff96a6 } else { 0xba3345 }).into()
}
pub fn warning(dark: bool) -> gpui_kit::Hsla {
    rgb(if dark { 0xdbbc76 } else { 0x896218 }).into()
}
pub fn sidebar_action(dark: bool) -> gpui_kit::Hsla {
    if dark {
        sidebar_muted(true)
    } else {
        sidebar_foreground(false)
    }
}

// Shared with the Tauri diff palette in diffExtensions.ts. Opaque surfaces
// keep unified/split previews consistent regardless of their parent surface.
pub fn diff_add(dark: bool) -> gpui_kit::Hsla {
    rgb(if dark { 0x1e2e26 } else { 0xedf8f0 }).into()
}
pub fn diff_add_ink(dark: bool) -> gpui_kit::Hsla {
    gpui_kit::rgb(if dark { 0x79cea9 } else { 0x24714b }).into()
}
pub fn diff_remove_ink(dark: bool) -> gpui_kit::Hsla {
    gpui_kit::rgb(if dark { 0xfb8a8d } else { 0xac3448 }).into()
}
pub fn diff_remove(dark: bool) -> gpui_kit::Hsla {
    rgb(if dark { 0x382227 } else { 0xfff0f1 }).into()
}
pub fn diff_hunk(dark: bool) -> gpui_kit::Hsla {
    rgb(if dark { 0x242830 } else { 0xf1f8ff }).into()
}
pub fn git_selection(dark: bool) -> gpui_kit::Hsla {
    rgb(if dark { 0x253d58 } else { 0xe5effb }).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit as gpui;

    #[gpui::test]
    fn appearance_changes_keep_controls_and_base_tokens_in_sync(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_kit::component::init);
        for dark in [false, true, false] {
            cx.update(|cx| {
                install(dark, cx);
                let theme = Theme::global(cx);
                let primary: gpui::Hsla = rgb(if dark { 0x4b96f6 } else { 0x1768d7 }).into();
                assert_eq!(theme.tokens.primary.color, primary);
                assert_eq!(theme.tokens.button_primary.color, primary);
                assert_eq!(
                    theme.tokens.button_primary_hover.color,
                    primary.opacity(0.8)
                );
                assert_eq!(
                    theme.tokens.button_primary_foreground.color,
                    theme.primary_foreground
                );
                assert_eq!(theme.tokens.background.color, theme.background);
                assert_eq!(theme.tokens.ring.color, theme.ring);
                assert_eq!(theme.tokens.switch.color, theme.border);
                assert_eq!(theme.tokens.switch_thumb.color, rgb(0xffffff).into());
                let base = gpui_kit::base::Theme::global(cx);
                assert_eq!(base.tokens.colors.primary, primary);
                assert_eq!(base.tokens.colors.background, theme.background);
            });
        }
    }
}
