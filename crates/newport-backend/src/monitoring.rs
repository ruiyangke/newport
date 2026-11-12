//! Validate remote data before it reaches the UI or background sampler.
use newport_protocol::monitor::metrics::Snapshot;

pub fn parse_overview(snapshot: Snapshot) -> Result<Snapshot, String> {
    snapshot.validate().map_err(|e| e.to_string())?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse_overview(value: serde_json::Value) -> Result<Snapshot, String> {
        super::parse_overview(serde_json::from_value(value).map_err(|e| e.to_string())?)
    }
    #[test]
    fn metrics_round_trip_preserves_the_ui_contract() {
        let input = include_str!("../tests/fixtures/overview.json");
        let expected: serde_json::Value = serde_json::from_str(input).unwrap();
        let actual = serde_json::to_value(parse_overview(expected.clone()).unwrap()).unwrap();
        assert_eq!(actual, expected);
        let mut invalid = expected;
        invalid["network"][0]["received"] = "not a counter".into();
        assert!(parse_overview(invalid).is_err());
    }
    #[test]
    fn per_core_usage_is_required_and_validated() {
        let input = include_str!("../tests/fixtures/overview.json");
        let mut missing: serde_json::Value = serde_json::from_str(input).unwrap();
        missing.as_object_mut().unwrap().remove("cpus");
        assert!(parse_overview(missing).is_err());
        let mut invalid: serde_json::Value = serde_json::from_str(input).unwrap();
        invalid["cpus"][0]["id"] = (-1).into();
        assert!(parse_overview(invalid).is_err());
    }
    #[test]
    fn rejects_well_formed_json_with_invalid_metrics_shape() {
        for response in ["null", "[]", "42", "{}", r#"{"cpu":42,"network":[]}"#] {
            assert!(
                parse_overview(serde_json::from_str(response).unwrap()).is_err(),
                "{response}"
            );
        }
    }

    #[test]
    fn rejects_out_of_range_or_unbounded_metrics_before_the_ui() {
        let input = include_str!("../tests/fixtures/overview.json");
        let valid: serde_json::Value = serde_json::from_str(input).unwrap();
        for path in ["cpu", "uptime"] {
            let mut invalid = valid.clone();
            invalid[path] = (-1).into();
            assert!(parse_overview(invalid).is_err());
        }
        let mut invalid = valid.clone();
        invalid["cpus"][0]["usage"] = 101.into();
        assert!(parse_overview(invalid).is_err());
        let mut invalid = valid.clone();
        invalid["memoryUsed"] = 8193.into();
        assert!(parse_overview(invalid).is_err());
        let mut invalid = valid;
        invalid["processes"][0]["command"] = "x"
            .repeat(newport_protocol::monitor::metrics::MAX_COMMAND + 1)
            .into();
        assert!(parse_overview(invalid).is_err());
    }
}
