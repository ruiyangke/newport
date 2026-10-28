//! Single-use command handovers for a fresh terminal's initial connection.
//! Service/permission commands remain review-only. Only an app-generated
//! container shell or project directory opts into execution, and neither mode survives connection loss.
#[derive(Debug, PartialEq, Eq)]
pub enum CommandDelivery {
    Review(String),
    ContainerShell(String),
    ProjectDirectory(String),
}

#[derive(Default)]
pub struct CommandDraft(Option<CommandDelivery>);

impl CommandDraft {
    /// Prefill a command without executing it; the user supplies Enter.
    pub fn new(command: String) -> Result<Self, String> {
        validate(&command)?;
        Ok(Self(Some(CommandDelivery::Review(command))))
    }

    /// Launch the already-validated, app-generated shell command in a NEW tab.
    /// The terminal must consume it only on the initial Ready event.
    pub fn container_shell(command: String) -> Result<Self, String> {
        validate(&command)?;
        Ok(Self(Some(CommandDelivery::ContainerShell(command))))
    }

    /// Enter an app-selected worktree in a fresh shell. Never replay into a running program.
    pub fn project_directory(path: &str) -> Result<Self, String> {
        if !path.starts_with('/') {
            return Err("The project directory must be absolute.".into());
        }
        validate(path)?;
        let quoted = path.replace('\'', "'\"'\"'");
        Ok(Self(Some(CommandDelivery::ProjectDirectory(format!(
            "cd -- '{quoted}'"
        )))))
    }

    pub fn take(&mut self) -> Option<CommandDelivery> {
        self.0.take()
    }

    pub fn discard(&mut self) {
        self.0 = None;
    }
}

fn validate(command: &str) -> Result<(), String> {
    if command.trim().is_empty() || command.len() > 16 * 1024 {
        return Err("The terminal command is empty or too long.".into());
    }
    if command.chars().any(char::is_control) {
        return Err("The terminal command contains unsupported control characters.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_directory_is_quoted_and_delivered_only_once() {
        let mut draft = CommandDraft::project_directory("/tmp/it's $(touch bad)").unwrap();
        assert_eq!(
            draft.take(),
            Some(CommandDelivery::ProjectDirectory(
                "cd -- '/tmp/it'\"'\"'s $(touch bad)'".into()
            ))
        );
        assert_eq!(draft.take(), None);
        let mut canceled = CommandDraft::project_directory("/project").unwrap();
        canceled.discard();
        assert_eq!(canceled.take(), None);
        for path in ["relative", "/tmp/a\nb", "/tmp/a\0b"] {
            assert!(CommandDraft::project_directory(path).is_err());
        }
    }

    #[test]
    fn service_draft_is_delivered_once_for_review_only() {
        let command = "sudo systemctl restart 'web.service'";
        let mut draft = CommandDraft::new(command.into()).unwrap();
        assert_eq!(draft.take(), Some(CommandDelivery::Review(command.into())));
        assert_eq!(draft.take(), None);
    }

    #[test]
    fn only_explicit_container_shell_mode_requests_execution_once() {
        let command = "docker exec -it -- abcdef sh";
        let mut draft = CommandDraft::container_shell(command.into()).unwrap();
        assert_eq!(
            draft.take(),
            Some(CommandDelivery::ContainerShell(command.into()))
        );
        assert_eq!(draft.take(), None);
    }

    #[test]
    fn connection_loss_discards_both_modes() {
        for mut draft in [
            CommandDraft::new("sudo systemctl stop web.service".into()).unwrap(),
            CommandDraft::container_shell("docker exec -it -- abcdef sh".into()).unwrap(),
        ] {
            draft.discard();
            assert_eq!(draft.take(), None);
        }
    }

    #[test]
    fn neither_mode_accepts_injected_enter_terminal_escape_or_unbounded_input() {
        for command in [
            "",
            " ",
            "echo a\necho b",
            "echo a\r",
            "\u{1b}[200~",
            "a\0b",
            "a\u{85}b",
        ] {
            assert!(CommandDraft::new(command.into()).is_err());
            assert!(CommandDraft::container_shell(command.into()).is_err());
        }
        for constructor in [CommandDraft::new, CommandDraft::container_shell] {
            assert!(constructor("a".repeat(16 * 1024 + 1)).is_err());
            assert!(constructor("echo '日本語'".into()).is_ok());
        }
    }
}
