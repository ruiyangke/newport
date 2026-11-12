//! Test-only desktop peer. All socket bytes use the shipping protobuf codec.
use newport_platform::ipc::{self, framed};
use newport_protocol::{
    common,
    desktop::{self, containers, services, wire},
    monitor,
};
use serde_json::{Value, json};
use std::{
    net::Shutdown,
    time::{Duration, Instant},
};

pub(super) const SERVER: &str = "00000000-0000-0000-0000-000000000001";
const WAIT: Duration = Duration::from_secs(2);

pub(super) struct Observation {
    pub method: String,
    pub args: Value,
}

pub(super) struct Peer {
    // Keep the same decoder and tail alive through every exchange phase.
    _reader: framed::Reader,
    writer: framed::Writer,
    request: desktop::Request,
}
impl Peer {
    pub(super) fn admit(stream: ipc::Stream) -> Self {
        stream.set_nonblocking(false).unwrap();
        let deadline = Instant::now() + WAIT;
        let mut writer = framed::Writer::new(stream.try_clone().unwrap());
        let mut reader = framed::Reader::new(stream);
        let frame = reader
            .next_until(desktop::HANDSHAKE_LIMITS.message, deadline)
            .unwrap()
            .unwrap();
        let hello = wire::decode_client(frame.body(), false).unwrap();
        assert!(
            matches!(&hello, desktop::ClientMessage::Hello(value) if value.version == desktop::VERSION)
        );
        let mut phase = desktop::Exchange::default();
        phase.client(&hello).unwrap();
        let ready = desktop::ServerMessage::Ready(Box::new(desktop::Ready {
            contract_id: Some(newport_protocol::desktop::CONTRACT_ID.into()),
            version: desktop::VERSION,
            backend_instance: uuid::Uuid::from_u128(2),
        }));
        phase.server(&ready).unwrap();
        writer
            .send_until(&wire::encode_server(&ready).unwrap(), deadline)
            .unwrap();
        let frame = reader
            .next_until(desktop::MAX_REQUEST, deadline)
            .unwrap()
            .unwrap();
        let message = wire::decode_client(frame.body(), false).unwrap();
        phase.client(&message).unwrap();
        let desktop::ClientMessage::Request(request) = message else {
            panic!("expected admitted request")
        };
        assert!(matches!(
            &request.operation,
            desktop::Operation::Services(_) | desktop::Operation::Containers(_)
        ));
        Self {
            _reader: reader,
            writer,
            request: *request,
        }
    }

    pub(super) fn observation(&self) -> Observation {
        let refresh = |mut value: Value, enabled| {
            if enabled {
                value["refresh"] = Value::Bool(true);
            }
            value
        };
        let (method, args) = match &self.request.operation {
            desktop::Operation::Containers(operation) => match operation.as_ref() {
                containers::Operation::Snapshot(v) => (
                    "containers_snapshot",
                    refresh(json!({"id":v.server}), v.refresh),
                ),
                containers::Operation::Inspect(v) => (
                    "container_inspect",
                    refresh(json!({"id":v.server,"container":v.container}), v.refresh),
                ),
                containers::Operation::Logs(v) => {
                    let (container, project) = match &v.request.target {
                        monitor::containers::LogTarget::Container(v) => (Some(v), None),
                        monitor::containers::LogTarget::Project(v) => (None, Some(v)),
                    };
                    (
                        "container_logs",
                        json!({"id":v.server,"container":container,"project":project,"cursor":v.request.cursor,"lines":v.request.lines}),
                    )
                }
                containers::Operation::Action(v) => (
                    "container_action",
                    json!({"id":v.server,"container":v.container,"verb":v.verb.as_str()}),
                ),
                containers::Operation::ProjectAction(v) => (
                    "project_action",
                    json!({"id":v.server,"project":v.project,"verb":v.verb.as_str(),"expectedIds":v.expected_ids}),
                ),
                containers::Operation::Remove(v) => (
                    "containers_remove",
                    json!({"id":v.server,"containers":v.containers}),
                ),
                containers::Operation::Prune(v) => (
                    "docker_prune",
                    json!({"id":v.server,"what":v.what.as_str()}),
                ),
                containers::Operation::Disk(v) => (
                    "docker_disk",
                    refresh(
                        json!({"id":v.server,"part":match v.part { containers::DiskPart::Full => "full", containers::DiskPart::Quick => "quick", containers::DiskPart::Volumes => "volumes" }}),
                        v.refresh,
                    ),
                ),
                containers::Operation::ComposeProject(v) => (
                    "compose_project",
                    refresh(json!({"id":v.server,"project":v.project}), v.refresh),
                ),
                containers::Operation::ComposeRun(v) => (
                    "compose_run",
                    json!({"id":v.server,"project":v.project,"action":v.action}),
                ),
                containers::Operation::ComposeJob(v) => (
                    "compose_job",
                    json!({"id":v.server,"job":v.job,"offset":v.offset}),
                ),
                containers::Operation::ComposeCancel(v) => {
                    ("compose_cancel", json!({"id":v.server,"job":v.job}))
                }
                containers::Operation::ComposeJobs(v) => ("compose_jobs", json!({"id":v.server})),
            },
            desktop::Operation::Services(operation) => match operation.as_ref() {
                services::Operation::Snapshot(v) => (
                    "services_snapshot",
                    refresh(json!({"id":v.server}), v.refresh),
                ),
                services::Operation::UnitFile(v) => (
                    "service_unit_file",
                    refresh(
                        json!({"id":v.server,"unit":v.unit,"scope":scope(v.scope)}),
                        v.refresh,
                    ),
                ),
                services::Operation::Logs(v) => (
                    "service_logs",
                    json!({"id":v.server,"unit":v.request.unit,"scope":scope(v.request.scope),"priority":v.request.priority.map(|v|v.as_str()),"cursor":v.request.cursor,"lines":v.request.lines}),
                ),
                services::Operation::Action(v) => (
                    "service_action",
                    json!({"id":v.server,"unit":v.unit,"scope":scope(v.scope),"verb":v.verb.as_str()}),
                ),
            },
            _ => unreachable!("fixture admits only its typed service families"),
        };
        Observation {
            method: method.into(),
            args,
        }
    }

