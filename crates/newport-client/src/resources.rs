//! Cached request state independent of the UI framework.
//!
//! Generations prevent late results from replacing a newer selection. Refreshes
//! retain the last successful value while tracking manual activity separately
//! from background reads.
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Resource {
    pub value: Value,
    pub error: Option<String>,
    /// Exact protocol code, never inferred from a message or transport error.
    pub error_code: Option<String>,
    pub pending: bool,
    /// A backend-owned query is refreshing independently of this RPC request.
    pub refreshing: bool,
    pub stale: bool,
    /// A manual refresh is awaiting this request, including a coalesced read.
    pub manual_pending: bool,
    /// Changes only when a successful response replaces the value.
    pub revision: u64,
    /// Changes only when the displayed value changes, for memoized projections.
    pub data_revision: u64,
    /// Settled display state, independent of request completion. Background
    /// observers can skip redraws when this stays unchanged. Request admission
    /// and operation acknowledgements still belong to their view/controller.
    pub presentation_revision: u64,
    /// Last accepted completion, successful or failed, used to schedule polling.
    pub finished_at: Option<Instant>,
    /// Last successful response; a failed refresh does not make old data fresh.
    pub updated_at: Option<Instant>,
    poll_checked_at: Option<Instant>,
    generation: u64,
    presentation: Option<Presentation>,
}

#[derive(PartialEq)]
struct Presentation {
    data_revision: u64,
    error: Option<String>,
    error_code: Option<String>,
    refreshing: bool,
    stale: bool,
    manual_pending: bool,
    // Keep the backend timestamp, rather than comparing reconstructed Instants
    // whose rounding differs slightly on every observation of the same reading.
    observed_at: Option<u64>,
}

impl Resource {
    /// Cached background reads do not replace content with a loading state.
    /// Explicit refreshes and initial reads still expose their activity.
    pub fn display_pending(&self) -> bool {
        self.pending && (self.manual_pending || self.updated_at.is_none())
    }

