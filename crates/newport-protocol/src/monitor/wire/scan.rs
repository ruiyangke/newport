//! Allocation-free collection and oneof checks before prost builds records.
use super::super::{compose, containers, file_watch, metrics, services};
use super::{Error, Invalid};
use crate::encoding::protobuf::fields;

pub(crate) const MAX_FIELDS: usize = 64 * 1024;
#[derive(Clone, Copy)]
enum Kind {
    Scalar,
    Integer32,
    Bool,
    Text(usize),
    Packed64,
    Message(&'static [Rule]),
    Discovery,
}
struct Rule {
    tag: u32,
    count: usize,
    group: bool,
    kind: Kind,
}

pub(super) fn check(body: &[u8], client: bool) -> Result<(), Error> {
    let schema = if client { CLIENT_FRAME } else { SERVER_FRAME };
    walk(body, schema, &mut 0)
}
fn walk(mut body: &[u8], schema: &[Rule], total: &mut usize) -> Result<(), Error> {
    let mut counts = [0usize; 32];
    let mut selected = false;
    while !body.is_empty() {
        let field = fields::field(&mut body)?;
        *total += 1;
        if *total > MAX_FIELDS {
            return Err(Error::FieldBudget);
        }
        let Some((index, rule)) = schema
            .iter()
            .enumerate()
            .find(|(_, r)| r.tag == field.number)
        else {
            continue;
        };
        let count = if matches!(rule.kind, Kind::Packed64) {
            field.fixed64_count()?
        } else {
            1
        };
        let seen = counts.get_mut(index).ok_or(Invalid("field schema"))?;
        *seen += count;
        if *seen > rule.count {
            return Err(Invalid("encoded collection count").into());
        }
        if rule.group {
            if selected {
                return Err(Invalid("duplicate operation").into());
            }
            selected = true;
        }
        match rule.kind {
            Kind::Text(cap) if field.bytes()?.len() > cap => {
                return Err(Invalid("encoded text size").into());
            }
            Kind::Text(_) | Kind::Packed64 | Kind::Scalar => {}
            Kind::Integer32 => {
                field.u32()?;
            }
            Kind::Bool if field.u32()? > 1 => return Err(Invalid("boolean value").into()),
            Kind::Bool => {}
            Kind::Message(child) => walk(field.bytes()?, child, total)?,
            Kind::Discovery => {
                *total += discovery::measure(field.bytes()?)?;
                if *total > MAX_FIELDS {
                    return Err(Error::FieldBudget);
                }
            }
        }
    }
    Ok(())
}

macro_rules! schema {
    ($vis:vis $name:ident { $($tag:literal => ($count:expr, $group:literal, $kind:expr)),* $(,)? }) => {
        $vis const $name: &[Rule] = &[$(Rule { tag: $tag, count: $count, group: $group, kind: $kind }),*];
    };
}

mod discovery;
pub(crate) use discovery::check as check_discovery;
pub(crate) use discovery::measure as measure_discovery;
pub(crate) use discovery::measure_report as measure_discovery_report;
pub(crate) use discovery::records as discovery_records;
pub(crate) use discovery::{check_helper_reply, check_helper_request};

#[cfg(test)]
use crate::desktop::wire::wire_tables::{Row, Table, Wire};

#[cfg(test)]
pub(crate) fn tables() -> Vec<(Table, &'static str)> {
    let mut tables = vec![
        (table(CLIENT_FRAME), "newport.monitor.ClientFrame"),
        (table(SERVER_FRAME), "newport.monitor.ServerFrame"),
    ];
    tables.extend(discovery::tables());
    tables
}

#[cfg(test)]
#[track_caller]
fn table(rules: &'static [Rule]) -> Table {
    Table::new(format!("monitor {:p}", rules.as_ptr()), move || {
        rules
            .iter()
            .map(|rule| {
                let wire = match rule.kind {
                    Kind::Scalar => Wire::Scalar,
                    Kind::Integer32 => Wire::Varint32,
                    Kind::Bool => Wire::Bool,
                    Kind::Text(_) => Wire::Len,
                    Kind::Packed64 => Wire::Fixed64,
                    Kind::Message(child) => Wire::Message(table(child)),
                    Kind::Discovery => Wire::Message(table(discovery::SNAPSHOT)),
                };
                Row::new(rule.tag, wire)
                    .repeated(rule.count > 1 || matches!(rule.kind, Kind::Packed64))
                    .oneof(rule.group)
            })
            .collect()
    })
}

schema!(STRING_LIST {
    1 => (containers::MAX_ITEMS, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(LABEL {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(METRICS_SNAPSHOT {
    1 => (1, false, Kind::Text(metrics::MAX_TEXT)),
    2 => (1, false, Kind::Text(metrics::MAX_TEXT)),
    3 => (1, false, Kind::Text(metrics::MAX_TEXT)),
    4 => (1, false, Kind::Integer32),
    5 => (1, false, Kind::Scalar),
    6 => (3, false, Kind::Packed64),
    7 => (1, false, Kind::Scalar),
    8 => (metrics::MAX_CPUS, false, Kind::Message(METRICS_CORE)),
    9 => (1, false, Kind::Scalar),
    10 => (1, false, Kind::Scalar),
    11 => (1, false, Kind::Scalar),
    12 => (1, false, Kind::Scalar),
    13 => (metrics::MAX_DISKS, false, Kind::Message(METRICS_DISK)),
    14 => (metrics::MAX_INTERFACES, false, Kind::Message(METRICS_NETWORK)),
    15 => (metrics::MAX_PROCESSES, false, Kind::Message(METRICS_PROCESS)),
    16 => (1, false, Kind::Integer32),
});

schema!(METRICS_CORE {
    1 => (1, false, Kind::Integer32),
    2 => (1, false, Kind::Scalar),
    3 => (metrics::MAX_HISTORY, false, Kind::Packed64),
});

schema!(METRICS_DISK {
    1 => (1, false, Kind::Text(metrics::MAX_TEXT)),
    2 => (1, false, Kind::Text(metrics::MAX_TEXT)),
    3 => (1, false, Kind::Scalar),
    4 => (1, false, Kind::Scalar),
    5 => (1, false, Kind::Scalar),
});

schema!(METRICS_NETWORK {
    1 => (1, false, Kind::Text(metrics::MAX_TEXT)),
    2 => (1, false, Kind::Scalar),
    3 => (1, false, Kind::Scalar),
});

schema!(METRICS_PROCESS {
    1 => (1, false, Kind::Integer32),
    2 => (1, false, Kind::Text(metrics::MAX_TEXT)),
    3 => (1, false, Kind::Scalar),
    4 => (1, false, Kind::Scalar),
    5 => (1, false, Kind::Text(metrics::MAX_TEXT)),
    6 => (1, false, Kind::Text(metrics::MAX_TEXT)),
    7 => (1, false, Kind::Scalar),
    8 => (1, false, Kind::Text(metrics::MAX_COMMAND)),
});

schema!(SERVICES_LOGS_REQUEST {
    1 => (1, false, Kind::Integer32),
    2 => (1, false, Kind::Text(services::MAX_TEXT)),
    3 => (1, false, Kind::Integer32),
    4 => (1, false, Kind::Text(services::MAX_TEXT)),
    5 => (1, false, Kind::Integer32),
});

schema!(SERVICES_SERVICE {
    1 => (1, false, Kind::Text(services::MAX_TEXT)),
    2 => (1, false, Kind::Integer32),
    3 => (1, false, Kind::Text(services::MAX_TEXT)),
    4 => (1, false, Kind::Text(services::MAX_TEXT)),
    5 => (1, false, Kind::Text(services::MAX_TEXT)),
    6 => (1, false, Kind::Text(services::MAX_TEXT)),
    7 => (1, false, Kind::Text(services::MAX_TEXT)),
    8 => (1, false, Kind::Integer32),
    9 => (1, false, Kind::Scalar),
    10 => (1, false, Kind::Scalar),
    11 => (1, false, Kind::Scalar),
    12 => (1, false, Kind::Scalar),
    13 => (1, false, Kind::Text(services::MAX_TEXT)),
    14 => (1, false, Kind::Scalar),
    15 => (1, false, Kind::Integer32),
    16 => (1, false, Kind::Text(services::MAX_TEXT)),
    17 => (services::MAX_LIST, false, Kind::Text(services::MAX_TEXT)),
    18 => (services::MAX_LIST, false, Kind::Text(services::MAX_TEXT)),
    19 => (1, false, Kind::Text(services::MAX_TEXT)),
});

schema!(SERVICES_TIMER {
    1 => (1, false, Kind::Text(services::MAX_TEXT)),
    2 => (1, false, Kind::Integer32),
    3 => (1, false, Kind::Text(services::MAX_TEXT)),
    4 => (1, false, Kind::Text(services::MAX_TEXT)),
    5 => (1, false, Kind::Scalar),
    6 => (1, false, Kind::Scalar),
    7 => (1, false, Kind::Text(services::MAX_TEXT)),
});

schema!(SERVICES_USER_MANAGER {
    1 => (1, false, Kind::Bool),
    2 => (1, false, Kind::Text(services::MAX_TEXT)),
});

schema!(SERVICES_SNAPSHOT {
    1 => (services::MAX_SERVICES, false, Kind::Message(SERVICES_SERVICE)),
    2 => (services::MAX_TIMERS, false, Kind::Message(SERVICES_TIMER)),
    3 => (1, false, Kind::Message(SERVICES_USER_MANAGER)),
    4 => (1, false, Kind::Integer32),
});

schema!(SERVICES_ENTRY {
    1 => (1, false, Kind::Scalar),
    2 => (1, false, Kind::Integer32),
    3 => (1, false, Kind::Integer32),
    4 => (1, false, Kind::Text(services::MAX_TEXT)),
    5 => (1, false, Kind::Text(services::MAX_TEXT)),
});

schema!(SERVICES_LOGS {
    1 => (services::MAX_LOG_ENTRIES, false, Kind::Message(SERVICES_ENTRY)),
    2 => (1, false, Kind::Text(services::MAX_TEXT)),
});

schema!(SERVICES_UNIT_FILE {
    1 => (1, false, Kind::Text(services::MAX_UNIT_FILE)),
});

schema!(SERVICES_ACTION_REPLY {
    1 => (1, false, Kind::Bool),
    2 => (1, false, Kind::Bool),
    3 => (1, false, Kind::Text(services::MAX_TEXT)),
});

schema!(CONTAINERS_ENGINE_INFO {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Text(containers::MAX_TEXT)),
    3 => (1, false, Kind::Text(containers::MAX_TEXT)),
    4 => (1, false, Kind::Bool),
    5 => (1, false, Kind::Bool),
});

schema!(CONTAINERS_CONTAINER_PORT {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Integer32),
    3 => (1, false, Kind::Integer32),
    4 => (1, false, Kind::Integer32),
});

schema!(CONTAINERS_COMPOSE_LABELS {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Text(containers::MAX_TEXT)),
    3 => (1, false, Kind::Bool),
    4 => (1, false, Kind::Scalar),
    5 => (1, false, Kind::Text(containers::MAX_TEXT)),
    6 => (1, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(CONTAINERS_CONTAINER {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Text(containers::MAX_TEXT)),
    3 => (1, false, Kind::Text(containers::MAX_TEXT)),
    4 => (1, false, Kind::Text(containers::MAX_TEXT)),
    5 => (1, false, Kind::Text(containers::MAX_TEXT)),
    6 => (1, false, Kind::Integer32),
    7 => (1, false, Kind::Text(containers::MAX_TEXT)),
    8 => (1, false, Kind::Integer32),
    9 => (1, false, Kind::Scalar),
    10 => (1, false, Kind::Bool),
    11 => (1, false, Kind::Text(containers::MAX_TEXT)),
    12 => (1, false, Kind::Scalar),
    13 => (1, false, Kind::Text(containers::MAX_TEXT)),
    14 => (1, false, Kind::Scalar),
    15 => (1, false, Kind::Scalar),
    16 => (1, false, Kind::Scalar),
    17 => (containers::MAX_ITEMS, false, Kind::Message(CONTAINERS_CONTAINER_PORT)),
    18 => (1, false, Kind::Message(CONTAINERS_COMPOSE_LABELS)),
    19 => (1, false, Kind::Scalar),
    20 => (1, false, Kind::Scalar),
    21 => (1, false, Kind::Scalar),
});

schema!(CONTAINERS_CONTAINERS_SNAPSHOT {
    1 => (1, false, Kind::Message(CONTAINERS_ENGINE_INFO)),
    2 => (containers::MAX_CONTAINERS, false, Kind::Message(CONTAINERS_CONTAINER)),
    3 => (1, false, Kind::Bool),
});

schema!(CONTAINERS_ENV_VAR {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(CONTAINERS_MOUNT {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Text(containers::MAX_TEXT)),
    3 => (1, false, Kind::Text(containers::MAX_TEXT)),
    4 => (1, false, Kind::Text(containers::MAX_TEXT)),
    5 => (1, false, Kind::Bool),
    6 => (1, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(CONTAINERS_NETWORK {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Text(containers::MAX_TEXT)),
    3 => (containers::MAX_ITEMS, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(CONTAINERS_HEALTHCHECK {
    1 => (containers::MAX_ITEMS, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Scalar),
});

schema!(CONTAINERS_HEALTH_RESULT {
    1 => (1, false, Kind::Scalar),
    2 => (1, false, Kind::Scalar),
    3 => (1, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(CONTAINERS_CONTAINER_INSPECT {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Text(containers::MAX_TEXT)),
    3 => (1, false, Kind::Text(containers::MAX_TEXT)),
    4 => (1, false, Kind::Scalar),
    5 => (1, false, Kind::Message(STRING_LIST)),
    6 => (1, false, Kind::Message(STRING_LIST)),
    7 => (1, false, Kind::Text(containers::MAX_TEXT)),
    8 => (1, false, Kind::Text(containers::MAX_TEXT)),
    9 => (1, false, Kind::Text(containers::MAX_TEXT)),
    10 => (containers::MAX_ITEMS, false, Kind::Message(CONTAINERS_ENV_VAR)),
    11 => (containers::MAX_ITEMS, false, Kind::Message(CONTAINERS_MOUNT)),
    12 => (containers::MAX_ITEMS, false, Kind::Message(CONTAINERS_NETWORK)),
    13 => (1, false, Kind::Text(containers::MAX_TEXT)),
    14 => (containers::MAX_ITEMS, false, Kind::Message(LABEL)),
    15 => (1, false, Kind::Message(CONTAINERS_HEALTHCHECK)),
    16 => (containers::MAX_ITEMS, false, Kind::Message(CONTAINERS_HEALTH_RESULT)),
});

schema!(CONTAINERS_CONTAINER_LOG_ENTRY {
    1 => (1, false, Kind::Scalar),
    2 => (1, false, Kind::Integer32),
    3 => (1, false, Kind::Text(containers::MAX_TEXT)),
    4 => (1, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(CONTAINERS_CONTAINER_LOG_PAGE {
    1 => (containers::MAX_LOG_ENTRIES, false, Kind::Message(CONTAINERS_CONTAINER_LOG_ENTRY)),
    2 => (1, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(CONTAINERS_DOCKER_IMAGE {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (containers::MAX_ITEMS, false, Kind::Text(containers::MAX_TEXT)),
    3 => (1, false, Kind::Scalar),
    4 => (1, false, Kind::Scalar),
    5 => (1, false, Kind::Scalar),
    6 => (1, false, Kind::Scalar),
    7 => (1, false, Kind::Bool),
});

schema!(CONTAINERS_DOCKER_VOLUME {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Text(containers::MAX_TEXT)),
    3 => (1, false, Kind::Scalar),
    4 => (1, false, Kind::Scalar),
    5 => (1, false, Kind::Bool),
    6 => (1, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(CONTAINERS_DISK_TOTALS {
    1 => (1, false, Kind::Scalar),
    2 => (1, false, Kind::Scalar),
    3 => (1, false, Kind::Scalar),
});

schema!(CONTAINERS_DOCKER_TOTALS {
    1 => (1, false, Kind::Message(CONTAINERS_DISK_TOTALS)),
    2 => (1, false, Kind::Message(CONTAINERS_DISK_TOTALS)),
    3 => (1, false, Kind::Message(CONTAINERS_DISK_TOTALS)),
    4 => (1, false, Kind::Message(CONTAINERS_DISK_TOTALS)),
});

schema!(CONTAINERS_DOCKER_DISK {
    1 => (containers::MAX_DISK_ROWS, false, Kind::Message(CONTAINERS_DOCKER_IMAGE)),
    2 => (containers::MAX_DISK_ROWS, false, Kind::Message(CONTAINERS_DOCKER_VOLUME)),
    3 => (1, false, Kind::Message(CONTAINERS_DOCKER_TOTALS)),
    4 => (1, false, Kind::Bool),
});

schema!(CONTAINERS_PRUNED {
    1 => (1, false, Kind::Scalar),
    2 => (1, false, Kind::Scalar),
});

schema!(CONTAINERS_DONE {
    1 => (1, false, Kind::Bool),
});

schema!(CONTAINERS_REMOVED {
    1 => (1, false, Kind::Scalar),
});

schema!(COMPOSE_CLI {
    1 => (1, false, Kind::Integer32),
    2 => (1, false, Kind::Text(compose::MAX_TEXT)),
});

schema!(COMPOSE_FILE {
    1 => (1, false, Kind::Text(compose::MAX_TEXT)),
    2 => (1, false, Kind::Text(compose::MAX_FILE_TEXT)),
    3 => (1, false, Kind::Text(compose::MAX_TEXT)),
});

schema!(COMPOSE_SERVICE {
    1 => (1, false, Kind::Text(compose::MAX_TEXT)),
    2 => (1, false, Kind::Text(compose::MAX_TEXT)),
    3 => (1, false, Kind::Bool),
    4 => (compose::MAX_ITEMS, false, Kind::Text(compose::MAX_TEXT)),
});

schema!(COMPOSE_PROJECT {
    1 => (1, false, Kind::Message(COMPOSE_CLI)),
    2 => (1, false, Kind::Text(compose::MAX_TEXT)),
    3 => (1, false, Kind::Text(compose::MAX_TEXT)),
    4 => (compose::MAX_FILES, false, Kind::Text(compose::MAX_TEXT)),
    5 => (compose::MAX_FILES, false, Kind::Message(COMPOSE_FILE)),
    6 => (1, false, Kind::Message(COMPOSE_SERVICE_LIST)),
    7 => (1, false, Kind::Text(compose::MAX_TEXT)),
});

schema!(COMPOSE_STARTED {
    1 => (1, false, Kind::Text(compose::MAX_TEXT)),
});

schema!(COMPOSE_JOB {
    1 => (1, false, Kind::Text(compose::MAX_TEXT)),
    2 => (1, false, Kind::Text(compose::MAX_TEXT)),
    3 => (1, false, Kind::Text(compose::MAX_TEXT)),
    4 => (1, false, Kind::Integer32),
    5 => (1, false, Kind::Integer32),
    6 => (1, false, Kind::Scalar),
    7 => (1, false, Kind::Scalar),
    8 => (1, false, Kind::Text(compose::MAX_OUTPUT)),
    9 => (1, false, Kind::Scalar),
});

schema!(COMPOSE_JOBS {
    1 => (compose::MAX_JOBS, false, Kind::Message(COMPOSE_JOB)),
});

schema!(COMPOSE_DONE {
    1 => (1, false, Kind::Bool),
});

schema!(DISCOVERY_REQUEST {
    1 => (1, false, Kind::Bool),
});

schema!(WATCH_REQUEST {
    1 => (1, false, Kind::Text(file_watch::MAX_PATH)),
    2 => (1, false, Kind::Text(file_watch::MAX_PATH)),
    3 => (file_watch::MAX_DIRECTORIES, false, Kind::Text(file_watch::MAX_PATH)),
    4 => (1, false, Kind::Text(file_watch::MAX_CURSOR)),
});

schema!(WATCH_REPLY {
    1 => (1, false, Kind::Text(file_watch::MAX_CURSOR)),
    2 => (file_watch::MAX_PATHS, false, Kind::Text(file_watch::MAX_PATH)),
    3 => (1, false, Kind::Bool),
});

schema!(DISPLAY_STATE {
    1 => (1, false, Kind::Bool),
    2 => (1, false, Kind::Message(DISPLAY_VIEWER)),
});

schema!(DISPLAY_VIEWER {
    1 => (1, false, Kind::Text(128)),
    2 => (1, false, Kind::Text(1024)),
});

schema!(SERVICES_UNIT {
    1 => (1, false, Kind::Integer32),
    2 => (1, false, Kind::Text(services::MAX_TEXT)),
});

schema!(SERVICES_ACTION {
    1 => (1, false, Kind::Integer32),
    2 => (1, false, Kind::Text(services::MAX_TEXT)),
    3 => (1, false, Kind::Integer32),
});

schema!(SERVICES_REQUEST {
    1 => (1, true, Kind::Message(PRESENT)),
    2 => (1, true, Kind::Message(SERVICES_LOGS_REQUEST)),
    3 => (1, true, Kind::Message(SERVICES_UNIT)),
    4 => (1, true, Kind::Message(SERVICES_ACTION)),
});

schema!(SERVICES_REPLY {
    1 => (1, true, Kind::Message(SERVICES_SNAPSHOT)),
    2 => (1, true, Kind::Message(SERVICES_LOGS)),
    3 => (1, true, Kind::Message(SERVICES_UNIT_FILE)),
    4 => (1, true, Kind::Message(SERVICES_ACTION_REPLY)),
});

schema!(CONTAINERS_TARGET {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(CONTAINERS_LOGS_REQUEST {
    1 => (1, true, Kind::Text(containers::MAX_TEXT)),
    2 => (1, true, Kind::Text(containers::MAX_TEXT)),
    3 => (1, false, Kind::Text(containers::MAX_TEXT)),
    4 => (1, false, Kind::Integer32),
});

schema!(CONTAINERS_ACTION {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Integer32),
});

schema!(CONTAINERS_PROJECT_ACTION {
    1 => (1, false, Kind::Text(containers::MAX_TEXT)),
    2 => (1, false, Kind::Integer32),
    3 => (containers::MAX_IDS, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(CONTAINERS_REMOVE {
    1 => (containers::MAX_IDS, false, Kind::Text(containers::MAX_TEXT)),
});

schema!(CONTAINERS_DISK_REQUEST {
    1 => (1, false, Kind::Integer32),
});

schema!(CONTAINERS_PRUNE {
    1 => (1, false, Kind::Integer32),
});

schema!(CONTAINERS_REQUEST {
    1 => (1, true, Kind::Message(PRESENT)),
    2 => (1, true, Kind::Message(CONTAINERS_TARGET)),
    3 => (1, true, Kind::Message(CONTAINERS_LOGS_REQUEST)),
    4 => (1, true, Kind::Message(CONTAINERS_ACTION)),
    5 => (1, true, Kind::Message(CONTAINERS_PROJECT_ACTION)),
    6 => (1, true, Kind::Message(CONTAINERS_REMOVE)),
    7 => (1, true, Kind::Message(CONTAINERS_DISK_REQUEST)),
    8 => (1, true, Kind::Message(CONTAINERS_PRUNE)),
});

schema!(CONTAINERS_REPLY {
    1 => (1, true, Kind::Message(CONTAINERS_CONTAINERS_SNAPSHOT)),
    2 => (1, true, Kind::Message(CONTAINERS_CONTAINER_INSPECT)),
    3 => (1, true, Kind::Message(CONTAINERS_CONTAINER_LOG_PAGE)),
    4 => (1, true, Kind::Message(CONTAINERS_DONE)),
    5 => (1, true, Kind::Message(CONTAINERS_DONE)),
    6 => (1, true, Kind::Message(CONTAINERS_REMOVED)),
    7 => (1, true, Kind::Message(CONTAINERS_DOCKER_DISK)),
    8 => (1, true, Kind::Message(CONTAINERS_PRUNED)),
});

schema!(COMPOSE_UP {
    1 => (compose::MAX_SERVICES, false, Kind::Text(compose::MAX_TEXT)),
    2 => (1, false, Kind::Bool),
    3 => (1, false, Kind::Bool),
    4 => (1, false, Kind::Bool),
});

schema!(COMPOSE_DOWN {
    1 => (1, false, Kind::Bool),
    2 => (1, false, Kind::Bool),
});

schema!(COMPOSE_PULL {
    1 => (compose::MAX_SERVICES, false, Kind::Text(compose::MAX_TEXT)),
});

schema!(COMPOSE_BUILD {
    1 => (compose::MAX_SERVICES, false, Kind::Text(compose::MAX_TEXT)),
    2 => (1, false, Kind::Bool),
});

schema!(COMPOSE_SCALE {
    1 => (1, false, Kind::Text(compose::MAX_TEXT)),
    2 => (1, false, Kind::Integer32),
});

schema!(COMPOSE_ACTION {
    1 => (1, true, Kind::Message(COMPOSE_UP)),
    2 => (1, true, Kind::Message(COMPOSE_DOWN)),
    3 => (1, true, Kind::Message(COMPOSE_PULL)),
    4 => (1, true, Kind::Message(COMPOSE_BUILD)),
    5 => (1, true, Kind::Message(COMPOSE_SCALE)),
});

schema!(COMPOSE_PROJECT_REQUEST {
    1 => (1, false, Kind::Text(compose::MAX_TEXT)),
});

schema!(COMPOSE_RUN {
    1 => (1, false, Kind::Text(compose::MAX_TEXT)),
    2 => (1, false, Kind::Message(COMPOSE_ACTION)),
});

schema!(COMPOSE_JOB_REQUEST {
    1 => (1, false, Kind::Text(compose::MAX_TEXT)),
    2 => (1, false, Kind::Scalar),
});

schema!(COMPOSE_CANCEL {
    1 => (1, false, Kind::Text(compose::MAX_TEXT)),
});

schema!(COMPOSE_REQUEST {
    1 => (1, true, Kind::Message(COMPOSE_PROJECT_REQUEST)),
    2 => (1, true, Kind::Message(COMPOSE_RUN)),
    3 => (1, true, Kind::Message(COMPOSE_JOB_REQUEST)),
    4 => (1, true, Kind::Message(COMPOSE_CANCEL)),
    5 => (1, true, Kind::Message(PRESENT)),
});

schema!(COMPOSE_REPLY {
    1 => (1, true, Kind::Message(COMPOSE_PROJECT)),
    2 => (1, true, Kind::Message(COMPOSE_STARTED)),
    3 => (1, true, Kind::Message(COMPOSE_JOB)),
    4 => (1, true, Kind::Message(COMPOSE_DONE)),
    5 => (1, true, Kind::Message(COMPOSE_JOBS)),
});

schema!(CALL {
    1 => (1, false, Kind::Scalar),
    2 => (1, true, Kind::Message(PRESENT)),
    3 => (1, true, Kind::Message(PRESENT)),
    4 => (1, true, Kind::Message(SERVICES_REQUEST)),
    5 => (1, true, Kind::Message(CONTAINERS_REQUEST)),
    6 => (1, true, Kind::Message(COMPOSE_REQUEST)),
    9 => (1, true, Kind::Message(DISCOVERY_REQUEST)),
    8 => (1, true, Kind::Message(WATCH_REQUEST)),
});
schema!(CLIENT_FRAME {
    1 => (1, true, Kind::Message(CALL)),
});

schema!(RESPONSE {
    1 => (1, false, Kind::Scalar),
    2 => (1, true, Kind::Message(METRICS_SNAPSHOT)),
    3 => (1, true, Kind::Message(DISPLAY_STATE)),
    4 => (1, true, Kind::Message(SERVICES_REPLY)),
    5 => (1, true, Kind::Message(CONTAINERS_REPLY)),
    6 => (1, true, Kind::Message(COMPOSE_REPLY)),
    10 => (1, true, Kind::Discovery),
    8 => (1, true, Kind::Message(WATCH_REPLY)),
    9 => (1, true, Kind::Message(ERROR)),
});
schema!(SERVER_FRAME {
    1 => (1, true, Kind::Message(RESPONSE)),
});

schema!(COMPOSE_SERVICE_LIST {
    1 => (compose::MAX_ITEMS, false, Kind::Message(COMPOSE_SERVICE)),
});

schema!(PRESENT {});

schema!(ERROR {
    1 => (1, false, Kind::Text(64)),
    2 => (1, false, Kind::Text(super::super::MAX_ERROR_MESSAGE)),
    3 => (1, false, Kind::Integer32),
});