    pub(super) fn reply(&mut self, fixture: Result<Value, String>) {
        let outcome = match fixture.and_then(|value| fixture_reply(&self.request.operation, value))
        {
            Ok(reply) => desktop::Outcome::Completed(Box::new(reply)),
            Err(message) => failure(message),
        };
        let response = desktop::ServerMessage::Response(Box::new(desktop::Response {
            id: self.request.id,
            outcome,
        }));
        let bytes = wire::encode_server_for(&response, &self.request).unwrap_or_else(|error| {
            // An intentionally malformed fixture is a typed failed result, not
            // a different on-wire protocol or an uncorrelated peer panic.
            wire::encode_server_for(
                &desktop::ServerMessage::Response(Box::new(desktop::Response {
                    id: self.request.id,
                    outcome: failure(format!("Invalid fixture result: {error}")),
                })),
                &self.request,
            )
            .unwrap()
        });
        self.writer
            .send_until(&bytes, Instant::now() + WAIT)
            .unwrap();
        // Windows has no half-close. Successful unary completion requires EOF
        // after the final frame, so close every retained clone explicitly.
        self.writer
            .try_clone_stream()
            .unwrap()
            .shutdown(Shutdown::Both)
            .unwrap();
    }
}
fn scope(value: monitor::services::Scope) -> &'static str {
    match value {
        monitor::services::Scope::System => "system",
        monitor::services::Scope::User => "user",
    }
}
fn failure(message: String) -> desktop::Outcome {
    desktop::Outcome::Failed(Box::new(desktop::Failure {
        error: common::Error {
            code: "fixture".into(),
            message,
            retry: common::Retry::No,
        },
        detail: None,
    }))
}

// Fixture data may remain JSON for terse UI test setup. These explicit arms
// lower only test data to typed replies; JSON never enters the transport.
fn fixture_reply(operation: &desktop::Operation, value: Value) -> Result<desktop::Reply, String> {
    let invalid = |error| format!("Invalid fixture result: {error}");
    Ok(match operation {
        desktop::Operation::Containers(operation) => {
            desktop::Reply::Containers(Box::new(match operation.as_ref() {
                containers::Operation::Snapshot(_) => containers::Reply::Snapshot(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
                containers::Operation::Inspect(_) => containers::Reply::Inspect(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
                containers::Operation::Logs(_) => containers::Reply::Logs(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
                containers::Operation::Action(_) => containers::Reply::Action(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
                containers::Operation::ProjectAction(_) => containers::Reply::ProjectAction(
                    Box::new(serde_json::from_value(value).map_err(invalid)?),
                ),
                containers::Operation::Remove(_) => containers::Reply::Remove(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
                containers::Operation::Disk(_) => containers::Reply::Disk(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
                containers::Operation::Prune(_) => containers::Reply::Prune(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
                containers::Operation::ComposeProject(_) => containers::Reply::ComposeProject(
                    Box::new(serde_json::from_value(value).map_err(invalid)?),
                ),
                containers::Operation::ComposeRun(_) => containers::Reply::ComposeRun(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
                containers::Operation::ComposeJob(_) => containers::Reply::ComposeJob(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
                containers::Operation::ComposeCancel(_) => containers::Reply::ComposeCancel(
                    Box::new(serde_json::from_value(value).map_err(invalid)?),
                ),
                containers::Operation::ComposeJobs(_) => containers::Reply::ComposeJobs(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
            }))
        }
        desktop::Operation::Services(operation) => {
            desktop::Reply::Services(Box::new(match operation.as_ref() {
                services::Operation::Snapshot(_) => services::Reply::Snapshot(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
                services::Operation::Logs(_) => {
                    services::Reply::Logs(Box::new(serde_json::from_value(value).map_err(invalid)?))
                }
                services::Operation::UnitFile(_) => services::Reply::UnitFile(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
                services::Operation::Action(_) => services::Reply::Action(Box::new(
                    serde_json::from_value(value).map_err(invalid)?,
                )),
            }))
        }
        _ => return Err("Unexpected fixture operation".into()),
    })
}
