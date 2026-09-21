//! Validate remote data before it reaches the UI or background sampler.
use serde::{Deserialize, Serialize};

pub const MAX_COLLECTION_ROWS: usize = 500;

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
    memory_total: u64,
    memory_used: u64,
    swap_total: u64,
    swap_used: u64,
    disks: Vec<Disk>,
    network: Vec<Network>,
    process_count: u64,
    process_cpu_mode: String,
    processes: Vec<Process>,
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
}
#[derive(Debug, Serialize)]
pub struct Service {
    name: String,
    load: String,
    active: String,
    sub: String,
    description: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Container {
    id: String,
    name: String,
    image: String,
    state: String,
    status: String,
    ports: String,
    compose_project: String,
    compose_service: String,
    compose_oneoff: String,
}

pub fn parse_overview(output: &str) -> Result<Overview, String> {
    serde_json::from_str(output).map_err(|e| format!("Invalid metrics response: {e}"))
}
pub fn parse_services(output: &str) -> Vec<Service> {
    let mut rows: Vec<_> = output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next()?;
            if !name.ends_with(".service") {
                return None;
            }
            Some(Service {
                name: name.into(),
                load: fields.next()?.into(),
                active: fields.next()?.into(),
                sub: fields.next()?.into(),
                description: fields.collect::<Vec<_>>().join(" "),
            })
        })
        .collect();
    rows.sort_by(|a, b| (a.active != "failed", &a.name).cmp(&(b.active != "failed", &b.name)));
    rows.truncate(MAX_COLLECTION_ROWS);
    rows
}
pub fn parse_containers(output: &str) -> Result<Vec<Container>, String> {
    output
        .lines()
        .take(MAX_COLLECTION_ROWS)
        .map(|line| serde_json::from_str(line).map_err(|e| format!("Invalid Docker response: {e}")))
        .collect()
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
    fn rejects_well_formed_json_with_invalid_metrics_shape() {
        for response in ["null", "[]", "42", "{}", r#"{"cpu":42,"network":[]}"#] {
            assert!(parse_overview(response).is_err(), "{response}");
        }
    }
    #[test]
    fn services_preserve_descriptions_and_sort_failed_first() {
        let rows = parse_services("z.service loaded active running Background worker\na.service loaded failed failed Failed job\nb.service loaded active running SSH server\ninvalid line");
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["a.service", "b.service", "z.service"]
        );
        assert_eq!(rows[2].description, "Background worker");
    }
    #[test]
    fn services_are_bounded_after_prioritizing_failures() {
        let mut input = (0..600)
            .map(|n| format!("worker-{n}.service loaded active running Worker\n"))
            .collect::<String>();
        input.push_str("critical.service loaded failed failed Critical\n");
        let rows = parse_services(&input);
        assert_eq!(rows.len(), MAX_COLLECTION_ROWS);
        assert_eq!(rows[0].name, "critical.service");
    }
}
