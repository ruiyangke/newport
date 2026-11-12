//! Keyboard cursor for a loaded picker page. It never selects unloaded rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Highlight(usize),
    Activate(usize),
}

pub(super) fn asks_for_menu(key: &str, modifiers: gpui_kit::Modifiers) -> bool {
    !modifiers.control
        && !modifiers.alt
        && !modifiers.platform
        // Fn may be required to send F10 on a Mac keyboard.
        && ((key == "f10" && modifiers.shift) || (key == "menu" && !modifiers.shift))
}

pub(super) fn step(key: &str, current: Option<usize>, count: usize) -> Option<Step> {
    if count == 0 {
        return None;
    }
    let current = current.filter(|index| *index < count);
    let index = match key {
        "down" => current.map_or(0, |index| (index + 1) % count),
        "up" => current.map_or(count - 1, |index| (index + count - 1) % count),
        "pagedown" => current.map_or(0, |index| (index + 5).min(count - 1)),
        "pageup" => current.map_or(0, |index| index.saturating_sub(5)),
        "enter" => return current.map(Step::Activate),
        _ => return None,
    };
    Some(Step::Highlight(index))
}

#[cfg(test)]
mod tests {
    use super::{Step, asks_for_menu, step};

    #[test]
    fn context_menu_shortcut_does_not_capture_other_modified_keys() {
        use gpui_kit::Modifiers;
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        assert!(asks_for_menu("f10", shift));
        assert!(asks_for_menu(
            "f10",
            Modifiers {
                function: true,
                ..shift
            }
        ));
        assert!(asks_for_menu("menu", Modifiers::default()));
        assert!(!asks_for_menu("f10", Modifiers::default()));
        assert!(!asks_for_menu("menu", shift));
        assert!(!asks_for_menu(
            "f10",
            Modifiers {
                platform: true,
                ..shift
            }
        ));
        assert!(!asks_for_menu(
            "f10",
            Modifiers {
                control: true,
                ..shift
            }
        ));
        assert!(!asks_for_menu("f10", Modifiers { alt: true, ..shift }));
    }

    #[test]
    fn navigation_wraps_and_pages_only_inside_loaded_rows() {
        assert_eq!(step("down", None, 9), Some(Step::Highlight(0)));
        assert_eq!(step("up", None, 9), Some(Step::Highlight(8)));
        assert_eq!(step("down", Some(8), 9), Some(Step::Highlight(0)));
        assert_eq!(step("up", Some(0), 9), Some(Step::Highlight(8)));
        assert_eq!(step("pagedown", Some(2), 9), Some(Step::Highlight(7)));
        assert_eq!(step("pagedown", Some(7), 9), Some(Step::Highlight(8)));
        assert_eq!(step("pageup", Some(2), 9), Some(Step::Highlight(0)));
    }

    #[test]
    fn activation_requires_a_valid_cursor_and_text_editing_keys_are_untouched() {
        assert_eq!(step("enter", Some(2), 3), Some(Step::Activate(2)));
        assert_eq!(step("enter", Some(3), 3), None);
        assert_eq!(step("enter", None, 3), None);
        for key in ["up", "down", "enter", "pageup", "pagedown"] {
            assert_eq!(step(key, Some(0), 0), None);
        }
        for key in ["home", "end", "left", "right", "space", "tab", "escape"] {
            assert_eq!(step(key, Some(0), 3), None);
        }
    }
}
