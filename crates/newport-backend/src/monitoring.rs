//! Validate remote data before it reaches the UI or background sampler.
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    hostname: String,
    os: String,
    kernel: String,
    cores: u32,
    uptime: f64,
    load: [f64; 3],
    cpu: f64,
    /// Each online core's usage over the sampling interval.
    cpus: Vec<Core>,
    memory_total: u64,
    memory_used: u64,
    swap_total: u64,
    swap_used: u64,
    disks: Vec<Disk>,
    network: Vec<Network>,
    process_count: u64,
    processes: Vec<Process>,
}
#[derive(Debug, Deserialize, Serialize)]
struct Core {
    id: u32,
    usage: f64,
    /// The agent's last minute for this core, one value per second.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    history: Vec<f64>,
}
#[derive(Debug, Deserialize, Serialize)]
struct Disk {
    mount: String,
    device: String,
    total: u64,
    used: u64,
    available: i64,
}
#[derive(Debug, Deserialize, Serialize)]
struct Network {
    name: String,
    received: u64,
    sent: u64,
}
#[derive(Debug, Deserialize, Serialize)]
struct Process {
    pid: u32,
    user: String,
    cpu: f64,
    memory: u64,
    state: String,
    name: String,
    /// Seconds since the process started; absent when unknowable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    elapsed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
}
pub fn parse_overview(output: &str) -> Result<Overview, String> {
    serde_json::from_str(output).map_err(|e| format!("Invalid metrics response: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metrics_round_trip_preserves_the_frontend_contract() {
        let input = include_str!("../tests/fixtures/overview.json");
        let expected: serde_json::Value = serde_json::from_str(input).unwrap();
        let actual = serde_json::to_value(parse_overview(input).unwrap()).unwrap();
        assert_eq!(actual, expected);
        let mut invalid = expected;
        invalid["network"][0]["received"] = "not a counter".into();
        assert!(parse_overview(&invalid.to_string()).is_err());
    }
    #[test]
    fn per_core_usage_is_required_and_validated() {
        let input = include_str!("../tests/fixtures/overview.json");
        let mut missing: serde_json::Value = serde_json::from_str(input).unwrap();
        missing.as_object_mut().unwrap().remove("cpus");
        assert!(parse_overview(&missing.to_string()).is_err());
        let mut invalid: serde_json::Value = serde_json::from_str(input).unwrap();
        invalid["cpus"][0]["id"] = (-1).into();
        assert!(parse_overview(&invalid.to_string()).is_err());
    }
    #[test]
    fn rejects_well_formed_json_with_invalid_metrics_shape() {
        for response in ["null", "[]", "42", "{}", r#"{"cpu":42,"network":[]}"#] {
            assert!(parse_overview(response).is_err(), "{response}");
        }
    }
}
