//! What programs say they are doing, through OSC 7501, the Program Status
//! Protocol Claude Code speaks. A program first asks `OSC 7501 ; ?` and
//! reports only to a terminal that answers. Each report is a `:`-separated
//! list of `key=value` fields and replaces one entry: the program's own, or,
//! with `id`, one of its background tasks. `title` and `msg` are base64.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{collections::BTreeMap, fmt::Write};

/// The OSC number of the protocol.
pub(crate) const CODE: &[u8] = b"7501";
/// Reports longer than this are ignored: they would only be cut down.
pub(crate) const MAX_REPORT: usize = 8 * 1024;
/// Bounds the tasks one program can make the terminal keep.
const MAX_TASKS: usize = 64;
const MAX_ID: usize = 32;
const MAX_APP: usize = 64;
const MAX_TITLE: usize = 200;
const MAX_MESSAGE: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProgramState {
    Working,
    /// Waiting for the person at the terminal.
    Blocked,
    /// Finished what it was asked to do.
    Done,
    Idle,
    Error,
}
impl ProgramState {
    fn name(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Idle => "idle",
            Self::Error => "error",
        }
    }
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            "idle" => Self::Idle,
            "error" => Self::Error,
            _ => return None,
        })
    }
}

/// What a blocked program waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockedOn {
    Permission,
    Question,
    SignIn,
}
impl BlockedOn {
    fn name(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::SignIn => "auth",
        }
    }
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "permission" => Self::Permission,
            "question" => Self::Question,
            "auth" => Self::SignIn,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProgramStatus {
    /// The reporting program, such as `claude-code`.
    pub app: Option<String>,
    pub state: ProgramState,
    pub blocked_on: Option<BlockedOn>,
    /// Percent of its plan done, while working or blocked.
    pub progress: Option<u8>,
    pub title: Option<String>,
    /// What it is doing or waiting for, in its own words.
    pub message: Option<String>,
}

/// The entries programs in one terminal have reported.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ProgramStatuses {
    program: Option<ProgramStatus>,
    tasks: BTreeMap<String, ProgramStatus>,
}
impl ProgramStatuses {
    /// The foreground program's own entry.
    pub fn program(&self) -> Option<&ProgramStatus> {
        self.program.as_ref()
    }
    /// Its background tasks, by id.
    pub fn tasks(&self) -> impl Iterator<Item = (&str, &ProgramStatus)> {
        self.tasks.iter().map(|(id, status)| (id.as_str(), status))
    }
    pub fn is_empty(&self) -> bool {
        self.program.is_none() && self.tasks.is_empty()
    }
    pub(crate) fn clear(&mut self) -> bool {
        let changed = !self.is_empty();
        *self = Self::default();
        changed
    }

    /// Applies one report, the text after `7501;`. Returns whether anything
    /// changed. Malformed reports and unknown states are ignored, so a newer
    /// program's vocabulary leaves the entries as they were.
    pub(crate) fn apply(&mut self, report: &[u8]) -> bool {
        let Some(report) = Report::parse(report) else {
            return false;
        };
        match (report.id, report.status) {
            (None, None) => self.clear(),
            (None, Some(status)) => {
                let changed = self.program.as_ref() != Some(&status);
                self.program = Some(status);
                changed
            }
            (Some(id), None) => self.tasks.remove(&id).is_some(),
            (Some(id), Some(status)) => {
                if !self.tasks.contains_key(&id) && self.tasks.len() >= MAX_TASKS {
                    return false;
                }
                self.tasks.insert(id, status.clone()) != Some(status)
            }
        }
    }

    /// The program's own entry or a task's, whichever most needs the
    /// person; the program's own wins a tie.
    pub fn most_urgent(&self) -> Option<&ProgramStatus> {
        let urgency = |state: ProgramState| match state {
            ProgramState::Blocked => 0,
            ProgramState::Error => 1,
            ProgramState::Working => 2,
            ProgramState::Done => 3,
            ProgramState::Idle => 4,
        };
        self.program
            .iter()
            .chain(self.tasks.values())
            .min_by_key(|status| urgency(status.state))
    }

    /// The reports that bring a fresh terminal to these entries.
    pub(crate) fn write(&self, out: &mut String) {
        let entries = self
            .program
            .iter()
            .map(|status| (None, status))
            .chain(self.tasks.iter().map(|(id, status)| (Some(id), status)));
        for (id, status) in entries {
            out.push_str("\x1b]7501;");
            status.write_report(out, id.map(String::as_str));
            out.push_str("\x1b\\");
        }
    }
}

