use crate::{model::Server, monitoring, ssh};
use serde::Deserialize;
use serde_json::Value;

const CONTAINERS_COMMAND: &str = r#"docker ps -a --no-trunc --format '{"id":{{json .ID}},"name":{{json .Names}},"image":{{json .Image}},"state":{{json .State}},"status":{{json .Status}},"ports":{{json .Ports}},"composeProject":{{json (.Label "com.docker.compose.project")}},"composeService":{{json (.Label "com.docker.compose.service")}},"composeOneoff":{{json (.Label "com.docker.compose.oneoff")}}}'"#;

static REQUESTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Section {
    Overview,
    Services,
    Containers,
}

pub async fn collect(server: &Server, section: Section) -> Result<Value, String> {
    let _permit = if matches!(section, Section::Overview) {
        REQUESTS.acquire().await.map_err(|_| "Sampler stopped")?
    } else {
        REQUESTS
            .try_acquire()
            .map_err(|_| "Cockpit is busy. Try again shortly.")?
    };
    match section {
        Section::Overview => {
            let output = ssh::execute(server, "sh -s", Some(include_str!("cockpit.sh"))).await?;
            serde_json::to_value(monitoring::parse_overview(&output)?).map_err(|e| e.to_string())
        }
        Section::Services => {
            let output = ssh::execute(server, "LC_ALL=C SYSTEMD_COLORS=0 systemctl list-units --type=service --all --no-pager --plain --no-legend --full", None).await?;
            serde_json::to_value(monitoring::parse_services(&output)).map_err(|e| e.to_string())
        }
        Section::Containers => {
            let output = ssh::execute(server, CONTAINERS_COMMAND, None).await?;
            let rows = parse_containers(&output)?;
            Ok(Value::Array(rows))
        }
    }
}

fn parse_containers(output: &str) -> Result<Vec<Value>, String> {
    monitoring::parse_containers(output)?
        .into_iter()
        .map(|container| serde_json::to_value(container).map_err(|e| e.to_string()))
        .collect()
}

fn valid_project(target: &str) -> bool {
    !target.is_empty()
        && target.len() <= 256
        && target.as_bytes()[0].is_ascii_alphanumeric()
        && target
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-".contains(&c))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogSource {
    Journal,
    Service,
    Container,
    Compose,
}

fn log_command(source: LogSource, target: &str) -> Result<String, String> {
    match source {
        LogSource::Journal => Ok("journalctl --no-pager --quiet -n 200 -o short-iso 2>&1".into()),
        LogSource::Service => {
            if target.len() > 256
                || !target.ends_with(".service")
                || target.starts_with('-')
                || !target
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.@:-\\".contains(&c))
            {
                return Err("Select a valid service to view its logs.".into());
            }
            Ok(format!(
                "journalctl --no-pager --quiet -n 200 -o short-iso --unit='{target}' 2>&1"
            ))
        }
        LogSource::Compose => {
            if !valid_project(target) {
                return Err("Select a valid Compose project.".into());
            }
            Ok(format!(
                r#"sh -c '
rows=$(docker ps -a --no-trunc --filter "label=com.docker.compose.project={target}" --format "{{{{.ID}}}} {{{{.Names}}}}") || exit $?
[ -n "$rows" ] || {{ printf "No containers remain in this project.\n"; exit 0; }}
printf "%s\n" "$rows" | head -20 | while read -r id name; do
  case "$id" in *[!0-9a-f]*|"") printf "Invalid container ID\n" >&2; exit 1;; esac
  printf "\n--- %s ---\n" "$name"
  docker logs --tail 100 --timestamps "$id" 2>&1 || printf "Logs unavailable for %s\n" "$name"
done'"#
            ))
        }
        LogSource::Container => {
            if !(12..=64).contains(&target.len()) || !target.bytes().all(|c| c.is_ascii_hexdigit())
            {
                return Err("Select a valid container to view its logs.".into());
            }
            Ok(format!("docker logs --tail 200 --timestamps {target} 2>&1"))
        }
    }
}

