//! Compose Activity and service rules over the protocol's Compose types,
//! shared by clients. Job offsets are cumulative UTF-8 byte positions in the
//! agent's cleaned command output.
use crate::{
    containers::{ProjectGroup, Tone, is_running},
    containers_log::{Severity, severity},
    services::duration,
};
use newport_protocol::monitor::{
    compose::{Job, Project, State},
    containers::Container,
};

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
    project: Option<&'a Project>,
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

pub const fn job_tone(state: State) -> Tone {
    match state {
        State::Running => Tone::Muted,
        State::Succeeded => Tone::Success,
        State::Failed | State::Cancelled => Tone::Danger,
    }
}

pub fn project_jobs<'a>(jobs: &'a [Job], project: &str) -> Vec<&'a Job> {
    jobs.iter().filter(|job| job.project == project).collect()
}

pub fn selected_job<'a>(jobs: &'a [Job], project: &str, chosen: Option<&str>) -> Option<&'a Job> {
    chosen
        .and_then(|chosen| {
            jobs.iter()
                .find(|job| job.project == project && job.job == chosen)
        })
        .or_else(|| jobs.iter().find(|job| job.project == project))
}

pub fn running_job<'a>(jobs: &'a [Job], project: &str) -> Option<&'a Job> {
    jobs.iter()
        .find(|job| job.project == project && job.state.is_running())
}

pub fn job_duration(job: &Job, now: i64) -> String {
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
pub fn job_status(job: &Job, now: i64) -> JobStatus {
    let elapsed = job_duration(job, now);
    let label = match job.state {
        State::Running => format!("Running · {elapsed}"),
        State::Succeeded => format!("Succeeded in {elapsed}"),
        State::Failed => format!(
            "Failed (exit {}) after {elapsed}",
            job.exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "?".into())
        ),
        State::Cancelled => format!("Cancelled after {elapsed}"),
    };
    JobStatus {
        label,
        tone: job_tone(job.state),
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
    use crate::containers::group_containers;
    use newport_protocol::monitor::containers::{ComposeLabels, ContainerState};

    fn job(id: &str, project: &str, state: State) -> Job {
        Job {
            job: id.into(),
            project: project.into(),
            command: "docker compose up -d".into(),
            state,
            started_at: 1000,
            ..Job::default()
        }
    }

    #[test]
    fn project_selection_preserves_agent_order_and_rejects_another_projects_choice() {
        let mut jobs = vec![
            job("other", "db", State::Running),
            job("new", "app", State::Succeeded),
            job("old", "app", State::Running),
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
        let mut row = job("j1", "app", State::Running);
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
            (State::Succeeded, Some(0), "Succeeded in 2s", Tone::Success),
            (
                State::Failed,
                Some(17),
                "Failed (exit 17) after 2s",
                Tone::Danger,
            ),
            (
                State::Failed,
                None,
                "Failed (exit ?) after 2s",
                Tone::Danger,
            ),
            (
                State::Cancelled,
                Some(130),
                "Cancelled after 2s",
                Tone::Danger,
            ),
            (State::Cancelled, None, "Cancelled after 2s", Tone::Danger),
            (
                State::Failed,
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
        let project = Project {
            services: Some(vec![
                newport_protocol::monitor::compose::Service {
                    name: "db".into(),
                    ..newport_protocol::monitor::compose::Service::default()
                },
                newport_protocol::monitor::compose::Service {
                    name: "worker".into(),
                    profiles: vec!["background".into(), "dev".into()],
                    ..newport_protocol::monitor::compose::Service::default()
                },
                newport_protocol::monitor::compose::Service {
                    name: "web".into(),
                    build: true,
                    ..newport_protocol::monitor::compose::Service::default()
                },
            ]),
            ..Project::default()
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
        let project = Project {
            services: Some(vec![
                newport_protocol::monitor::compose::Service {
                    name: "web".into(),
                    ..newport_protocol::monitor::compose::Service::default()
                },
                newport_protocol::monitor::compose::Service {
                    name: "migrate".into(),
                    profiles: vec!["tools".into()],
                    ..newport_protocol::monitor::compose::Service::default()
                },
            ]),
            ..Project::default()
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
        for project in [None, Some(&Project::default())] {
            let rows = service_rows(group, project);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].service, "web");
            assert!(!rows[0].defined);
            assert!(!rows[0].orphaned);
            assert_eq!(rows[0].running_replicas, 1);
        }
        let empty = Project {
            services: Some(vec![]),
            ..Project::default()
        };
        assert!(service_rows(group, Some(&empty))[0].orphaned);
    }
}
