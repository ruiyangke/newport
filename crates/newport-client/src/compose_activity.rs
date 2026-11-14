//! Selection-scoped Activity data over the retained Containers reads.
//! No request dispatch or UI state belongs to this projection.
use crate::{containers::Containers, resources::Resource};
use newport_protocol::monitor::compose::{Job, Jobs};
use std::time::Instant;

#[derive(Default)]
pub struct ActivityData {
    project: Option<String>,
    selected_job: Option<String>,
    jobs_cache: ReadingCache<Jobs>,
    jobs: Vec<Job>,
    output_cache: ReadingCache<Job>,
    terminal: Option<TerminalObservation>,
}

impl ActivityData {
    /// Refresh metadata every time, but copy a reading only when a successful
    /// read changes it. Returns whether the visible data or request metadata
    /// changed.
    pub fn sync(&mut self, containers: &Containers) -> bool {
        self.sync_readings(
            containers.selected_project(),
            containers.selected_job(),
            Reading {
                value: containers.jobs(),
                resource: containers.resource("compose-jobs"),
            },
            Reading {
                value: containers.job_output(),
                resource: containers.resource("compose-output"),
            },
        )
    }

    pub fn project(&self) -> Option<&str> {
        self.project.as_deref()
    }
    pub fn selected_job(&self) -> Option<&str> {
        self.selected_job.as_deref()
    }
    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }
    pub fn output(&self) -> Option<&Job> {
        self.output_cache.data.as_ref()
    }
    /// Lifecycle metadata uses the latest known terminal observation even if
    /// output is delayed or fails. Read the retained command text via output().
    pub fn current(&self) -> Option<&Job> {
        self.terminal
            .as_ref()
            .map(|terminal| &terminal.job)
            .or_else(|| self.output())
            .or_else(|| self.summary())
    }
    fn summary(&self) -> Option<&Job> {
        match self.selected_job() {
            Some(selected) => self.jobs.iter().find(|job| job.job == selected),
            None => self.jobs.first(),
        }
    }
    pub fn jobs_error(&self) -> Option<&str> {
        self.jobs_cache.error.as_deref()
    }
    pub fn output_error(&self) -> Option<&str> {
        self.output_cache.error.as_deref()
    }
    pub fn jobs_pending(&self) -> bool {
        self.jobs_cache.pending
    }
    pub fn output_pending(&self) -> bool {
        self.output_cache.pending
    }

    fn sync_readings(
        &mut self,
        project: Option<&str>,
        selected: Option<&str>,
        jobs: Reading<'_, Jobs>,
        output: Reading<'_, Job>,
    ) -> bool {
        let project_changed = self.project.as_deref() != project;
        let output_changed = project_changed || self.selected_job.as_deref() != selected;
        if project_changed {
            self.project = project.map(str::to_owned);
        }
        if output_changed {
            self.selected_job = selected.map(str::to_owned);
            self.output_cache = ReadingCache::default();
            self.terminal = None;
        }
        let before = self.jobs_cache.value_generation;
        let jobs_changed = self.jobs_cache.sync(jobs, |_| Ok(()));
        if project_changed || before != self.jobs_cache.value_generation {
            self.jobs = self.jobs_cache.data.as_ref().map_or_else(Vec::new, |jobs| {
                jobs.jobs
                    .iter()
                    .filter(|job| Some(job.project.as_str()) == project)
                    .cloned()
                    .collect()
            });
        }
        let output_updated = if let (Some(project), Some(selected)) = (project, selected) {
            self.output_cache.sync(output, |job| {
                if job.project != project || job.job != selected {
                    return Err(
                        "Compose activity output does not match the selected project and command."
                            .into(),
                    );
                }
                if job.output.len() as u64 > job.offset {
                    return Err(
                        "Compose activity output exceeds its cumulative byte offset.".into(),
                    );
                }
                Ok(())
            })
        } else {
            self.output_cache.clear()
        };
        let lifecycle_changed = self.reconcile_terminal();
        project_changed || output_changed || jobs_changed || output_updated || lifecycle_changed
    }

    fn reconcile_terminal(&mut self) -> bool {
        let identity = self
            .output()
            .or_else(|| self.summary())
            .map(|job| (&job.project, &job.job));
        let retained = self
            .terminal
            .as_ref()
            .filter(|terminal| identity == Some((&terminal.job.project, &terminal.job.job)));
        let newest = [
            self.summary().zip(self.jobs_cache.accepted_stamp),
            self.output().zip(self.output_cache.accepted_stamp),
        ]
        .into_iter()
        .flatten()
        .filter(|(job, _)| !job.state.is_running())
        .max_by_key(|(_, (revision, at))| (*at, *revision));
        if let Some(retained) = retained
            && newest.is_none_or(|(_, (revision, at))| {
                (at, revision) <= (retained.stamp.1, retained.stamp.0)
            })
        {
            return false;
        }
        let next = newest.map(|(job, stamp)| TerminalObservation {
            // Lifecycle metadata is small; output() keeps the sole typed text
            // buffer and its cumulative offset, regardless of refresh errors.
            job: Job {
                job: job.job.clone(),
                project: job.project.clone(),
                command: job.command.clone(),
                state: job.state,
                exit_code: job.exit_code,
                started_at: job.started_at,
                finished_at: job.finished_at,
                output: String::new(),
                offset: job.offset,
            },
            stamp,
        });
        let changed = self.terminal.as_ref().map(|terminal| &terminal.job)
            != next.as_ref().map(|terminal| &terminal.job);
        self.terminal = next;
        changed
    }
}