impl ProgramStatus {
    /// The entry as a program reports it, the text after `7501;`, so it can
    /// travel elsewhere, such as in a held shell's record.
    pub fn report(&self) -> String {
        let mut out = String::new();
        self.write_report(&mut out, None);
        out
    }
    /// An entry from [`ProgramStatus::report`]'s text, checked as a
    /// program's own report is. A task's id is dropped.
    pub fn from_report(report: &str) -> Option<Self> {
        Report::parse(report.as_bytes())?.status
    }
    fn write_report(&self, out: &mut String, id: Option<&str>) {
        let _ = write!(out, "state={}", self.state.name());
        if let Some(app) = &self.app {
            let _ = write!(out, ":app={app}");
        }
        if let Some(id) = id {
            let _ = write!(out, ":id={id}");
        }
        if let Some(kind) = self.blocked_on {
            let _ = write!(out, ":kind={}", kind.name());
        }
        if let Some(progress) = self.progress {
            let _ = write!(out, ":progress={progress}");
        }
        if let Some(title) = &self.title {
            let _ = write!(out, ":title={}", STANDARD.encode(title));
        }
        if let Some(message) = &self.message {
            let _ = write!(out, ":msg={}", STANDARD.encode(message));
        }
    }
}

struct Report {
    id: Option<String>,
    /// `None` clears the entry.
    status: Option<ProgramStatus>,
}
impl Report {
    fn parse(report: &[u8]) -> Option<Self> {
        if report.len() > MAX_REPORT {
            return None;
        }
        let report = std::str::from_utf8(report).ok()?;
        let mut state = None;
        let mut clear = false;
        let mut id = None;
        let mut app = None;
        let mut blocked_on = None;
        let mut progress = None;
        let mut title = None;
        let mut message = None;
        for field in report.split(':') {
            let (key, value) = field.split_once('=').unwrap_or((field, ""));
            match key {
                "state" if value == "clear" => clear = true,
                "state" => state = Some(ProgramState::parse(value)?),
                "id" => id = Some(token(value, MAX_ID)?),
                "app" => app = Some(token(value, MAX_APP)?),
                "kind" => blocked_on = BlockedOn::parse(value),
                "progress" => progress = value.parse::<u8>().ok().filter(|p| *p <= 100),
                "title" => title = text(value, MAX_TITLE),
                "msg" => message = text(value, MAX_MESSAGE),
                _ => {}
            }
        }
        let status = match (clear, state) {
            (true, _) => None,
            (false, Some(state)) => Some(ProgramStatus {
                app,
                state,
                blocked_on: blocked_on.filter(|_| state == ProgramState::Blocked),
                progress: progress
                    .filter(|_| matches!(state, ProgramState::Working | ProgramState::Blocked)),
                title,
                message,
            }),
            (false, None) => return None,
        };
        Some(Self { id, status })
    }
}

/// Ids and app names stay to the characters the protocol allows them, so a
/// snapshot can write them back as they came.
fn token(value: &str, max: usize) -> Option<String> {
    let valid = !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'));
    valid.then(|| value.to_owned())
}

