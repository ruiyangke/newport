//! Overview observations belong to each server session. The application only
//! renews background demand; Query owns caching, refreshes and cancellation.
use crate::{
    cockpit,
    manager::Shared,
    metrics_db::{self, MetricsDb},
    model::Server,
    query::{Policy, Query},
    server_session::ServerSession,
};
use serde_json::Value;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;

const CHANGED: &str = "Server connection changed. Refresh again.";
const INTERVAL: Duration = Duration::from_secs(3);

// Consent/setup changes must discard pre-install failures without retiring the
// link. Cosmetic edits and integration switches preserve the metrics cache.
pub(crate) fn same_sampling_profile(a: &Server, b: &Server) -> bool {
    a.same_connection(b)
        && a.agent_install_approved == b.agent_install_approved
        && a.setup_complete == b.setup_complete
}

#[derive(Default)]
pub(crate) struct Slot(Mutex<Option<Arc<Reading>>>);
struct Reading {
    profile: Server,
    query: Query<Arc<Value>>,
    cancelled: Arc<AtomicBool>,
    retry: Arc<AtomicBool>,
}
impl Reading {
    fn close(&self) {
        // Also fence history recording by loaders waiting on the history gate.
        // Closing the query alone only fences its cached result, not DB writes.
        self.cancelled.store(true, Ordering::Release);
        self.query.close();
    }
}
impl Slot {
    pub(crate) fn clear(&self) {
        if let Some(reading) = self.0.lock().unwrap().take() {
            reading.close();
        }
    }
    fn get_or_insert(&self, profile: &Server, create: impl FnOnce() -> Reading) -> Arc<Reading> {
        let mut current = self.0.lock().unwrap();
        if let Some(reading) = current.as_ref() {
            if same_sampling_profile(&reading.profile, profile) {
                return reading.clone();
            }
            reading.close();
        }
        let reading = Arc::new(create());
        *current = Some(reading.clone());
        reading
    }
}

fn policy() -> Policy {
    Policy {
        stale_after: INTERVAL,
        retry_after: INTERVAL,
        timeout: Duration::from_secs(40),
        // The background owner renews every second, including hidden pages.
        observation_lease: Duration::from_secs(5),
    }
}

fn query(state: &Shared, db: &MetricsDb, id: Uuid) -> Result<Arc<Reading>, String> {
    let (profile, owner) = state.current(id)?;
    let reading = owner.metrics.get_or_insert(&profile, || {
        let cancelled = Arc::new(AtomicBool::new(false));
        let retry = Arc::new(AtomicBool::new(false));
        let weak_state = Arc::downgrade(state);
        let weak_owner = Arc::downgrade(&owner);
        let source_profile = profile.clone();
        let db = db.clone();
        let token = cancelled.clone();
        let retry_request = retry.clone();
        Reading {
            profile: profile.clone(),
            cancelled,
            retry,
            query: Query::new(policy(), move || {
                let (state, owner, profile, db, token, retry) = (
                    weak_state.clone(),
                    weak_owner.clone(),
                    source_profile.clone(),
                    db.clone(),
                    token.clone(),
                    retry_request.clone(),
                );
                async move {
                    let state = state.upgrade().ok_or("Application closed.")?;
                    let owner = owner.upgrade().ok_or(CHANGED)?;
                    validate(&state, &owner, &profile, &token)?;
                    if retry.swap(false, Ordering::AcqRel) {
                        crate::monitor::retry_overview(&owner).await;
                    }
                    let data = cockpit::collect(&owner, cockpit::Section::Overview).await?;
                    record(&state, &db, &owner, &profile, &token, data)
                        .await
                        .map(Arc::new)
                }
            }),
        }
    });
    // Publication and lookup have separate short locks. Validate after dropping
    // the slot lock, avoiding a registry/feature lock-order inversion.
    validate(state, &owner, &profile, &reading.cancelled)?;
    Ok(reading)
}

fn validate(
    state: &Shared,
    owner: &Arc<ServerSession>,
    profile: &Server,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let (current, current_owner) = state.current(profile.id)?;
    if cancelled.load(Ordering::Acquire)
        || owner.is_revoked()
        || !Arc::ptr_eq(owner, &current_owner)
        || !same_sampling_profile(&current, profile)
    {
        return Err(CHANGED.into());
    }
    Ok(())
}

async fn record(
    state: &Shared,
    db: &MetricsDb,
    owner: &Arc<ServerSession>,
    profile: &Server,
    cancelled: &AtomicBool,
    mut data: Value,
) -> Result<Value, String> {
    let sampled_at = metrics_db::now_ms();
    // Deletion/recreation coordinates only this server. Explicit clear-all
    // history takes the exclusive maintenance guard, not ordinary UI reads.
    let _history = crate::manager::server_history_guard(state, profile.id).await;
    validate(state, owner, profile, cancelled)?;
    data["sampledAt"] = sampled_at.into();
    if let Err(error) = db.record(profile, sampled_at, &data).await {
        data["historyError"] =
            format!("Live metrics loaded, but local history could not be saved: {error}").into();
    }
    validate(state, owner, profile, cancelled)?;
    Ok(data)
}

fn cached(reading: &Reading) -> Option<Result<Value, String>> {
    let snapshot = reading.query.snapshot();
    if let Some(data) = snapshot.data {
        let mut data = (*data).clone();
        if let Some(error) = snapshot.error {
            data["collectionError"] = error.into();
        }
        Some(Ok(data))
    } else {
        snapshot.error.map(Err)
    }
}

/// Navigation uses the last observation immediately; the query refreshes stale
/// data in the background. Explicit refresh reports its fresh result or error.
pub async fn read(state: &Shared, db: &MetricsDb, id: Uuid, force: bool) -> Result<Value, String> {
    let reading = query(state, db, id)?;
    let result = if force {
        reading.retry.store(true, Ordering::Release);
        reading.query.refresh().await.map(|data| (*data).clone())
    } else {
        reading.query.observe(false);
        match cached(&reading) {
            Some(result) => result,
            None => reading.query.read().await.map(|data| (*data).clone()),
        }
    };
    if reading.cancelled.load(Ordering::Acquire) {
        return Err(CHANGED.into());
    }
    result
}

pub fn clear_cache(state: &Shared) {
    for owner in state.sessions.all() {
        owner.metrics.clear();
    }
}

struct Background(Shared);
impl Drop for Background {
    fn drop(&mut self) {
        clear_cache(&self.0);
    }
}

pub async fn run(state: Shared, db: MetricsDb) {
    let _lifetime = Background(state.clone());
    let mut renew = tokio::time::interval(Duration::from_secs(1));
    renew.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        renew.tick().await;
        for (profile, _) in state.sessions.profiles() {
            if let Ok(reading) = query(&state, &db, profile.id) {
                reading.query.observe(false);
            }
        }
    }
}

#[cfg(test)]
mod tests;