struct TerminalObservation {
    job: Job,
    stamp: (u64, Option<Instant>),
}

struct Reading<'a, T> {
    value: Option<&'a T>,
    resource: Option<&'a Resource>,
}
impl<T> Clone for Reading<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Reading<'_, T> {}

struct ReadingCache<T> {
    stamp: Option<(u64, Option<Instant>)>,
    /// The metadata belongs to the last valid value, not a newer invalid
    /// response whose error merely retained that value.
    accepted_stamp: Option<(u64, Option<Instant>)>,
    data: Option<T>,
    invalid: Option<String>,
    error: Option<String>,
    pending: bool,
    value_generation: u64,
    #[cfg(test)]
    copies: usize,
}
impl<T> Default for ReadingCache<T> {
    fn default() -> Self {
        Self {
            stamp: None,
            accepted_stamp: None,
            data: None,
            invalid: None,
            error: None,
            pending: false,
            value_generation: 0,
            #[cfg(test)]
            copies: 0,
        }
    }
}

impl<T: Clone + PartialEq> ReadingCache<T> {
    fn clear(&mut self) -> bool {
        let changed = self.clear_data() || self.error.is_some() || self.pending;
        self.error = None;
        self.pending = false;
        changed
    }

    fn clear_data(&mut self) -> bool {
        let changed = self.data.take().is_some();
        if changed {
            self.value_generation += 1;
        }
        self.stamp = None;
        self.accepted_stamp = None;
        self.invalid = None;
        changed
    }

    /// Copies a new successful reading that `validate` accepts; an invalid
    /// one leaves the last valid data in place and reports why.
    fn sync(
        &mut self,
        reading: Reading<'_, T>,
        validate: impl FnOnce(&T) -> Result<(), String>,
    ) -> bool {
        let mut changed = false;
        match (reading.value, reading.resource) {
            (Some(value), Some(resource)) => {
                let stamp = (resource.revision, resource.updated_at);
                if self.stamp != Some(stamp) {
                    self.stamp = Some(stamp);
                    if self.data.as_ref() == Some(value) {
                        self.invalid = None;
                    } else {
                        match validate(value) {
                            Ok(()) => {
                                self.data = Some(value.clone());
                                self.value_generation += 1;
                                changed = true;
                                self.invalid = None;
                                #[cfg(test)]
                                {
                                    self.copies += 1;
                                }
                            }
                            Err(error) => self.invalid = Some(error),
                        }
                    }
                    if self.invalid.is_none() {
                        self.accepted_stamp = Some(stamp);
                    }
                }
            }
            _ => changed |= self.clear_data(),
        }
        let error = reading
            .resource
            .and_then(|resource| resource.error.as_ref())
            .or(self.invalid.as_ref());
        let pending = reading.resource.is_some_and(|resource| resource.pending);
        changed |= self.error.as_ref() != error || self.pending != pending;
        if self.error.as_ref() != error {
            self.error = error.cloned();
        }
        self.pending = pending;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::monitor::compose::State;

    fn job(id: &str, project: &str, output: &str) -> Job {
        Job {
            job: id.into(),
            project: project.into(),
            command: "docker compose up -d".into(),
            state: State::Running,
            started_at: 1000,
            output: output.into(),
            offset: output.len() as u64,
            ..Job::default()
        }
    }
    fn listing(jobs: Vec<Job>) -> Jobs {
        Jobs { jobs }
    }
    fn resource(revision: u64) -> Resource {
        let mut resource = Resource::default();
        resource.revision = revision;
        resource
    }
    fn reading<'a, T>(value: &'a T, resource: &'a Resource) -> Reading<'a, T> {
        Reading {
            value: Some(value),
            resource: Some(resource),
        }
    }
    fn absent<T>() -> Reading<'static, T> {
        Reading {
            value: None,
            resource: None,
        }
    }