/// Decoded text without control or direction-changing characters, which
/// would let a program rearrange the text it is shown beside.
fn text(value: &str, max: usize) -> Option<String> {
    let bytes = STANDARD.decode(value).ok()?;
    let text: String = String::from_utf8_lossy(&bytes)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .filter(
            |c| !matches!(*c as u32, 0x061c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069),
        )
        .take(max)
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(text: &str) -> String {
        STANDARD.encode(text)
    }

    fn applied(reports: &[&str]) -> ProgramStatuses {
        let mut statuses = ProgramStatuses::default();
        for report in reports {
            statuses.apply(report.as_bytes());
        }
        statuses
    }

    #[test]
    fn a_report_sets_the_programs_entry() {
        let report = format!(
            "state=blocked:app=claude-code:kind=permission:progress=40:title={}:msg={}",
            b64("Fix the flaky test"),
            b64("Bash: cargo test"),
        );
        let statuses = applied(&[&report]);
        assert_eq!(
            statuses.program(),
            Some(&ProgramStatus {
                app: Some("claude-code".into()),
                state: ProgramState::Blocked,
                blocked_on: Some(BlockedOn::Permission),
                progress: Some(40),
                title: Some("Fix the flaky test".into()),
                message: Some("Bash: cargo test".into()),
            })
        );
        assert_eq!(statuses.tasks().count(), 0);
    }

    #[test]
    fn a_later_report_replaces_the_whole_entry() {
        let statuses = applied(&[
            "state=blocked:app=claude-code:kind=question",
            "state=working:app=claude-code",
        ]);
        let program = statuses.program().unwrap();
        assert_eq!(program.state, ProgramState::Working);
        assert_eq!(program.blocked_on, None);
    }

    #[test]
    fn tasks_are_kept_by_id_and_cleared_one_by_one_or_all_at_once() {
        let mut statuses = applied(&[
            "state=working:app=claude-code",
            "state=working:app=claude-code:id=build",
            "state=blocked:app=claude-code:id=review",
        ]);
        let ids: Vec<_> = statuses.tasks().map(|(id, _)| id).collect();
        assert_eq!(ids, ["build", "review"]);
        assert!(statuses.apply(b"state=clear:id=build"));
        assert_eq!(statuses.tasks().count(), 1);
        assert!(statuses.program().is_some());
        assert!(statuses.apply(b"state=clear"));
        assert!(statuses.is_empty());
        assert!(!statuses.apply(b"state=clear"), "nothing left to clear");
    }

    #[test]
    fn repeating_a_report_changes_nothing() {
        let mut statuses = applied(&["state=working:app=claude-code"]);
        assert!(!statuses.apply(b"state=working:app=claude-code"));
        assert!(statuses.apply(b"state=done:app=claude-code"));
    }

    #[test]
    fn malformed_or_unknown_reports_leave_the_entries_alone() {
        let mut statuses = applied(&["state=working:app=claude-code"]);
        let before = statuses.clone();
        for report in [
            "state=thinking:app=claude-code",
            "app=claude-code",
            "state=done:id=has space",
            "state=done:app=claude code",
        ] {
            assert!(!statuses.apply(report.as_bytes()), "{report}");
        }
        assert!(!statuses.apply(&vec![b'x'; MAX_REPORT + 1]));
        assert!(!statuses.apply(b"\xff\xfe"));
        assert_eq!(statuses, before);
    }

    #[test]
    fn unknown_fields_and_bad_values_are_dropped_not_fatal() {
        let statuses = applied(&["state=working:app=x:progress=250:kind=nap:title=!!:future=1"]);
        let program = statuses.program().unwrap();
        assert_eq!(program.progress, None);
        assert_eq!(program.title, None);
        assert_eq!(program.blocked_on, None, "kind belongs to blocked reports");
    }

    #[test]
    fn text_loses_control_and_direction_characters() {
        let report = format!(
            "state=done:msg={}",
            b64("ok\x1b[31m\u{202e}evil\u{2066}\nnext")
        );
        let statuses = applied(&[&report]);
        assert_eq!(
            statuses.program().unwrap().message.as_deref(),
            Some("ok [31mevil next")
        );
    }

    #[test]
    fn the_number_of_tasks_is_bounded() {
        let mut statuses = ProgramStatuses::default();
        for i in 0..MAX_TASKS + 5 {
            statuses.apply(format!("state=working:id=t{i}").as_bytes());
        }
        assert_eq!(statuses.tasks().count(), MAX_TASKS);
        assert!(
            statuses.apply(b"state=done:id=t0"),
            "known tasks still update"
        );
    }

    #[test]
    fn the_most_urgent_entry_is_one_waiting_for_the_person() {
        let statuses = applied(&[
            "state=working:app=claude-code",
            "state=done:id=lint",
            "state=blocked:id=review:kind=question",
        ]);
        let urgent = statuses.most_urgent().unwrap();
        assert_eq!(urgent.blocked_on, Some(BlockedOn::Question));
        assert_eq!(
            applied(&["state=idle"]).most_urgent().unwrap().state,
            ProgramState::Idle
        );
        assert!(ProgramStatuses::default().most_urgent().is_none());
    }

    #[test]
    fn an_entry_survives_its_own_report_text() {
        let statuses = applied(&[&format!(
            "state=blocked:app=claude-code:kind=permission:progress=5:msg={}",
            b64("approve Bash: ls")
        )]);
        let entry = statuses.program().unwrap();
        assert_eq!(
            ProgramStatus::from_report(&entry.report()).as_ref(),
            Some(entry)
        );
        assert_eq!(ProgramStatus::from_report("state=clear"), None);
        assert_eq!(ProgramStatus::from_report("garbage"), None);
    }

    #[test]
    fn written_reports_restore_the_same_entries() {
        let original = applied(&[
            &format!(
                "state=blocked:app=claude-code:kind=auth:progress=10:title={}:msg={}",
                b64("Löschen 🧹"),
                b64("Sign in again")
            ),
            "state=working:app=claude-code:id=lint.1:progress=0",
            "state=error:id=deploy",
        ]);
        let mut written = String::new();
        original.write(&mut written);
        let mut restored = ProgramStatuses::default();
        for report in written.split("\x1b\\").filter(|r| !r.is_empty()) {
            let report = report.strip_prefix("\x1b]7501;").unwrap();
            restored.apply(report.as_bytes());
        }
        assert_eq!(restored, original);
    }
}
