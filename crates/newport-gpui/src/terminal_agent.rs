//! What a coding agent in a terminal says it is doing, as its tab shows it.
//! Agents report through the terminal itself (see `newport_terminal`'s
//! program statuses); the tab shows the entry that most needs the person.
use newport_terminal::{BlockedOn, ProgramState, ProgramStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentMark {
    Working,
    NeedsYou,
    /// Finished, and nobody has looked at the tab since.
    Done,
    Failed,
}
impl AgentMark {
    /// How the tab's accessible name ends.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::NeedsYou => "needs you",
            Self::Done => "done",
            Self::Failed => "stopped with an error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentPresentation {
    pub mark: AgentMark,
    pub hint: String,
}

#[derive(Default)]
pub(crate) struct AgentStatus {
    shown: Option<ProgramStatus>,
    /// Someone looked at the tab since the agent last finished.
    seen: bool,
    /// A finish this Mac saw before Newport last quit, as its report.
    seen_before: Option<String>,
}
impl AgentStatus {
    /// Follows the entry that most needs the person (see
    /// `ProgramStatuses::most_urgent`). A finish counts as seen when it
    /// happens in front of the person.
    pub(crate) fn update(&mut self, most_urgent: Option<&ProgramStatus>, looking: bool) {
        let next = most_urgent.cloned();
        let finished = |status: &Option<ProgramStatus>| {
            status
                .as_ref()
                .is_some_and(|s| s.state == ProgramState::Done)
        };
        if finished(&next) && !finished(&self.shown) {
            self.seen = looking || next.as_ref().map(ProgramStatus::report) == self.seen_before;
        }
        self.shown = next;
    }

    /// A finish, as its report, that this Mac saw before Newport last quit.
    /// The same finish counts as seen when it shows again.
    pub(crate) fn remember_seen(&mut self, report: String) {
        if self.shown.as_ref().map(ProgramStatus::report).as_ref() == Some(&report)
            && self
                .shown
                .as_ref()
                .is_some_and(|s| s.state == ProgramState::Done)
        {
            self.seen = true;
        }
        self.seen_before = Some(report);
    }

    /// The finish showing now, as its report, if someone has seen it.
    pub(crate) fn seen_finish(&self) -> Option<String> {
        self.shown
            .as_ref()
            .filter(|s| self.seen && s.state == ProgramState::Done)
            .map(ProgramStatus::report)
    }

    /// The person is looking at the tab. Returns whether its mark changed.
    pub(crate) fn see(&mut self) -> bool {
        let unseen = !self.seen
            && self
                .shown
                .as_ref()
                .is_some_and(|s| s.state == ProgramState::Done);
        self.seen = true;
        unseen
    }

    pub(crate) fn presentation(&self) -> Option<AgentPresentation> {
        let status = self.shown.as_ref()?;
        let (mark, label) = match status.state {
            ProgramState::Working => (AgentMark::Working, working(status.progress)),
            ProgramState::Blocked => (AgentMark::NeedsYou, waiting(status.blocked_on).into()),
            ProgramState::Done if !self.seen => (AgentMark::Done, "Done".into()),
            ProgramState::Error => (AgentMark::Failed, "Stopped with an error".into()),
            ProgramState::Done | ProgramState::Idle => return None,
        };
        let hint = match &status.message {
            Some(message) => format!("{label}\n{message}"),
            None => label,
        };
        Some(AgentPresentation { mark, hint })
    }
}

fn working(progress: Option<u8>) -> String {
    match progress {
        Some(percent) => format!("Working, {percent}% of its plan done"),
        None => "Working".into(),
    }
}

fn waiting(on: Option<BlockedOn>) -> &'static str {
    match on {
        Some(BlockedOn::Permission) => "Waiting for your permission",
        Some(BlockedOn::Question) => "Waiting for your answer",
        Some(BlockedOn::SignIn) => "Waiting for you to sign in",
        None => "Waiting for you",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_terminal::Engine;

    /// The entry a terminal fed `output` finds most urgent.
    fn urgent(output: &str) -> Option<ProgramStatus> {
        let mut engine = Engine::new(80, 24).unwrap();
        engine.advance(output.as_bytes()).unwrap();
        engine.program_statuses().most_urgent().cloned()
    }

    #[test]
    fn a_finish_seen_before_a_restart_is_seen_only_if_it_is_the_same_one() {
        let done = urgent("\x1b]7501;state=done:app=claude-code\x07");
        let report = done.as_ref().unwrap().report();
        // Remembered before the finish shows.
        let mut agent = AgentStatus::default();
        agent.remember_seen(report.clone());
        agent.update(done.as_ref(), false);
        assert_eq!(agent.presentation(), None);
        assert_eq!(agent.seen_finish(), Some(report.clone()));
        // Remembered once it shows.
        let mut agent = AgentStatus::default();
        agent.update(done.as_ref(), false);
        agent.remember_seen(report.clone());
        assert_eq!(agent.presentation(), None);
        // A different finish is new.
        let mut agent = AgentStatus::default();
        agent.remember_seen(report);
        agent.update(
            urgent("\x1b]7501;state=done:app=claude-code:msg=QWdhaW4=\x07").as_ref(),
            false,
        );
        assert_eq!(agent.presentation().map(|p| p.mark), Some(AgentMark::Done));
        assert_eq!(agent.seen_finish(), None, "not seen yet");
    }

    #[test]
    fn a_task_waiting_for_the_person_outranks_the_working_program() {
        let mut agent = AgentStatus::default();
        let entry = urgent(concat!(
            "\x1b]7501;state=working:app=claude-code\x07",
            "\x1b]7501;state=blocked:app=claude-code:id=review:kind=question:msg=UmV2aWV3Pw==\x07",
        ));
        agent.update(entry.as_ref(), false);
        assert_eq!(
            agent.presentation(),
            Some(AgentPresentation {
                mark: AgentMark::NeedsYou,
                hint: "Waiting for your answer\nReview?".into(),
            })
        );
    }

    #[test]
    fn done_shows_until_someone_looks() {
        let working = urgent("\x1b]7501;state=working\x07");
        let done = urgent("\x1b]7501;state=done\x07");
        let mut agent = AgentStatus::default();
        agent.update(working.as_ref(), false);
        agent.update(done.as_ref(), false);
        assert_eq!(agent.presentation().unwrap().mark, AgentMark::Done);
        assert!(agent.see());
        assert_eq!(agent.presentation(), None);
        assert!(!agent.see(), "already seen");

        agent.update(working.as_ref(), true);
        agent.update(done.as_ref(), true);
        assert_eq!(
            agent.presentation(),
            None,
            "finished in front of the person"
        );
    }

    #[test]
    fn idle_and_cleared_agents_show_nothing() {
        let mut agent = AgentStatus::default();
        agent.update(urgent("\x1b]7501;state=idle\x07").as_ref(), false);
        assert_eq!(agent.presentation(), None);
        let error = urgent("\x1b]7501;state=error:msg=T3V0IG9mIGNyZWRpdHM=\x07");
        agent.update(error.as_ref(), false);
        assert_eq!(
            agent.presentation().unwrap().hint,
            "Stopped with an error\nOut of credits"
        );
        agent.update(None, false);
        assert_eq!(agent.presentation(), None);
    }

    #[test]
    fn working_says_how_much_of_its_plan_is_done() {
        let mut agent = AgentStatus::default();
        agent.update(
            urgent("\x1b]7501;state=working:progress=40\x07").as_ref(),
            false,
        );
        assert_eq!(
            agent.presentation().unwrap().hint,
            "Working, 40% of its plan done"
        );
    }
}