    #[test]
    fn caches_unchanged_data_and_updates_errors_and_pending_without_reparsing() {
        let list = listing(vec![job("a", "app", ""), job("b", "other", "")]);
        let output = job("a", "app", "first\n");
        let mut list_resource = resource(1);
        let mut output_resource = resource(2);
        let mut data = ActivityData::default();
        assert!(data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &list_resource),
            reading(&output, &output_resource)
        ));
        for _ in 0..20 {
            assert!(!data.sync_readings(
                Some("app"),
                Some("a"),
                reading(&list, &list_resource),
                reading(&output, &output_resource)
            ));
        }
        assert_eq!(data.jobs().len(), 1);
        assert_eq!(data.output().unwrap().output, "first\n");
        assert_eq!((data.jobs_cache.copies, data.output_cache.copies), (1, 1));
        list_resource.error = Some("list failed".into());
        output_resource.error = Some("output failed".into());
        output_resource.pending = true;
        assert!(data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &list_resource),
            reading(&output, &output_resource)
        ));
        assert_eq!(data.jobs_error(), Some("list failed"));
        assert_eq!(data.output_error(), Some("output failed"));
        assert!(data.output_pending());
        assert_eq!(data.current().unwrap().output, "first\n");
        list_resource.revision = 3;
        list_resource.error = None;
        output_resource.revision = 4;
        output_resource.error = None;
        output_resource.pending = false;
        assert!(data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &list_resource),
            reading(&output, &output_resource)
        ));
        assert_eq!((data.jobs_cache.copies, data.output_cache.copies), (1, 1));
        assert!(data.jobs_error().is_none());
        assert!(data.output_error().is_none());
    }

    #[test]
    fn selection_changes_filter_existing_jobs_and_never_expose_another_targets_output() {
        let list = listing(vec![job("a", "app", ""), job("b", "other", "")]);
        let output = job("a", "app", "private app output\n");
        let metadata = resource(1);
        let mut data = ActivityData::default();
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &metadata),
            reading(&output, &metadata),
        );
        data.sync_readings(
            Some("other"),
            Some("b"),
            reading(&list, &metadata),
            reading(&output, &metadata),
        );
        assert_eq!(data.jobs_cache.copies, 1);
        assert_eq!(data.jobs()[0].job, "b");
        assert!(data.output().is_none());
        assert_eq!(data.current().unwrap().job, "b");
        assert!(data.output_error().unwrap().contains("selected project"));
        data.sync_readings(Some("other"), None, reading(&list, &metadata), absent());
        assert_eq!(data.current().unwrap().job, "b");
        assert!(data.output_error().is_none());
        data.sync_readings(None, None, reading(&list, &metadata), absent());
        assert!(data.jobs().is_empty());
        assert!(data.current().is_none());
        assert!(data.output().is_none());
    }

    #[test]
    fn invalid_output_refresh_retains_valid_scoped_data_and_initial_failure_is_contextual() {
        let list = listing(vec![job("a", "app", "")]);
        let output = job("a", "app", "ready\n");
        let metadata = resource(1);
        let changed = resource(2);
        let mut bad_output = job("a", "app", "ready\nmore\n");
        bad_output.offset = 1;
        let mut data = ActivityData::default();
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &metadata),
            reading(&output, &metadata),
        );
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &changed),
            reading(&bad_output, &changed),
        );
        assert_eq!(data.jobs()[0].job, "a");
        assert_eq!(data.output().unwrap().output, "ready\n");
        assert!(data.jobs_error().is_none());
        assert!(data.output_error().unwrap().contains("byte offset"));
        let mut initial = ActivityData::default();
        initial.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &changed),
            reading(&bad_output, &changed),
        );
        assert!(initial.output().is_none());
        assert!(initial.output_error().is_some());
        let recovered = resource(3);
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &recovered),
            reading(&output, &recovered),
        );
        assert!(data.jobs_error().is_none());
        assert!(data.output_error().is_none());
    }

    #[test]
    fn slot_reset_clears_old_data_even_when_recreated_slot_reuses_a_revision() {
        let list = listing(vec![job("a", "app", "")]);
        let output = job("a", "app", "old\n");
        let replacement = job("a", "app", "new\n");
        let metadata = resource(1);
        let mut data = ActivityData::default();
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &metadata),
            reading(&output, &metadata),
        );
        assert!(data.sync_readings(Some("app"), Some("a"), absent(), absent()));
        assert!(data.current().is_none());
        assert!(data.jobs().is_empty());
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &metadata),
            reading(&replacement, &metadata),
        );
        assert_eq!(data.output().unwrap().output, "new\n");
        let mut invalid = job("a", "app", "é");
        invalid.offset = 1;
        let refreshed = resource(2);
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &metadata),
            reading(&invalid, &refreshed),
        );
        assert_eq!(data.output().unwrap().output, "new\n");
        assert!(data.output_error().unwrap().contains("byte offset"));
    }

    #[test]
    fn initial_pending_and_failure_metadata_settles_without_a_success_revision() {
        let mut metadata = resource(0);
        metadata.pending = true;
        let mut data = ActivityData::default();
        fn pending<T>(resource: &Resource) -> Reading<'_, T> {
            Reading {
                value: None,
                resource: Some(resource),
            }
        }
        assert!(data.sync_readings(
            Some("app"),
            Some("a"),
            pending(&metadata),
            pending(&metadata)
        ));
        for _ in 0..10 {
            assert!(!data.sync_readings(
                Some("app"),
                Some("a"),
                pending(&metadata),
                pending(&metadata)
            ));
        }
        assert!(data.jobs_pending());
        assert!(data.output_pending());
        metadata.pending = false;
        metadata.error = Some("offline".into());
        assert!(data.sync_readings(
            Some("app"),
            Some("a"),
            pending(&metadata),
            pending(&metadata)
        ));
        assert!(!data.sync_readings(
            Some("app"),
            Some("a"),
            pending(&metadata),
            pending(&metadata)
        ));
        assert_eq!(data.jobs_error(), Some("offline"));
        assert_eq!(data.output_error(), Some("offline"));
        assert!(!data.jobs_pending());
        assert!(!data.output_pending());
    }

    #[test]
    fn choosing_another_command_clears_output_even_with_the_same_project_and_revision() {
        let list = listing(vec![job("a", "app", ""), job("b", "app", "")]);
        let first = job("a", "app", "first command\n");
        let second = job("b", "app", "second command\n");
        let metadata = resource(1);
        let mut data = ActivityData::default();
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &metadata),
            reading(&first, &metadata),
        );
        data.sync_readings(Some("app"), Some("b"), reading(&list, &metadata), absent());
        assert!(data.output().is_none());
        assert_eq!(data.current().unwrap().job, "b");
        assert_eq!(data.current().unwrap().output, "");
        data.sync_readings(
            Some("app"),
            Some("b"),
            reading(&list, &metadata),
            reading(&second, &metadata),
        );
        assert_eq!(data.output().unwrap().output, "second command\n");
        assert_eq!(data.jobs_cache.copies, 1);
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &metadata),
            reading(&first, &metadata),
        );
        assert_eq!(data.output().unwrap().output, "first command\n");
    }

    #[test]
    fn terminal_jobs_list_wins_while_running_output_is_delayed_or_failed() {
        for state in [State::Succeeded, State::Failed, State::Cancelled] {
            let running_list = listing(vec![job("a", "app", "")]);
            let output = job("a", "app", "retained output\n");
            let listed = resource(1);
            let mut output_resource = resource(2);
            let mut data = ActivityData::default();
            data.sync_readings(
                Some("app"),
                Some("a"),
                reading(&running_list, &listed),
                reading(&output, &output_resource),
            );
            let mut terminal = running_list.clone();
            terminal.jobs[0].state = state;
            terminal.jobs[0].exit_code = Some(17);
            terminal.jobs[0].finished_at = Some(2500);
            let completed = resource(3);
            output_resource.pending = true;
            data.sync_readings(
                Some("app"),
                Some("a"),
                reading(&terminal, &completed),
                reading(&output, &output_resource),
            );
            assert_eq!(data.current().unwrap().state, state);
            assert_eq!(data.current().unwrap().exit_code, Some(17));
            assert_eq!(data.current().unwrap().finished_at, Some(2500));
            assert!(data.output_pending());
            output_resource.pending = false;
            output_resource.error = Some("output refresh failed".into());
            data.sync_readings(
                Some("app"),
                Some("a"),
                reading(&terminal, &completed),
                reading(&output, &output_resource),
            );
            assert_eq!(data.current().unwrap().state, state);
            assert_eq!(data.output().unwrap().output, "retained output\n");
            assert_eq!(
                data.output().unwrap().offset,
                "retained output\n".len() as u64
            );
            assert_eq!(data.output_error(), Some("output refresh failed"));
            assert_eq!((data.jobs_cache.copies, data.output_cache.copies), (2, 1));
        }
    }

    #[test]
    fn terminal_output_never_regresses_on_later_running_list_or_output_observations() {
        let list = listing(vec![job("a", "app", "")]);
        let mut output = job("a", "app", "finished\n");
        output.state = State::Succeeded;
        output.exit_code = Some(0);
        output.finished_at = Some(3000);
        let first = resource(1);
        let completed = resource(2);
        let mut data = ActivityData::default();
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &first),
            reading(&output, &completed),
        );
        let stale_list = resource(3);
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &stale_list),
            reading(&output, &completed),
        );
        assert_eq!(data.current().unwrap().state, State::Succeeded);
        let late_running = job("a", "app", "finished\nlate output\n");
        let stale_output = resource(4);
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &stale_list),
            reading(&late_running, &stale_output),
        );
        assert_eq!(data.current().unwrap().state, State::Succeeded);
        assert_eq!(data.current().unwrap().finished_at, Some(3000));
        assert_eq!(data.output().unwrap().output, "finished\nlate output\n");
        assert_eq!(
            data.output().unwrap().offset,
            "finished\nlate output\n".len() as u64
        );
        let others = listing(vec![job("b", "app", "")]);
        data.sync_readings(
            Some("app"),
            Some("b"),
            reading(&others, &resource(5)),
            absent(),
        );
        assert_eq!(data.current().unwrap().state, State::Running);
        assert_eq!(data.current().unwrap().job, "b");
        assert!(data.output().is_none());
        data.sync_readings(Some("app"), Some("b"), absent(), absent());
        assert!(data.current().is_none());
    }

    #[test]
    fn terminal_precedence_uses_latest_valid_completion_not_failed_refresh_metadata() {
        let mut output = job("a", "app", "retained\n");
        output.state = State::Succeeded;
        output.exit_code = Some(0);
        output.finished_at = Some(2000);
        let mut list = listing(vec![job("a", "app", "")]);
        list.jobs[0].state = State::Failed;
        list.jobs[0].exit_code = Some(1);
        list.jobs[0].finished_at = Some(2500);
        let now = Instant::now();
        let mut listed = resource(1);
        listed.updated_at = Some(now + std::time::Duration::from_secs(1));
        let mut output_resource = resource(2);
        output_resource.updated_at = Some(now);
        let mut data = ActivityData::default();
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &listed),
            reading(&output, &output_resource),
        );
        assert_eq!(data.current().unwrap().state, State::Failed);
        let mut invalid = job("a", "app", "late\n");
        invalid.state = State::Succeeded;
        invalid.offset = 0;
        output_resource.revision = 3;
        output_resource.updated_at = Some(now + std::time::Duration::from_secs(2));
        data.sync_readings(
            Some("app"),
            Some("a"),
            reading(&list, &listed),
            reading(&invalid, &output_resource),
        );
        assert_eq!(data.current().unwrap().state, State::Failed);
        assert_eq!(data.current().unwrap().exit_code, Some(1));
        assert_eq!(data.output().unwrap().output, "retained\n");
        assert!(data.output_error().is_some());
    }
}
