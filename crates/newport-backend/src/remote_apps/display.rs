//! Shared-display prerequisites. Installed dependencies are not a live display.
use newport_domain::requirements::{Finding, Gap, Requirement};
use serde_json::{Value, json};

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

pub(super) fn status(value: Option<&str>) -> (Value, Option<Gap>) {
    let parsed = value.and_then(version);
    let compatible = parsed.map(|v| v >= [23, 1, 0]);
    let state = match (value, compatible) {
        (Some("missing"), _) => "missing",
        (_, Some(true)) => "available",
        (_, Some(false)) => "outdated",
        _ => "unverified",
    };
    let shown = parsed.map(|[a, b, c]| format!("{a}.{b}.{c}"));
    let information = json!({
        "state": state,
        "installed": value.map(|v| v != "missing"),
        "version": shown,
        "minimumVersion": "23.1.0",
        "compatible": if state == "missing" { Some(false) } else { compatible },
    });
    let finding = match state {
        "available" => return (information, None),
        "missing" => Finding::Missing,
        "outdated" => Finding::Outdated {
            version: shown.unwrap_or_default(),
        },
        _ => Finding::Unverified,
    };
    (
        information,
        Some(Gap {
            requirement: Requirement::Xwayland,
            finding,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_old_and_unverified_are_distinct_from_available() {
        for (input, state, installed, compatible) in [
            (Some("missing"), "missing", Some(false), Some(false)),
            (Some("22.1.9"), "outdated", Some(true), Some(false)),
            (Some("23.0.99"), "outdated", Some(true), Some(false)),
            (Some("23.1.0"), "available", Some(true), Some(true)),
            (Some("24.1.6-1"), "available", Some(true), Some(true)),
            (Some("unverified"), "unverified", Some(true), None),
            (None, "unverified", None, None),
        ] {
            let (info, missing) = status(input);
            assert_eq!(info["state"], state);
            assert_eq!(info["installed"], json!(installed));
            assert_eq!(info["compatible"], json!(compatible));
            assert_eq!(missing.is_none(), state == "available");
            if let Some(gap) = missing {
                assert_eq!(gap.requirement, Requirement::Xwayland);
                assert_eq!(gap.installs(), state != "unverified");
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
            assert_eq!(status(Some(input)).0["state"], "unverified");
        }
    }
}
