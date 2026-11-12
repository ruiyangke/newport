//! Shared-display prerequisites. Installed dependencies are not a live display.
use newport_domain::requirements::{Finding, Gap, Requirement};

pub(super) const PROBE: &str = include_str!("display_probe.sh");

fn version(value: &str) -> Option<[u32; 3]> {
    if value.len() > 64 {
        return None;
    }
    let mut parts = value.split('.');
    let result = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.split('-').next()?.parse().ok()?,
    ];
    parts.next().is_none().then_some(result)
}

/// The XWayland gap for a probed version, or none when it is usable.
pub(super) fn gap(value: Option<&str>) -> Option<Gap> {
    let parsed = value.and_then(version);
    let finding = match (value, parsed.map(|v| v >= [23, 1, 0])) {
        (Some("missing"), _) => Finding::Missing,
        (_, Some(true)) => return None,
        (_, Some(false)) => Finding::Outdated {
            version: parsed.map(|[a, b, c]| format!("{a}.{b}.{c}"))?,
        },
        _ => Finding::Unverified,
    };
    Some(Gap {
        requirement: Requirement::Xwayland,
        finding,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_old_and_unverified_are_distinct_from_available() {
        for (input, finding) in [
            (Some("missing"), Some(Finding::Missing)),
            (
                Some("22.1.9"),
                Some(Finding::Outdated {
                    version: "22.1.9".into(),
                }),
            ),
            (
                Some("23.0.99"),
                Some(Finding::Outdated {
                    version: "23.0.99".into(),
                }),
            ),
            (Some("23.1.0"), None),
            (Some("24.1.6-1"), None),
            (Some("unverified"), Some(Finding::Unverified)),
            (None, Some(Finding::Unverified)),
        ] {
            let found = gap(input);
            assert_eq!(found.as_ref().map(|g| &g.finding), finding.as_ref());
            if let Some(gap) = found {
                assert_eq!(gap.requirement, Requirement::Xwayland);
                assert_eq!(gap.installs(), gap.finding != Finding::Unverified);
            }
        }
        for input in [
            "24",
            "24.1",
            "24.1.6.2",
            "999999999999.1.0",
            "unknown",
            "24.1.0\nmissing=dbus",
        ] {
            assert_eq!(gap(Some(input)).unwrap().finding, Finding::Unverified);
        }
    }
}
