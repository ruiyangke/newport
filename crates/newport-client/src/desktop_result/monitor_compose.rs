//! Explicit monitor reply fields at the UI rendering boundary.
use newport_protocol::monitor::compose::*;
use serde_json::{Value, json};
fn cli_kind(value: CliKind) -> &'static str {
    match value {
        CliKind::Plugin => "plugin",
        CliKind::Standalone => "standalone",
        CliKind::Podman => "podman",
    }
}
pub(super) fn cli(value: Cli) -> Value {
    json!({"kind":cli_kind(value.kind), "version":value.version})
}
pub(super) fn file(value: File) -> Value {
    json!({"path":value.path, "text":value.text, "error":value.error})
}
pub(super) fn service(value: Service) -> Value {
    json!({"name":value.name, "image":value.image, "build":value.build, "profiles":value.profiles})
}
pub(super) fn project(value: Project) -> Value {
    json!({"cli":value.cli.map(cli), "cliError":value.cli_error, "workingDir":value.working_dir, "configFiles":value.config_files, "files":value.files.into_iter().map(file).collect::<Vec<_>>(), "services":value.services.map(|value| value.into_iter().map(service).collect::<Vec<_>>()), "configError":value.config_error})
}
pub(super) fn started(value: Started) -> Value {
    json!({"job":value.job})
}
fn state(value: State) -> &'static str {
    match value {
        State::Running => "running",
        State::Succeeded => "succeeded",
        State::Failed => "failed",
        State::Cancelled => "cancelled",
    }
}
pub(super) fn job(value: Job) -> Value {
    json!({"job":value.job, "project":value.project, "command":value.command, "state":state(value.state), "exitCode":value.exit_code, "startedAt":value.started_at, "finishedAt":value.finished_at, "output":value.output, "offset":value.offset})
}
pub(super) fn jobs(value: Jobs) -> Value {
    json!({"jobs":value.jobs.into_iter().map(job).collect::<Vec<_>>()})
}
pub(super) fn done(value: Done) -> Value {
    json!({"ok":value.ok})
}
