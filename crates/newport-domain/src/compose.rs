//! Compose read contracts and Activity presentation rules shared by clients.
//! Field names and order match the shared backend Compose contract; job offsets are
//! cumulative UTF-8 byte positions in the agent's cleaned command output.
use crate::{
    containers::{Container, ProjectGroup, Tone, is_running},
    containers_log::{Severity, severity},
    services::duration,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ComposeCliKind {
    #[default]
    Plugin,
    Standalone,
    Podman,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ComposeCli {
    pub kind: ComposeCliKind,
    pub version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ComposeFile {
    pub path: String,
    pub text: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ComposeService {
    pub name: String,
    pub image: Option<String>,
    pub build: bool,
    pub profiles: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeProject {
    pub cli: Option<ComposeCli>,
    pub cli_error: Option<String>,
    pub working_dir: Option<String>,
    pub config_files: Vec<String>,
    pub files: Vec<ComposeFile>,
    pub services: Option<Vec<ComposeService>>,
    pub config_error: Option<String>,
}

/// A defined service (including one not created yet) or an existing replica
/// whose service is no longer in the Compose configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ServiceRow<'a> {
    pub service: &'a str,
    pub container: Option<&'a Container>,
    pub profiles: &'a [String],
    pub build: bool,
    /// False when configuration is unavailable as well as for orphan rows.
    pub defined: bool,
    /// Only known configuration can establish that a service is orphaned.
    pub orphaned: bool,
    pub running_replicas: usize,
}

/// Preserve Compose's definition order and the group's replica order. Show
/// undeclared existing services last; one-off jobs never supply a service row.
pub fn service_rows<'a>(
    group: &ProjectGroup<'a>,
    project: Option<&'a ComposeProject>,
) -> Vec<ServiceRow<'a>> {
    let defined = project.and_then(|project| project.services.as_deref());
    let mut rows = Vec::new();
    for service in defined.unwrap_or_default() {
        let replicas = service_replicas(group, &service.name);
        let mut members = group
            .services
            .iter()
            .copied()
            .filter(|container| {
                container
                    .compose
                    .as_ref()
                    .is_some_and(|labels| labels.service == service.name)
            })
            .peekable();
        if members.peek().is_none() {
            rows.push(ServiceRow {
                service: &service.name,
                container: None,
                profiles: &service.profiles,
                build: service.build,
                defined: true,
                orphaned: false,
                running_replicas: replicas,
            });
        } else {
            rows.extend(members.map(|container| ServiceRow {
                service: &service.name,
                container: Some(container),
                profiles: &service.profiles,
                build: service.build,
                defined: true,
                orphaned: false,
                running_replicas: replicas,
            }));
        }
    }
    for container in &group.services {
        if defined.is_some_and(|services| {
            services.iter().any(|service| {
                container
                    .compose
                    .as_ref()
                    .is_some_and(|labels| labels.service == service.name)
            })
        }) {
            continue;
        }
        let service = container
            .compose
            .as_ref()
            .map_or(container.name.as_str(), |labels| labels.service.as_str());
        rows.push(ServiceRow {
            service,
            container: Some(container),
            profiles: &[],
            build: false,
            defined: false,
            orphaned: defined.is_some(),
            running_replicas: service_replicas(group, service),
        });
    }
    rows
}

/// Existing running, paused, or restarting service containers, excluding the
/// project's one-off jobs. Matches the Scale dialog's source count.
pub fn service_replicas(group: &ProjectGroup<'_>, service: &str) -> usize {
    group
        .services
        .iter()
        .filter(|container| {
            container
                .compose
                .as_ref()
                .is_some_and(|labels| labels.service == service)
                && is_running(container)
        })
        .count()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    #[default]
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    pub const fn tone(self) -> Tone {
        match self {
            Self::Running => Tone::Muted,
            Self::Succeeded => Tone::Success,
            Self::Failed | Self::Cancelled => Tone::Danger,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeJob {
    pub job: String,
    pub project: String,
    pub command: String,
    pub state: JobState,
    pub exit_code: Option<i64>,
    /// Epoch milliseconds.
    pub started_at: i64,
    /// Epoch milliseconds.
    pub finished_at: Option<i64>,
    /// The returned output suffix, or the retained contiguous suffix after
    /// client accumulation. Job listings carry an empty string here.
    pub output: String,
    /// Cumulative byte position immediately after `output`. Listings use zero.
    pub offset: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ComposeJobs {
    /// Agent insertion order, newest first. Preserve this order when filtering:
    /// wall-clock timestamps may tie or move backwards.
    pub jobs: Vec<ComposeJob>,
}

pub fn project_jobs<'a>(jobs: &'a [ComposeJob], project: &str) -> Vec<&'a ComposeJob> {
    jobs.iter().filter(|job| job.project == project).collect()
}

pub fn selected_job<'a>(
    jobs: &'a [ComposeJob],
    project: &str,
    chosen: Option<&str>,
) -> Option<&'a ComposeJob> {
    chosen
        .and_then(|chosen| {
            jobs.iter()
                .find(|job| job.project == project && job.job == chosen)
        })
        .or_else(|| jobs.iter().find(|job| job.project == project))
}

pub fn running_job<'a>(jobs: &'a [ComposeJob], project: &str) -> Option<&'a ComposeJob> {
    jobs.iter()
        .find(|job| job.project == project && job.state.is_running())
}