    /// Settle a read whose generation has already been validated by its owner.
    /// Completion identity always advances, even when presentation does not.
    pub(crate) fn finish_read(
        &mut self,
        generation: u64,
        result: Result<Value, String>,
        now: Instant,
        error_code: Option<String>,
    ) {
        let finished_manual = self.manual_pending;
        self.pending = false;
        self.refreshing = false;
        self.manual_pending = false;
        self.finished_at = Some(now);
        self.poll_checked_at = None;
        self.error_code = error_code;
        match result {
            Ok(value) => {
                self.set_value(value, generation);
                self.updated_at = Some(now);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        self.publish_presentation(generation, None, finished_manual);
    }

    fn publish_presentation(
        &mut self,
        generation: u64,
        observed_at: Option<u64>,
        finished_manual: bool,
    ) {
        let next = Presentation {
            data_revision: self.data_revision,
            error: self.error.clone(),
            error_code: self.error_code.clone(),
            refreshing: self.refreshing,
            stale: self.stale,
            manual_pending: self.manual_pending,
            observed_at,
        };
        if self.presentation.as_ref() != Some(&next) || finished_manual {
            self.presentation_revision = generation;
            self.presentation = Some(next);
        }
    }
    pub fn set_value(&mut self, value: Value, revision: u64) {
        if self.updated_at.is_none() || self.value != value {
            self.value = value;
            self.data_revision = revision;
        }
        self.revision = revision;
    }

    /// Whether a settled resource can be refreshed by a visible page's timer.
    /// Initial reads are started explicitly, not by the polling timer.
    pub fn poll_due(&self, now: Instant, interval: Duration, visible: bool) -> bool {
        visible
            && !self.pending
            && self
                .finished_at
                .and_then(|finished_at| now.checked_duration_since(finished_at))
                .is_some_and(|elapsed| elapsed >= interval)
    }
}

/// Settled read resources suspended with their owning view. Pending rows are
/// omitted: their workers may finish, but their results must not enter a later
/// view that happens to use the same keys.
#[derive(Default)]
pub struct ResourceGroup {
    rows: HashMap<String, Resource>,
    had_pending: bool,
    generation: u64,
}

impl ResourceGroup {
    /// The owner should reload its view if a read was abandoned while leaving.
    pub fn had_pending(&self) -> bool {
        self.had_pending
    }
}

#[derive(Default)]
pub struct Resources {
    rows: HashMap<String, Resource>,
    next: u64,
}
impl Resources {
    pub fn invalidate(&mut self, key: &str) {
        self.rows.remove(key);
    }
    pub fn invalidate_prefix(&mut self, prefix: &str) {
        self.rows.retain(|key, _| !key.starts_with(prefix));
    }
    /// Retire any in-flight read so the next request starts fresh, while the
    /// last value stays on screen. Prefer this after a mutation: `invalidate`
    /// also drops the value, which sends views back to their first-load state.
    pub fn fence(&mut self, key: &str) {
        let group = self.take_cached(&[key]);
        self.restore(group);
    }
    /// [`Self::fence`] for every key in a dynamic read namespace.
    pub fn fence_prefix(&mut self, prefix: &str) {
        let group = self.take_cached_prefix(prefix);
        self.restore(group);
    }
    /// Suspend only this namespace without clearing unrelated shell resources.
    /// Values, errors, revisions and freshness are moved intact. In-flight rows
    /// are removed too, but never saved as permanently pending cached resources.
    pub fn take_prefix(&mut self, prefix: &str) -> ResourceGroup {
        self.take_prefix_except(prefix, &[])
    }
    /// Suspend view-local resources while keeping exact shared keys active.
    /// Exclusions do not match descendants or other keys with the same prefix.
    pub fn take_prefix_except(&mut self, prefix: &str, exceptions: &[&str]) -> ResourceGroup {
        let mut group = ResourceGroup {
            generation: self.next,
            ..ResourceGroup::default()
        };
        for (key, row) in self
            .rows
            .extract_if(|key, _| key.starts_with(prefix) && !exceptions.contains(&key.as_str()))
        {
            if row.pending {
                group.had_pending = true;
            } else {
                group.rows.insert(key, row);
            }
        }
        group
    }
    /// Keep the last rendered data when leaving a page, even during a refresh.
    /// Detached requests are invalidated; they cannot populate another route.
    pub fn take_cached(&mut self, keys: &[&str]) -> ResourceGroup {
        let mut group = ResourceGroup::default();
        for key in keys {
            if let Some(mut row) = self.rows.remove(*key) {
                group.had_pending |= row.pending;
                self.next += 1;
                row.generation = self.next;
                row.pending = false;
                row.manual_pending = false;
                group.rows.insert((*key).to_owned(), row);
            }
        }
        group.generation = self.next;
        group
    }
    /// Preserve cached values for a dynamic read namespace while fencing its
    /// detached workers, just like `take_cached` does for fixed keys.
    pub fn take_cached_prefix(&mut self, prefix: &str) -> ResourceGroup {
        let keys: Vec<_> = self
            .rows
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect();
        self.take_cached(&keys.iter().map(String::as_str).collect::<Vec<_>>())
    }
    /// Shell-owned operations, and pages that manage their own returns, must
    /// survive ordinary page navigation.
    pub fn clear_preserving(&mut self, prefixes: &[&str]) {
        let kept: HashMap<_, _> = self
            .rows
            .extract_if(|key, _| prefixes.iter().any(|prefix| key.starts_with(prefix)))
            .collect();
        self.clear();
        self.rows.extend(kept);
    }
    /// Restore a view's settled cache. The caller owns repository/view scope
    /// validation and drops suspended groups after invalidating that scope.
    /// An already-active key always wins, including a newer pending request.
    pub fn restore(&mut self, group: ResourceGroup) {
        // Include generations of omitted pending rows so transferring a group
        // cannot reuse a generation whose delayed response is still in flight.
        self.next = self.next.max(group.generation);
        for (key, row) in group.rows {
            self.rows.entry(key).or_insert(row);
        }
    }
    pub fn get(&self, key: &str) -> Option<&Resource> {
        self.rows.get(key)
    }
    /// Consume a scheduled interval even when hidden. Becoming visible does
    /// not itself trigger a refresh; the next interval does, as in usePorts.
    pub fn take_poll(
        &mut self,
        key: &str,
        now: Instant,
        interval: Duration,
        visible: bool,
    ) -> bool {
        let Some(row) = self.rows.get_mut(key) else {
            return false;
        };
        if !row.poll_due(now, interval, true)
            || row.poll_checked_at.is_some_and(|at| {
                now.checked_duration_since(at)
                    .is_none_or(|elapsed| elapsed < interval)
            })
        {
            return false;
        }
        row.poll_checked_at = Some(now);
        visible
    }
    pub fn begin(&mut self, key: &str) -> Option<u64> {
        self.begin_with_activity(key, false, false)
    }
    /// Starts a manual read, or marks an existing read as serving that refresh.
    /// A `None` result means the caller must not dispatch a second request.
    pub fn begin_manual(&mut self, key: &str) -> Option<u64> {
        self.begin_with_activity(key, true, false)
    }
    /// Refresh a cached reading without hiding its stale-data warning while a
    /// retry is in flight. Initial-read errors can clear because no cached
    /// reading is being presented as current.
    pub fn begin_refresh(&mut self, key: &str, manual: bool) -> Option<u64> {
        self.begin_with_activity(key, manual, true)
    }
    fn begin_with_activity(
        &mut self,
        key: &str,
        manual: bool,
        preserve_stale_error: bool,
    ) -> Option<u64> {
        let row = self.rows.entry(key.to_owned()).or_default();
        if row.pending {
            row.manual_pending |= manual;
            return None;
        }
        self.next += 1;
        row.generation = self.next;
        row.pending = true;
        row.manual_pending = manual || row.refreshing && row.manual_pending;
        if !preserve_stale_error || row.updated_at.is_none() {
            row.error = None;
            row.error_code = None;
        }
        Some(self.next)
    }
    pub fn complete(&mut self, key: &str, generation: u64, result: Result<Value, String>) -> bool {
        self.complete_at(key, generation, result, Instant::now())
    }
    pub fn complete_detailed(
        &mut self,
        key: &str,
        generation: u64,
        result: Result<Value, crate::request_error::RequestError>,
    ) -> bool {
        let code = result.as_ref().err().and_then(|error| error.code.clone());
        self.complete_at_with_code(
            key,
            generation,
            result.map_err(|error| error.message),
            Instant::now(),
            code,
        )
    }

    /// Accept an application-owned query observation without treating every
    /// cache read as a fresh remote result or rebuilding unchanged rows.
    pub fn complete_query(
        &mut self,
        key: &str,
        generation: u64,
        result: Result<Value, crate::request_error::RequestError>,
    ) -> bool {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Observation {
            data: Value,
            error: Option<String>,
            fetching: bool,
            stale: bool,
            updated_at: Option<u64>,
        }
        let observation = match result {
            Ok(value) => match serde_json::from_value::<Observation>(value) {
                Ok(value) => value,
                Err(error) => {
                    return self.complete(
                        key,
                        generation,
                        Err(format!("Invalid query snapshot: {error}")),
                    );
                }
            },
            Err(error) => return self.complete_detailed(key, generation, Err(error)),
        };
        let Some(row) = self.rows.get_mut(key) else {
            return false;
        };
        if !row.pending || row.generation != generation {
            return false;
        }
        let manual_pending = row.manual_pending;
        row.pending = false;
        row.refreshing = observation.fetching;
        row.stale = observation.stale;
        row.manual_pending &= observation.fetching;
        row.finished_at = Some(Instant::now());
        row.poll_checked_at = None;
        row.error = observation.error;
        row.error_code = None;
        // A replaced or retired backend query reports no data while it
        // refetches. Keep presenting the last reading instead of falling back
        // to the first-load state; real scope changes clear rows explicitly.
        let keep = observation.data.is_null() && observation.fetching && !row.value.is_null();
        if !keep {
            if row.value != observation.data {
                row.set_value(observation.data, generation);
                row.revision = generation;
            }
            row.updated_at = observation.updated_at.and_then(|millis| {
                let age = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH + Duration::from_millis(millis))
                    .unwrap_or_default();
                Instant::now().checked_sub(age)
            });
        }
        row.publish_presentation(
            generation,
            observation.updated_at,
            manual_pending && !row.manual_pending,
        );
        true
    }
    fn complete_at(
        &mut self,
        key: &str,
        generation: u64,
        result: Result<Value, String>,
        now: Instant,
    ) -> bool {
        self.complete_at_with_code(key, generation, result, now, None)
    }
    fn complete_at_with_code(
        &mut self,
        key: &str,
        generation: u64,
        result: Result<Value, String>,
        now: Instant,
        error_code: Option<String>,
    ) -> bool {
        let Some(row) = self.rows.get_mut(key) else {
            return false;
        };
        if !row.pending || row.generation != generation {
            return false;
        }
        row.finish_read(generation, result, now, error_code);
        true
    }
    pub fn clear(&mut self) {
        self.rows.retain(|key, row| {
            key == "snapshot"
                // The server editor belongs to the shell. A snapshot can switch
                // pages before its observer consumes a completed save or key read.
                // Its owner explicitly invalidates these when the dialog closes.
                || key == "server-save"
                || key == "server-form-agent-keys"
                // First-run imports finish one by one while the page may change;
                // the welcome screen reads each result before starting the next.
                || key.starts_with("server-import:")
                || ((key == "project-created-save"
                    || key == "action"
                    || key == "compose-start"
                    || key == "file-project"
                    || key.starts_with("server-test:")
                    || key.starts_with("integration:")
                    || key.starts_with("server-delete:"))
                    && row.pending)
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cached_prefix_retains_values_and_errors_but_retires_pending_generations() {
        let mut resources = Resources::default();
        let key = "project-status:one";
        let generation = resources.begin(key).unwrap();
        resources.complete(key, generation, Ok(json!({"changes":2})));
        let generation = resources.begin(key).unwrap();
        resources.complete(key, generation, Err("Offline".into()));
        let pending = resources.begin_refresh(key, false).unwrap();
        let unrelated = resources.begin("repository").unwrap();
        let saved = resources.take_cached_prefix("project-status:");
        assert!(saved.had_pending());
        assert!(resources.get(key).is_none());
        assert!(resources.get("repository").unwrap().pending);
        resources.restore(saved);
        let row = resources.get(key).unwrap();
        assert_eq!(row.value, json!({"changes":2}));
        assert_eq!(row.error.as_deref(), Some("Offline"));
        assert!(!row.pending);
        assert!(!resources.complete(key, pending, Ok(json!({"changes":99}))));
        assert!(resources.complete("repository", unrelated, Ok(json!({}))));
    }

    #[test]
    fn fence_keeps_the_displayed_value_but_rejects_the_retired_read() {
        let mut resources = Resources::default();
        let generation = resources.begin("snapshot").unwrap();
        resources.complete("snapshot", generation, Ok(json!({"revision":1})));
        let stale = resources.begin_refresh("snapshot", false).unwrap();
        resources.fence("snapshot");
        let row = resources.get("snapshot").unwrap();
        assert_eq!(row.value, json!({"revision":1}));
        assert!(!row.pending);
        assert!(!resources.complete("snapshot", stale, Ok(json!({"revision":0}))));
        let fresh = resources.begin_refresh("snapshot", false).unwrap();
        assert!(fresh > stale);
        assert_eq!(
            resources.get("snapshot").unwrap().value,
            json!({"revision":1})
        );
        assert!(resources.complete("snapshot", fresh, Ok(json!({"revision":2}))));

        resources.fence("missing");
        assert!(resources.get("missing").is_none());
        let status = resources.begin("project-status:a").unwrap();
        resources.complete("project-status:a", status, Ok(json!({"changes":1})));
        resources.fence_prefix("project-status:");
        assert_eq!(
            resources.get("project-status:a").unwrap().value,
            json!({"changes":1})
        );
    }

    #[test]
    fn presentation_tracks_errors_and_manual_completion_without_redrawing_identical_reads() {
        let mut resources = Resources::default();
        let first = resources.begin("snapshot").unwrap();
        resources.complete("snapshot", first, Ok(json!({"ready":true})));
        assert_eq!(
            resources.get("snapshot").unwrap().presentation_revision,
            first
        );
        let second = resources.begin("snapshot").unwrap();
        resources.complete("snapshot", second, Ok(json!({"ready":true})));
        assert_eq!(
            resources.get("snapshot").unwrap().presentation_revision,
            first
        );
        assert_eq!(resources.get("snapshot").unwrap().revision, second);
        let failed = resources.begin("snapshot").unwrap();
        resources.complete("snapshot", failed, Err("offline".into()));
        assert_eq!(
            resources.get("snapshot").unwrap().presentation_revision,
            failed
        );
        let repeat = resources.begin_refresh("snapshot", false).unwrap();
        resources.complete("snapshot", repeat, Err("offline".into()));
        assert_eq!(
            resources.get("snapshot").unwrap().presentation_revision,
            failed
        );
        let recovered = resources.begin_refresh("snapshot", false).unwrap();
        resources.complete("snapshot", recovered, Ok(json!({"ready":true})));
        assert_eq!(
            resources.get("snapshot").unwrap().presentation_revision,
            recovered
        );
        let manual = resources.begin_manual("snapshot").unwrap();
        resources.complete("snapshot", manual, Ok(json!({"ready":true})));
        assert_eq!(
            resources.get("snapshot").unwrap().presentation_revision,
            manual
        );
        assert!(!resources.get("snapshot").unwrap().manual_pending);
        assert_eq!(resources.get("snapshot").unwrap().data_revision, first);
    }

    #[test]
    fn query_presentation_preserves_timestamp_identity_and_shows_new_sample_and_refresh_state() {
        let mut resources = Resources::default();
        let mut observation =
            json!({"data":[8080],"fetching":false,"error":null,"stale":false,"updatedAt":1000});
        let first = resources.begin("page").unwrap();
        resources.complete_query("page", first, Ok(observation.clone()));
        let second = resources.begin_refresh("page", false).unwrap();
        resources.complete_query("page", second, Ok(observation.clone()));
        assert_eq!(resources.get("page").unwrap().presentation_revision, first);
        observation["updatedAt"] = json!(2000);
        let reading = resources.begin_refresh("page", false).unwrap();
        resources.complete_query("page", reading, Ok(observation.clone()));
        assert_eq!(
            resources.get("page").unwrap().presentation_revision,
            reading
        );
        assert_eq!(resources.get("page").unwrap().data_revision, first);
        observation["fetching"] = json!(true);
        let manual = resources.begin_manual("page").unwrap();
        resources.complete_query("page", manual, Ok(observation.clone()));
        assert_eq!(resources.get("page").unwrap().presentation_revision, manual);
        let poll = resources.begin_refresh("page", false).unwrap();
        resources.complete_query("page", poll, Ok(observation.clone()));
        assert_eq!(resources.get("page").unwrap().presentation_revision, manual);
        observation["fetching"] = json!(false);
        let settled = resources.begin_refresh("page", false).unwrap();
        resources.complete_query("page", settled, Ok(observation));
        assert_eq!(
            resources.get("page").unwrap().presentation_revision,
            settled
        );
        assert!(!resources.get("page").unwrap().manual_pending);
    }

    #[test]
    fn identical_results_keep_projection_identity_but_advance_completion() {
        let mut resources = Resources::default();
        let first = resources.begin("snapshot").unwrap();
        resources.complete("snapshot", first, Ok(json!([1, 2, 3])));
        let previous = resources.get("snapshot").unwrap().data_revision;
        let second = resources.begin("snapshot").unwrap();
        resources.complete("snapshot", second, Ok(json!([1, 2, 3])));
        let row = resources.get("snapshot").unwrap();
        assert_eq!(row.data_revision, previous);
        assert_eq!(row.revision, second);
        let third = resources.begin("snapshot").unwrap();
        resources.complete("snapshot", third, Ok(json!([1, 2, 4])));
        assert_eq!(resources.get("snapshot").unwrap().data_revision, third);
    }

    #[test]
    fn refetching_without_data_keeps_the_last_reading_and_its_age() {
        let mut resources = Resources::default();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let reading = json!({"data":[8080],"fetching":false,"error":null,"stale":false,"updatedAt":now - 60_000});
        let first = resources.begin("page").unwrap();
        assert!(resources.complete_query("page", first, Ok(reading)));
        let refetch =
            json!({"data":null,"fetching":true,"error":null,"stale":false,"updatedAt":null});
        let second = resources.begin_refresh("page", false).unwrap();
        assert!(resources.complete_query("page", second, Ok(refetch)));
        let row = resources.get("page").unwrap();
        assert_eq!(row.value, json!([8080]));
        assert!(row.updated_at.is_some());
        assert!(!row.pending);
        // A settled empty reading is real data and replaces the old one.
        let empty =
            json!({"data":null,"fetching":false,"error":null,"stale":false,"updatedAt":now});
        let third = resources.begin_refresh("page", false).unwrap();
        assert!(resources.complete_query("page", third, Ok(empty)));
        assert!(resources.get("page").unwrap().value.is_null());
    }

    #[test]
    fn query_observations_keep_data_age_and_revision_and_reject_old_routes() {
        let mut resources = Resources::default();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let observation = json!({"data":[8080],"fetching":false,"error":null,"stale":false,"updatedAt":now - 60_000});
        let first = resources.begin("page").unwrap();
        assert!(resources.complete_query("page", first, Ok(observation.clone())));
        let revision = resources.get("page").unwrap().revision;
        let second = resources.begin("page").unwrap();
        assert!(resources.complete_query("page", second, Ok(observation.clone())));
        let row = resources.get("page").unwrap();
        assert_eq!(row.revision, revision);
        assert!(row.updated_at.unwrap().elapsed() >= Duration::from_secs(59));
        let mut stale = observation.clone();
        stale["error"] = json!("offline");
        stale["stale"] = json!(true);
        let third = resources.begin("page").unwrap();
        resources.complete_query("page", third, Ok(stale));
        assert_eq!(resources.get("page").unwrap().value, json!([8080]));
        assert_eq!(
            resources.get("page").unwrap().error.as_deref(),
            Some("offline")
        );
        let late = resources.begin("page").unwrap();
        let saved = resources.take_cached(&["page"]);
        resources.restore(saved);
        assert!(!resources.complete_query("page", late, Ok(observation)));
    }

    #[test]
    fn query_manual_activity_survives_observation_polls_until_remote_completion() {
        let mut resources = Resources::default();
        let fetching =
            json!({"data":[80],"fetching":true,"error":null,"stale":true,"updatedAt":null});
        let manual = resources.begin_refresh("page", true).unwrap();
        resources.complete_query("page", manual, Ok(fetching.clone()));
        assert!(!resources.get("page").unwrap().pending);
        assert!(resources.get("page").unwrap().manual_pending);
        let poll = resources.begin_refresh("page", false).unwrap();
        resources.complete_query("page", poll, Ok(fetching.clone()));
        assert!(resources.get("page").unwrap().manual_pending);
        let poll = resources.begin_refresh("page", false).unwrap();
        let mut ready = fetching;
        ready["fetching"] = json!(false);
        resources.complete_query("page", poll, Ok(ready));
        assert!(!resources.get("page").unwrap().manual_pending);
    }

    #[test]
    fn suspension_exclusions_keep_exact_pending_shared_keys_active() {
        let mut state = Resources::default();
        let receipts = state.begin_manual("git-receipts").unwrap();
        let log = state.begin("git-log").unwrap();
        let list = state.begin("git-list").unwrap();
        state.complete("git-list", list, Ok(json!(["main"])));
        let descendant = state.begin("git-log-events").unwrap();
        state.complete("git-log-events", descendant, Ok(json!("view-local")));

        let group = state.take_prefix_except("git-", &["git-receipts", "git-log"]);
        assert!(!group.had_pending(), "excluded reads were not suspended");
        assert!(state.get("git-receipts").unwrap().pending);
        assert!(state.get("git-receipts").unwrap().manual_pending);
        assert!(state.get("git-log").unwrap().pending);
        assert!(state.get("git-list").is_none());
        assert!(
            state.get("git-log-events").is_none(),
            "exclusions match exact keys only"
        );
        assert!(state.complete("git-receipts", receipts, Ok(json!(["receipt"]))));
        assert!(state.complete("git-log", log, Ok(json!("shared log"))));

        state.restore(group);
        assert_eq!(state.get("git-receipts").unwrap().value, json!(["receipt"]));
        assert_eq!(state.get("git-log").unwrap().value, json!("shared log"));
        assert_eq!(state.get("git-list").unwrap().value, json!(["main"]));
        assert_eq!(
            state.get("git-log-events").unwrap().value,
            json!("view-local")
        );
    }

    #[test]
    fn suspended_prefix_drops_pending_reads_and_preserves_unrelated_resources() {
        let mut state = Resources::default();
        let listed = state.begin("git-list").unwrap();
        state.complete("git-list", listed, Ok(json!(["main"])));
        let pending = state.begin_manual("git-details").unwrap();
        let refreshing = state.begin_manual("git-list").unwrap();
        let shell = state.begin("snapshot").unwrap();
        let other = state.begin("integration:server").unwrap();

        let group = state.take_prefix("git-");
        assert!(group.had_pending());
        assert!(state.get("git-list").is_none());
        assert!(state.get("git-details").is_none());
        assert!(state.get("snapshot").unwrap().pending);
        assert!(state.get("integration:server").unwrap().pending);
        assert!(!state.complete("git-details", pending, Ok(json!("late"))));
        state.restore(group);
        assert!(
            state.get("git-list").is_none(),
            "a stale value attached to a pending refresh is also dropped"
        );
        assert!(state.get("git-details").is_none());

        let next = state.begin("git-list").unwrap();
        assert!(next > other);
        assert!(!state.complete("git-list", refreshing, Ok(json!("late refresh"))));
        assert!(state.complete("snapshot", shell, Ok(json!("current shell"))));
        assert!(state.complete("integration:server", other, Ok(json!("completed"))));
        assert!(state.complete("git-list", next, Ok(json!(["main", "topic"]))));
    }

    #[test]
    fn suspended_settled_values_errors_timestamps_and_poll_state_round_trip() {
        let mut state = Resources::default();
        let initial = Instant::now();
        let successful = state.begin("git-list").unwrap();
        state.complete_at("git-list", successful, Ok(json!(["main"])), initial);
        let cached = state.begin("git-status").unwrap();
        state.complete_at("git-status", cached, Ok(json!({"clean":true})), initial);
        let failed_at = initial + Duration::from_secs(10);
        let failed = state.begin_manual("git-status").unwrap();
        state.complete_at("git-status", failed, Err("offline".into()), failed_at);
        let never_succeeded = state.begin("git-remotes").unwrap();
        state.complete_at(
            "git-remotes",
            never_succeeded,
            Err("unavailable".into()),
            failed_at,
        );
        let poll_checked = initial + Duration::from_secs(30);
        assert!(!state.take_poll("git-list", poll_checked, Duration::from_secs(20), false));

        let group = state.take_prefix("git-");
        assert!(!group.had_pending());
        state.restore(group);
        let list = state.get("git-list").unwrap();
        assert_eq!(list.value, json!(["main"]));
        assert_eq!(list.error, None);
        assert_eq!(list.revision, successful);
        assert_eq!(list.updated_at, Some(initial));
        assert_eq!(list.finished_at, Some(initial));
        assert_eq!(list.poll_checked_at, Some(poll_checked));
        assert!(!list.pending && !list.manual_pending);
        let status = state.get("git-status").unwrap();
        assert_eq!(status.value, json!({"clean":true}));
        assert_eq!(status.error.as_deref(), Some("offline"));
        assert_eq!(status.revision, cached);
        assert_eq!(status.updated_at, Some(initial));
        assert_eq!(status.finished_at, Some(failed_at));
        assert!(!status.pending && !status.manual_pending);
        let remotes = state.get("git-remotes").unwrap();
        assert!(remotes.value.is_null());
        assert_eq!(remotes.error.as_deref(), Some("unavailable"));
        assert_eq!(remotes.updated_at, None);
        assert_eq!(remotes.finished_at, Some(failed_at));
        assert!(!state.complete("git-list", successful, Ok(json!("duplicate"))));
    }

    #[test]
    fn restoring_does_not_overwrite_a_newer_pending_or_completed_request() {
        for finish_newer in [false, true] {
            let mut state = Resources::default();
            let old = state.begin("git-list").unwrap();
            state.complete("git-list", old, Ok(json!("old view")));
            let group = state.take_prefix("git-");
            let current = state.begin_manual("git-list").unwrap();
            if finish_newer {
                state.complete("git-list", current, Ok(json!("new view")));
            }
            state.restore(group);
            let row = state.get("git-list").unwrap();
            assert_eq!(row.generation, current);
            assert_eq!(row.pending, !finish_newer);
            assert_eq!(row.manual_pending, !finish_newer);
            assert!(!state.complete("git-list", old, Ok(json!("delayed old view"))));
            if !finish_newer {
                assert!(state.complete("git-list", current, Ok(json!("new view"))));
            }
            assert_eq!(state.get("git-list").unwrap().value, json!("new view"));
        }
    }

    #[test]
    fn restored_generation_watermark_includes_abandoned_reads() {
        let mut original = Resources::default();
        let saved = original.begin("git-list").unwrap();
        original.complete("git-list", saved, Ok(json!("saved")));
        let abandoned = original.begin("git-details").unwrap();
        let group = original.take_prefix("git-");
        let mut replacement = Resources::default();
        replacement.restore(group);
        let current = replacement.begin("git-details").unwrap();
        assert!(current > abandoned);
        assert!(!replacement.complete("git-details", abandoned, Ok(json!("abandoned"))));
        assert!(replacement.complete("git-details", current, Ok(json!("current"))));
    }

    #[test]
    fn repeated_view_switches_preserve_each_cache_without_reusing_generations() {
        let mut state = Resources::default();
        let first = state.begin("git-list").unwrap();
        state.complete("git-list", first, Ok(json!("changes")));
        let mut changes = state.take_prefix("git-");
        let second = state.begin("git-list").unwrap();
        state.complete("git-list", second, Ok(json!("history")));
        let mut history = state.take_prefix("git-");
        let shell = state.begin("snapshot").unwrap();
        for _ in 0..3 {
            state.restore(changes);
            assert_eq!(state.get("git-list").unwrap().value, json!("changes"));
            changes = state.take_prefix("git-");
            state.restore(history);
            assert_eq!(state.get("git-list").unwrap().value, json!("history"));
            history = state.take_prefix("git-");
            assert!(!changes.had_pending() && !history.had_pending());
            assert!(state.get("snapshot").unwrap().pending);
        }
        state.restore(changes);
        let current = state.begin("git-list").unwrap();
        assert!(current > shell);
        assert!(!state.complete("git-list", first, Ok(json!("late changes"))));
        assert!(!state.complete("git-list", second, Ok(json!("late history"))));
        assert!(state.complete("git-list", current, Ok(json!("refreshed changes"))));
        assert!(state.complete("snapshot", shell, Ok(json!("shell"))));
    }

    #[test]
    fn hidden_intervals_are_skipped_without_refetching_on_visibility() {
        let mut state = Resources::default();
        let now = Instant::now();
        let interval = Duration::from_secs(30);
        let generation = state.begin_refresh("ports", false).unwrap();
        state.complete_at("ports", generation, Ok(json!([])), now);
        assert!(!state.take_poll("ports", now + interval, interval, false));
        assert!(!state.take_poll(
            "ports",
            now + interval + Duration::from_secs(1),
            interval,
            true
        ));
        assert!(state.take_poll("ports", now + interval * 2, interval, true));
        assert!(!state.take_poll("ports", now + interval * 2, interval, true));
        let next = state.begin_refresh("ports", false).unwrap();
        let completed = now + interval * 2 + Duration::from_secs(2);
        state.complete_at("ports", next, Err("offline".into()), completed);
        assert!(!state.take_poll(
            "ports",
            completed + interval - Duration::from_secs(1),
            interval,
            true
        ));
        assert!(state.take_poll("ports", completed + interval, interval, true));
    }

    #[test]
    fn refresh_keeps_cached_error_until_success_but_clears_initial_failure() {
        let mut state = Resources::default();
        let first = state.begin_refresh("ports", false).unwrap();
        state.complete("ports", first, Err("offline".into()));
        let second = state.begin_refresh("ports", true).unwrap();
        assert!(state.get("ports").unwrap().error.is_none());
        state.complete("ports", second, Ok(json!([8080])));
        let success = state.get("ports").unwrap().updated_at;
        let third = state.begin_refresh("ports", false).unwrap();
        state.complete("ports", third, Err("offline again".into()));
        let retry = state.begin_refresh("ports", false).unwrap();
        assert!(state.begin_refresh("ports", true).is_none());
        let ports = state.get("ports").unwrap();
        assert_eq!(ports.error.as_deref(), Some("offline again"));
        assert_eq!(ports.updated_at, success);
        assert!(ports.manual_pending);
        assert_eq!(ports.value, json!([8080]));
        state.complete("ports", retry, Ok(json!([8080, 9000])));
        assert!(state.get("ports").unwrap().error.is_none());
        assert!(!state.get("ports").unwrap().manual_pending);
    }

    #[test]
    fn manual_refresh_coalesces_with_background_read_until_completion() {
        let mut state = Resources::default();
        let generation = state.begin("ports").unwrap();
        assert!(!state.get("ports").unwrap().manual_pending);

        assert_eq!(state.begin_manual("ports"), None);
        assert_eq!(state.begin_manual("ports"), None);
        assert_eq!(state.begin("ports"), None);
        assert!(state.get("ports").unwrap().manual_pending);

        assert!(state.complete("ports", generation, Ok(json!([8080]))));
        let ports = state.get("ports").unwrap();
        assert!(!ports.pending);
        assert!(!ports.manual_pending);

        state.begin("ports").unwrap();
        assert!(!state.get("ports").unwrap().manual_pending);
    }

    #[test]
    fn manual_failure_settles_without_changing_success_timestamp_or_value() {
        let mut state = Resources::default();
        let updated = Instant::now();
        let generation = state.begin("ports").unwrap();
        assert!(state.complete_at("ports", generation, Ok(json!([8080])), updated));

        let generation = state.begin_manual("ports").unwrap();
        let ports = state.get("ports").unwrap();
        assert!(ports.pending);
        assert!(ports.manual_pending);
        assert_eq!(ports.value, json!([8080]));
        let failed = updated + Duration::from_secs(30);
        assert!(state.complete_at("ports", generation, Err("offline".into()), failed));

        let ports = state.get("ports").unwrap();
        assert!(!ports.pending);
        assert!(!ports.manual_pending);
        assert_eq!(ports.value, json!([8080]));
        assert_eq!(ports.updated_at, Some(updated));
        assert_eq!(ports.finished_at, Some(failed));
        assert_eq!(ports.error.as_deref(), Some("offline"));
        assert!(!ports.poll_due(failed, Duration::from_secs(30), true));
    }

    #[test]
    fn initial_failure_has_no_success_timestamp() {
        let mut state = Resources::default();
        let generation = state.begin_manual("ports").unwrap();
        let failed = Instant::now();
        assert!(state.complete_at("ports", generation, Err("offline".into()), failed));
        let ports = state.get("ports").unwrap();
        assert_eq!(ports.updated_at, None);
        assert_eq!(ports.finished_at, Some(failed));
        assert!(!ports.manual_pending);
    }

    #[test]
    fn stale_generation_cannot_settle_a_new_manual_refresh() {
        let mut state = Resources::default();
        let old = state.begin("ports").unwrap();
        state.invalidate("ports");
        let current = state.begin_manual("ports").unwrap();
        let now = Instant::now();
        assert!(!state.complete_at("ports", old, Err("stale failure".into()), now));
        let ports = state.get("ports").unwrap();
        assert!(ports.pending);
        assert!(ports.manual_pending);
        assert_eq!(ports.finished_at, None);
        assert_eq!(ports.updated_at, None);
        assert_eq!(ports.error, None);
        assert!(state.complete_at("ports", current, Ok(json!([8080])), now));
    }

    #[test]
    fn duplicate_completion_cannot_change_a_settled_result() {
        let mut state = Resources::default();
        let generation = state.begin_manual("ports").unwrap();
        let now = Instant::now();
        assert!(state.complete_at("ports", generation, Ok(json!([8080])), now));
        assert!(!state.complete_at(
            "ports",
            generation,
            Err("duplicate failure".into()),
            now + Duration::from_secs(1),
        ));
        let ports = state.get("ports").unwrap();
        assert_eq!(ports.value, json!([8080]));
        assert_eq!(ports.error, None);
        assert_eq!(ports.finished_at, Some(now));
        assert_eq!(ports.updated_at, Some(now));
    }

    #[test]
    fn polling_requires_visible_settled_resource_and_full_interval() {
        let mut state = Resources::default();
        let now = Instant::now();
        let interval = Duration::from_secs(30);
        assert!(!Resource::default().poll_due(now, interval, true));

        let generation = state.begin("ports").unwrap();
        assert!(!state.get("ports").unwrap().poll_due(now, interval, true));
        assert!(state.complete_at("ports", generation, Ok(json!([])), now));
        let due = now + interval;
        let ports = state.get("ports").unwrap();
        assert!(!ports.poll_due(now - Duration::from_secs(1), interval, true));
        assert!(!ports.poll_due(due - Duration::from_nanos(1), interval, true));
        assert!(!ports.poll_due(due, interval, false));
        assert!(ports.poll_due(due, interval, true));

        state.begin("ports").unwrap();
        assert!(!state.get("ports").unwrap().poll_due(due, interval, true));
    }

    #[test]
    fn value_revision_changes_only_when_data_is_replaced() {
        let mut resources = Resources::default();
        let first = resources.begin("history").unwrap();
        assert_eq!(resources.get("history").unwrap().revision, 0);
        resources.complete("history", first, Ok(json!([1])));
        assert_eq!(resources.get("history").unwrap().revision, first);
        let second = resources.begin("history").unwrap();
        assert_eq!(resources.get("history").unwrap().revision, first);
        resources.complete("history", second, Err("offline".into()));
        assert_eq!(resources.get("history").unwrap().revision, first);
        let third = resources.begin("history").unwrap();
        assert!(!resources.complete("history", second, Ok(json!([2]))));
        resources.complete("history", third, Ok(json!([3])));
        assert_eq!(resources.get("history").unwrap().revision, third);
    }
    #[test]
    fn library_refresh_discards_old_results_without_touching_unrelated_work() {
        let mut state = Resources::default();
        let old = state.begin("project-status:one").unwrap();
        let other = state.begin("project-created-save").unwrap();
        state.invalidate_prefix("project-status:");
        let new = state.begin("project-status:one").unwrap();
        assert!(!state.complete("project-status:one", old, Ok(json!("stale clean"))));
        assert!(state.complete("project-created-save", other, Ok(json!("saved"))));
        assert!(state.complete("project-status:one", new, Ok(json!("current changes"))));
        assert_eq!(
            state.get("project-status:one").unwrap().value,
            "current changes"
        );
    }
    #[test]
    fn changing_selection_supersedes_pending_read_without_clearing_other_resources() {
        let mut state = Resources::default();
        let old = state.begin("detail").unwrap();
        let list = state.begin("list").unwrap();
        state.invalidate("detail");
        let new = state.begin("detail").unwrap();
        assert!(!state.complete("detail", old, Ok(json!("wrong commit"))));
        assert!(state.complete("list", list, Ok(json!([1, 2]))));
        assert!(state.complete("detail", new, Ok(json!("selected commit"))));
        assert_eq!(state.get("detail").unwrap().value, "selected commit");
    }
    #[test]
    fn server_operations_survive_navigation_without_blocking_other_servers() {
        let mut state = Resources::default();
        let a = state.begin("server-test:a").unwrap();
        let save = state.begin("server-save").unwrap();
        state.clear();
        let b = state.begin("server-test:b").unwrap();
        assert!(state.begin("server-test:a").is_none());
        assert!(state.begin("server-save").is_none());
        assert!(state.complete("server-test:b", b, Ok(json!(true))));
        assert!(state.get("server-test:a").unwrap().pending);
        assert!(state.complete("server-test:a", a, Err("unreachable".into())));
        assert!(state.complete("server-save", save, Ok(Value::Null)));
        assert!(state.get("server-test:b").unwrap().error.is_none());
    }
    #[test]
    fn server_editor_keeps_completed_results_until_explicit_cleanup() {
        for result in [Ok(Value::Null), Err("Cannot persist the profile".into())] {
            let mut state = Resources::default();
            let save = state.begin("server-save").unwrap();
            let keys = state.begin("server-form-agent-keys").unwrap();
            state.complete("server-save", save, result.clone());
            state.clear();
            assert_eq!(state.get("server-save").unwrap().error, result.err());
            assert!(state.complete("server-form-agent-keys", keys, Ok(json!({"keys":[]}))));
            state.clear();
            assert!(!state.get("server-form-agent-keys").unwrap().pending);
            state.invalidate("server-save");
            state.invalidate("server-form-agent-keys");
            assert!(state.get("server-save").is_none());
            assert!(state.get("server-form-agent-keys").is_none());
        }
    }
    #[test]
    fn integration_write_survives_navigation_and_supersedes_old_snapshot() {
        let mut state = Resources::default();
        let write = state.begin("integration:a").unwrap();
        let stale_snapshot = state.begin("snapshot").unwrap();
        state.clear();
        assert!(state.begin("integration:a").is_none());
        assert!(state.begin("integration:b").is_some());
        assert!(state.complete("integration:a", write, Ok(Value::Null)));
        state.invalidate("snapshot");
        let fresh_snapshot = state.begin("snapshot").unwrap();
        assert!(!state.complete("snapshot", stale_snapshot, Ok(json!("stale"))));
        assert!(state.complete("snapshot", fresh_snapshot, Ok(json!("confirmed"))));
    }
    #[test]
    fn older_selection_cannot_replace_newer_data() {
        let mut state = Resources::default();
        let old = state.begin("files").unwrap();
        state.clear();
        let current = state.begin("files").unwrap();
        assert!(!state.complete("files", old, Ok(json!("old"))));
        assert!(state.complete("files", current, Ok(json!("current"))));
        assert_eq!(state.get("files").unwrap().value, "current");
    }
    #[test]
    fn duplicate_reads_are_coalesced_and_failures_retain_last_data() {
        let mut state = Resources::default();
        let first = state.begin("overview").unwrap();
        assert_eq!(state.begin("overview"), None);
        state.complete("overview", first, Ok(json!(42)));
        let second = state.begin("overview").unwrap();
        state.complete("overview", second, Err("offline".into()));
        assert_eq!(state.get("overview").unwrap().value, 42);
        assert_eq!(
            state.get("overview").unwrap().error.as_deref(),
            Some("offline")
        );
    }
}
#[test]
fn structured_errors_follow_request_generations_and_clear_on_retry() {
    let mut resources = Resources::default();
    let stale = crate::request_error::RequestError::from_response(
        &serde_json::json!({"code":"STALE_ENTRY","message":"Changed"}),
    );
    let old = resources.begin("diff").unwrap();
    resources.invalidate("diff");
    let current = resources.begin("diff").unwrap();
    assert!(!resources.complete_detailed("diff", old, Err(stale.clone())));
    assert!(resources.get("diff").unwrap().error_code.is_none());
    assert!(resources.complete_detailed("diff", current, Err(stale)));
    assert_eq!(
        resources.get("diff").unwrap().error_code.as_deref(),
        Some("STALE_ENTRY")
    );
    let retry = resources.begin("diff").unwrap();
    assert!(resources.get("diff").unwrap().error_code.is_none());
    resources.complete("diff", retry, Err("Network failure".into()));
    assert!(resources.get("diff").unwrap().error_code.is_none());
    let retry = resources.begin("diff").unwrap();
    resources.complete_detailed("diff", retry, Ok(serde_json::json!({})));
    assert!(resources.get("diff").unwrap().error.is_none());
}