pub async fn logs(server: &Server, source: LogSource, target: &str) -> Result<String, String> {
    let command = log_command(source, target)?;
    let _permit = REQUESTS
        .try_acquire()
        .map_err(|_| "Cockpit is busy. Try again shortly.")?;
    ssh::execute(server, &command, None).await
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectAction {
    Start,
    Stop,
    Restart,
}
fn project_action_command(
    project: &str,
    action: ProjectAction,
    expected: &[String],
    containers: &[Value],
) -> Result<String, String> {
    if !valid_project(project) {
        return Err("Select a valid Compose project.".into());
    }
    let mut ids = containers
        .iter()
        .filter(|c| {
            c["composeProject"] == project
                && !c["composeOneoff"]
                    .as_str()
                    .unwrap_or("")
                    .eq_ignore_ascii_case("true")
        })
        .map(|c| c["id"].as_str().unwrap_or("").to_owned())
        .collect::<Vec<_>>();
    ids.sort();
    let mut expected = expected.to_vec();
    expected.sort();
    if ids.is_empty() {
        return Err(
            "No existing service containers remain in this project. Refresh the list.".into(),
        );
    }
    if ids != expected {
        return Err("Project containers changed since you opened the confirmation. Refresh and review the project again.".into());
    }
    if ids.len() > monitoring::MAX_COLLECTION_ROWS
        || ids
            .iter()
            .any(|id| id.len() != 64 || !id.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return Err("Invalid project container list.".into());
    }
    let verb = match action {
        ProjectAction::Start => "start",
        ProjectAction::Stop => "stop",
        ProjectAction::Restart => "restart",
    };
    Ok(format!("docker container {verb} {}", ids.join(" ")))
}
pub async fn project_action(
    server: &Server,
    project: &str,
    action: ProjectAction,
    expected: Vec<String>,
) -> Result<(), String> {
    if !valid_project(project) {
        return Err("Select a valid Compose project.".into());
    }
    let _permit = REQUESTS
        .try_acquire()
        .map_err(|_| "Cockpit is busy. Try again shortly.")?;
    let output = ssh::execute(
        server,
        &format!("{CONTAINERS_COMMAND} --filter 'label=com.docker.compose.project={project}'"),
        None,
    )
    .await?;
    if output.lines().count() > monitoring::MAX_COLLECTION_ROWS {
        return Err("Project exceeds the 500-container control limit.".into());
    }
    let containers = parse_containers(&output)?;
    let command = project_action_command(project, action, &expected, &containers)?;
    ssh::execute(server, &command, None)
        .await
        .map(|_| ())
        .map_err(|error| {
            format!("{error}. The action may have partially completed; refresh the project before retrying.")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_actions_only_target_confirmed_non_oneoff_members() {
        let id = "a".repeat(64);
        let job = "b".repeat(64);
        let rows = vec![
            serde_json::json!({"id":id,"composeProject":"app","composeOneoff":"False"}),
            serde_json::json!({"id":job,"composeProject":"app","composeOneoff":"True"}),
        ];
        assert_eq!(
            project_action_command("app", ProjectAction::Stop, std::slice::from_ref(&id), &rows)
                .unwrap(),
            format!("docker container stop {id}")
        );
        assert!(project_action_command("app", ProjectAction::Start, &[job], &rows).is_err());
        assert!(project_action_command("other", ProjectAction::Restart, &[id], &rows).is_err());
    }
    #[test]
    fn compose_labels_are_structured_and_project_targets_validated() {
        let rows = parse_containers(r#"{"id":"abc","name":"api","image":"image","state":"running","status":"Up","ports":"","composeProject":"my-app","composeService":"api","composeOneoff":"False"}"#).unwrap();
        assert_eq!(rows[0]["composeProject"], "my-app");
        assert!(parse_containers("{}").is_err());
        for invalid in ["", "-bad", "a;id", "a$(id)", "a\nb", "UPPER"] {
            assert!(log_command(LogSource::Compose, invalid).is_err());
        }
        assert!(log_command(LogSource::Compose, "my-app_1").is_ok());
    }
    #[test]
    fn log_targets_cannot_inject_commands() {
        for target in [
            "a';touch /tmp/oops;.service",
            "$(id).service",
            "-evil.service",
            "a\n.service",
        ] {
            assert!(log_command(LogSource::Service, target).is_err());
        }
        assert!(log_command(LogSource::Service, "getty@tty1.service").is_ok());
        assert!(log_command(LogSource::Container, "abc;uname -a").is_err());
        assert!(log_command(LogSource::Container, "a".repeat(64).as_str()).is_ok());
    }
}