pub fn job_duration(job: &ComposeJob, now: i64) -> String {
    duration(
        job.finished_at
            .unwrap_or(now)
            .saturating_sub(job.started_at),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobStatus {
    pub label: String,
    pub tone: Tone,
}

/// The recorded state remains authoritative, including cancellation with no
/// exit code. Completed durations stop at `finished_at` instead of ticking.
pub fn job_status(job: &ComposeJob, now: i64) -> JobStatus {
    let elapsed = job_duration(job, now);
    let label = match job.state {
        JobState::Running => format!("Running · {elapsed}"),
        JobState::Succeeded => format!("Succeeded in {elapsed}"),
        JobState::Failed => format!(
            "Failed (exit {}) after {elapsed}",
            job.exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "?".into())
        ),
        JobState::Cancelled => format!("Cancelled after {elapsed}"),
    };
    JobStatus {
        label,
        tone: job.state.tone(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputLine<'a> {
    /// Absolute UTF-8 byte start. Pair this with the job ID for a row identity
    /// that survives prefix eviction and identical repeated messages.
    pub offset: u64,
    pub message: &'a str,
    pub severity: Severity,
}

/// Split a retained contiguous output suffix into lines, preserving blank lines
/// and omitting only the final empty split.
/// The agent already converts CR/CRLF progress and strips terminal escapes;
/// this layer does not reinterpret text or alter its cumulative byte offsets.
pub fn output_lines(output: &str, end_offset: u64) -> Result<Vec<OutputLine<'_>>, &'static str> {
    let mut offset = end_offset
        .checked_sub(output.len() as u64)
        .ok_or("Compose output exceeds its cumulative byte offset.")?;
    Ok(output
        .split_inclusive('\n')
        .map(|part| {
            let message = part.strip_suffix('\n').unwrap_or(part);
            let line = OutputLine {
                offset,
                message,
                severity: severity(message),
            };
            offset += part.len() as u64;
            line
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containers::{ComposeLabels, ContainerState, group_containers};
    use serde_json::{Value, json};

    fn job(id: &str, project: &str, state: JobState) -> ComposeJob {
        ComposeJob {
            job: id.into(),
            project: project.into(),
            command: "docker compose up -d".into(),
            state,
            started_at: 1000,
            ..ComposeJob::default()
        }
    }

    #[test]
    fn job_contract_preserves_wire_names_states_and_positional_order() {
        let value = json!({"job":"j1", "project":"app", "command":"docker compose up -d", "state":"failed", "exitCode":17, "startedAt":1000, "finishedAt":2500, "output":"failed\n", "offset":7});
        let typed: ComposeJob = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), value);
        let bytes = rmp_serde::to_vec(&typed).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<Value>(&bytes).unwrap(),
            json!([
                "j1",
                "app",
                "docker compose up -d",
                "failed",
                17,
                1000,
                2500,
                "failed\n",
                7
            ])
        );
        assert_eq!(rmp_serde::from_slice::<ComposeJob>(&bytes).unwrap(), typed);
        for state in [
            JobState::Running,
            JobState::Succeeded,
            JobState::Failed,
            JobState::Cancelled,
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), json!(state.as_str()));
        }
        let mut additive = value.clone();
        additive["newAgentField"] = json!({"allowed":true});
        assert_eq!(
            serde_json::from_value::<ComposeJob>(additive).unwrap(),
            typed
        );
        for required in [
            "job",
            "project",
            "command",
            "state",
            "startedAt",
            "output",
            "offset",
        ] {
            let mut invalid = value.clone();
            invalid.as_object_mut().unwrap().remove(required);
            assert!(
                serde_json::from_value::<ComposeJob>(invalid).is_err(),
                "{required}"
            );
        }
        let mut invalid = value;
        invalid["state"] = json!("unknown");
        assert!(serde_json::from_value::<ComposeJob>(invalid).is_err());
        assert!(serde_json::from_value::<ComposeJobs>(json!({})).is_err());
        assert_eq!(
            serde_json::from_value::<ComposeJobs>(json!({"jobs":[],"more":true}))
                .unwrap()
                .jobs,
            []
        );
    }

    #[test]
    fn project_contract_keeps_nullable_configuration_and_nested_fields() {
        let value = json!({"cli":{"kind":"podman","version":"1.2"},"cliError":null,"workingDir":"/srv/app","configFiles":["compose.yml"],"files":[{"path":"/srv/app/compose.yml","text":"services: {}\n","error":null}],"services":[{"name":"web","image":null,"build":true,"profiles":["dev"]}],"configError":null});
        let project: ComposeProject = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&project).unwrap(), value);
        let bytes = rmp_serde::to_vec(&project).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<Value>(&bytes).unwrap(),
            json!([
                ["podman", "1.2"],
                null,
                "/srv/app",
                ["compose.yml"],
                [["/srv/app/compose.yml", "services: {}\n", null]],
                [["web", null, true, ["dev"]]],
                null
            ])
        );
        let unavailable: ComposeProject = serde_json::from_value(json!({"cli":null,"cliError":"not installed","workingDir":null,"configFiles":[],"files":[],"services":null,"configError":"cannot read"})).unwrap();
        assert!(unavailable.cli.is_none());
        assert!(unavailable.services.is_none());
        assert_eq!(unavailable.config_error.as_deref(), Some("cannot read"));
        assert!(serde_json::from_value::<ComposeProject>(json!({})).is_err());
    }

    #[test]
    fn project_selection_preserves_agent_order_and_rejects_another_projects_choice() {
        let mut jobs = vec![
            job("other", "db", JobState::Running),
            job("new", "app", JobState::Succeeded),
            job("old", "app", JobState::Running),
        ];
        jobs[1].started_at = 1;
        jobs[2].started_at = 9000;
        assert_eq!(
            project_jobs(&jobs, "app")
                .iter()
                .map(|row| row.job.as_str())
                .collect::<Vec<_>>(),
            ["new", "old"]
        );
        assert_eq!(selected_job(&jobs, "app", None).unwrap().job, "new");
        assert_eq!(selected_job(&jobs, "app", Some("old")).unwrap().job, "old");
        for chosen in ["other", "expired"] {
            assert_eq!(selected_job(&jobs, "app", Some(chosen)).unwrap().job, "new");
        }
        assert_eq!(running_job(&jobs, "app").unwrap().job, "old");
        assert!(selected_job(&jobs, "missing", None).is_none());
        assert!(running_job(&[], "app").is_none());
    }

    #[test]
    fn status_uses_recorded_outcome_exit_code_and_finished_time() {
        let mut row = job("j1", "app", JobState::Running);
        assert_eq!(
            job_status(&row, 2499),
            JobStatus {
                label: "Running · 1s".into(),
                tone: Tone::Muted
            }
        );
        assert_eq!(job_duration(&row, 2500), "2s");
        assert_eq!(job_duration(&row, 0), "0s");
        row.finished_at = Some(2500);
        for (state, code, label, tone) in [
            (
                JobState::Succeeded,
                Some(0),
                "Succeeded in 2s",
                Tone::Success,
            ),
            (
                JobState::Failed,
                Some(17),
                "Failed (exit 17) after 2s",
                Tone::Danger,
            ),
            (
                JobState::Failed,
                None,
                "Failed (exit ?) after 2s",
                Tone::Danger,
            ),
            (
                JobState::Cancelled,
                Some(130),
                "Cancelled after 2s",
                Tone::Danger,
            ),
            (
                JobState::Cancelled,
                None,
                "Cancelled after 2s",
                Tone::Danger,
            ),
            (
                JobState::Failed,
                Some(0),
                "Failed (exit 0) after 2s",
                Tone::Danger,
            ),
        ] {
            row.state = state;
            row.exit_code = code;
            assert_eq!(
                job_status(&row, 100_000),
                JobStatus {
                    label: label.into(),
                    tone
                }
            );
        }
        row.finished_at = Some(1000 + 90_000_000);
        assert_eq!(job_duration(&row, 0), "1d 1h");
        row.started_at = i64::MIN;
        row.finished_at = Some(i64::MAX);
        assert!(!job_duration(&row, 0).is_empty());
    }

    #[test]
    fn output_lines_keep_blank_unterminated_and_cleaned_terminal_progress_lines() {
        assert!(output_lines("", 0).unwrap().is_empty());
        let text = "green\nprogress 1\nprogress 2\n\nlasté";
        let lines = output_lines(text, text.len() as u64).unwrap();
        assert_eq!(
            lines.iter().map(|line| line.message).collect::<Vec<_>>(),
            ["green", "progress 1", "progress 2", "", "lasté"]
        );
        assert_eq!(
            output_lines("\n\n", 2)
                .unwrap()
                .iter()
                .map(|line| line.offset)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(output_lines("line\n", 5).unwrap().len(), 1);
        // A raw CR, if returned by a different agent, stays message content just
        // as it does in the source viewer; it never silently changes offsets.
        let raw = output_lines("one\rtwo\r\n", 9).unwrap();
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].message, "one\rtwo\r");
        let levels = output_lines("warning\nfatal\nready", 19).unwrap();
        assert_eq!(
            levels.iter().map(|line| line.severity).collect::<Vec<_>>(),
            [Severity::Warning, Severity::Error, Severity::Normal]
        );
    }

    #[test]
    fn byte_offsets_stay_stable_across_utf8_suffix_eviction_and_repeated_lines() {
        let text = "é\nsame\nsame\n東京";
        let end = text.len() as u64;
        let lines = output_lines(text, end).unwrap();
        assert_eq!(
            lines.iter().map(|line| line.offset).collect::<Vec<_>>(),
            [0, 3, 8, 13]
        );
        let suffix = output_lines(&text[3..], end).unwrap();
        assert_eq!(&lines[1..], suffix);
        let bounded = output_lines("京", end).unwrap();
        assert_eq!(bounded[0].offset, 16);
        assert_eq!(bounded[0].message, "京");
        assert!(output_lines("é", 1).is_err());
        let at_limit = output_lines("é\n", u64::MAX).unwrap();
        assert_eq!(at_limit[0].offset, u64::MAX - 3);
    }

    fn container(service: &str, number: u64, state: ContainerState, oneoff: bool) -> Container {
        Container {
            id: format!("{service}-{number}"),
            name: format!("app-{service}-{number}"),
            state,
            compose: Some(ComposeLabels {
                project: "app".into(),
                service: service.into(),
                number: Some(number),
                oneoff,
                ..ComposeLabels::default()
            }),
            ..Container::default()
        }
    }

    #[test]
    fn service_rows_follow_definition_then_replica_order_and_append_orphans() {
        let containers = vec![
            container("web", 2, ContainerState::Running, false),
            container("legacy", 1, ContainerState::Exited, false),
            container("db", 1, ContainerState::Running, false),
            container("web", 1, ContainerState::Running, false),
        ];
        let groups = group_containers(&containers);
        let group = &groups.projects[0];
        let project = ComposeProject {
            services: Some(vec![
                ComposeService {
                    name: "db".into(),
                    ..ComposeService::default()
                },
                ComposeService {
                    name: "worker".into(),
                    profiles: vec!["background".into(), "dev".into()],
                    ..ComposeService::default()
                },
                ComposeService {
                    name: "web".into(),
                    build: true,
                    ..ComposeService::default()
                },
            ]),
            ..ComposeProject::default()
        };
        let rows = service_rows(group, Some(&project));
        assert_eq!(
            rows.iter().map(|row| row.service).collect::<Vec<_>>(),
            ["db", "worker", "web", "web", "legacy"]
        );
        assert!(rows[1].container.is_none());
        assert_eq!(rows[1].profiles, ["background", "dev"]);
        assert_eq!(rows[1].running_replicas, 0);
        assert_eq!(rows[2].container.unwrap().id, "web-1");
        assert_eq!(rows[3].container.unwrap().id, "web-2");
        assert_eq!(rows[2].running_replicas, 2);
        assert!(rows[2].build);
        assert!(rows[2].defined);
        assert!(!rows[2].orphaned);
        assert!(rows[4].orphaned);
        assert!(!rows[4].defined);
        assert!(!rows[4].build);
    }

    #[test]
    fn service_rows_exclude_oneoffs_and_count_paused_and_restarting_replicas() {
        let containers = vec![
            container("web", 1, ContainerState::Running, false),
            container("web", 2, ContainerState::Paused, false),
            container("web", 3, ContainerState::Restarting, false),
            container("web", 4, ContainerState::Exited, false),
            container("web", 5, ContainerState::Created, false),
            container("web", 6, ContainerState::Running, true),
            container("migrate", 1, ContainerState::Running, true),
        ];
        let groups = group_containers(&containers);
        let group = &groups.projects[0];
        let project = ComposeProject {
            services: Some(vec![
                ComposeService {
                    name: "web".into(),
                    ..ComposeService::default()
                },
                ComposeService {
                    name: "migrate".into(),
                    profiles: vec!["tools".into()],
                    ..ComposeService::default()
                },
            ]),
            ..ComposeProject::default()
        };
        let rows = service_rows(group, Some(&project));
        assert_eq!(rows.len(), 6);
        assert_eq!(service_replicas(group, "web"), 3);
        assert_eq!(service_replicas(group, "migrate"), 0);
        assert!(
            rows.iter()
                .filter_map(|row| row.container)
                .all(|container| !container.compose.as_ref().unwrap().oneoff)
        );
        assert_eq!(rows[5].service, "migrate");
        assert!(rows[5].container.is_none());
        assert_eq!(rows[5].profiles, ["tools"]);
    }

    #[test]
    fn unavailable_definitions_keep_existing_services_without_claiming_orphans() {
        let containers = vec![container("web", 1, ContainerState::Running, false)];
        let groups = group_containers(&containers);
        let group = &groups.projects[0];
        for project in [None, Some(&ComposeProject::default())] {
            let rows = service_rows(group, project);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].service, "web");
            assert!(!rows[0].defined);
            assert!(!rows[0].orphaned);
            assert_eq!(rows[0].running_replicas, 1);
        }
        let empty = ComposeProject {
            services: Some(vec![]),
            ..ComposeProject::default()
        };
        assert!(service_rows(group, Some(&empty))[0].orphaned);
    }
}
