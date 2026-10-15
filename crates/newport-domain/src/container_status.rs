//! Readable container lifecycle and failure context, shared by desktop clients.
use crate::containers::{
    problem_reason, Container, ContainerHealth, ContainerInspect, ContainerState, ContainerVerb,
};

pub fn action_progress(verb: ContainerVerb) -> &'static str {
    match verb {
        ContainerVerb::Start => "Starting…",
        ContainerVerb::Stop => "Stopping…",
        ContainerVerb::Restart => "Restarting…",
        ContainerVerb::Pause => "Pausing…",
        ContainerVerb::Unpause => "Resuming…",
    }
}

/// Lifecycle context uses the observed snapshot time, never a negative uptime.
pub fn lifecycle_context(container: &Container, now: i64) -> Option<String> {
    let running = container.state == ContainerState::Running;
    let since = if running {
        container.started_at
    } else {
        container.finished_at
    }?;
    let duration = crate::services::duration(now.saturating_sub(since));
    Some(if running {
        format!("for {duration}")
    } else {
        format!(
            "since {duration} ago{}",
            container
                .exit_code
                .map(|code| format!(" · exit code {code}"))
                .unwrap_or_default()
        )
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProblemContext {
    pub reason: String,
    pub restarts: Option<String>,
    pub health_output: Option<String>,
}

/// Docker retains health history after stops and restarts. Present it only for
/// the current running, unhealthy container, with an exact inspection identity.
pub fn problem_context(
    container: &Container,
    inspect: Option<&ContainerInspect>,
) -> Option<ProblemContext> {
    let reason = problem_reason(container)?;
    let restarts = (container.restart_count > 0 && container.state != ContainerState::Restarting)
        .then(|| format!("Restarted {}× so far.", container.restart_count));
    let health_output = inspect
        .filter(|inspect| inspect.id == container.id)
        .filter(|_| {
            container.state == ContainerState::Running
                && container.health == Some(ContainerHealth::Unhealthy)
        })
        .and_then(|inspect| {
            inspect
                .last_health
                .iter()
                .rev()
                .find(|check| check.exit_code != 0)
        })
        .map(|check| {
            let output = check.output.trim();
            if output.is_empty() {
                format!("check exited {}", check.exit_code)
            } else {
                output.to_owned()
            }
        });
    Some(ProblemContext {
        reason,
        restarts,
        health_output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::containers::HealthResult;

    #[test]
    fn lifecycle_distinguishes_exact_running_state_and_observed_exit() {
        let mut c = Container {
            state: ContainerState::Running,
            started_at: Some(1_000),
            finished_at: Some(61_000),
            exit_code: Some(137),
            ..Default::default()
        };
        assert_eq!(lifecycle_context(&c, 91_000).as_deref(), Some("for 1m"));
        assert_eq!(lifecycle_context(&c, 0).as_deref(), Some("for 0s"));
        c.state = ContainerState::Exited;
        assert_eq!(
            lifecycle_context(&c, 91_000).as_deref(),
            Some("since 30s ago · exit code 137")
        );
        c.state = ContainerState::Paused;
        c.finished_at = None;
        assert!(lifecycle_context(&c, 91_000).is_none());
    }

    #[test]
    fn latest_failed_health_check_uses_exact_identity_and_empty_output_fallback() {
        let c = Container {
            id: "current".into(),
            state: ContainerState::Running,
            health: Some(ContainerHealth::Unhealthy),
            restart_count: 3,
            ..Default::default()
        };
        let mut inspect = ContainerInspect {
            id: c.id.clone(),
            last_health: vec![
                HealthResult {
                    exit_code: 1,
                    output: " old failure ".into(),
                    ..Default::default()
                },
                HealthResult {
                    exit_code: 2,
                    output: " \n\t ".into(),
                    ..Default::default()
                },
                HealthResult {
                    exit_code: 0,
                    output: "success".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let problem = problem_context(&c, Some(&inspect)).unwrap();
        assert_eq!(problem.health_output.as_deref(), Some("check exited 2"));
        assert_eq!(problem.restarts.as_deref(), Some("Restarted 3× so far."));
        inspect.last_health[1].output = "  connection refused\n".into();
        assert_eq!(
            problem_context(&c, Some(&inspect))
                .unwrap()
                .health_output
                .as_deref(),
            Some("connection refused")
        );
        inspect.id = "previous-incarnation".into();
        assert!(problem_context(&c, Some(&inspect))
            .unwrap()
            .health_output
            .is_none());
    }

    #[test]
    fn stale_stopped_health_and_restart_counts_do_not_misdiagnose_the_problem() {
        let mut c = Container {
            id: "current".into(),
            state: ContainerState::Exited,
            health: Some(ContainerHealth::Unhealthy),
            exit_code: Some(1),
            restart_count: 2,
            ..Default::default()
        };
        let inspect = ContainerInspect {
            id: c.id.clone(),
            last_health: vec![HealthResult {
                exit_code: 1,
                output: "old health failure".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(problem_context(&c, Some(&inspect))
            .unwrap()
            .health_output
            .is_none());
        c.state = ContainerState::Restarting;
        let problem = problem_context(&c, Some(&inspect)).unwrap();
        assert_eq!(problem.reason, "Keeps exiting; restarted 2×");
        assert!(problem.restarts.is_none());
    }
}
