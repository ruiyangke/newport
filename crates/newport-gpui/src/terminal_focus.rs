//! DECSET 1004 focus reports, independent of repaint frequency.

#[derive(Default, Clone, Copy)]
pub struct FocusReporter {
    reported: Option<bool>,
}

impl FocusReporter {
    /// Disabling reporting or losing the session discards the previous report.
    pub fn update(&mut self, enabled: bool, focused: bool) -> Option<&'static [u8]> {
        if !enabled {
            self.reported = None;
            return None;
        }
        if self.reported == Some(focused) {
            return None;
        }
        self.reported = Some(focused);
        Some(if focused { b"\x1b[I" } else { b"\x1b[O" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_in_reports_current_focus_and_only_transitions() {
        let mut reporter = FocusReporter::default();
        assert_eq!(reporter.update(false, true), None);
        assert_eq!(reporter.update(true, true), Some(&b"\x1b[I"[..]));
        assert_eq!(reporter.update(true, true), None);
        assert_eq!(reporter.update(true, false), Some(&b"\x1b[O"[..]));
        assert_eq!(reporter.update(true, false), None);
        assert_eq!(reporter.update(true, true), Some(&b"\x1b[I"[..]));
    }

    #[test]
    fn disabled_or_replaced_session_does_not_inherit_reports() {
        let mut reporter = FocusReporter::default();
        assert_eq!(reporter.update(true, false), Some(&b"\x1b[O"[..]));
        assert_eq!(reporter.update(false, false), None);
        assert_eq!(reporter.update(false, true), None);
        assert_eq!(reporter.update(true, true), Some(&b"\x1b[I"[..]));
    }
}
