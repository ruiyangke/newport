//! Signs this computer in to a Newport account and keeps its license fresh.
//!
//! Sign-in follows RFC 8252: the browser approves on the account site, which
//! hands a one-time code to a listener on 127.0.0.1; the code is exchanged with
//! a PKCE verifier for a device credential. The credential stays in the
//! backend's encrypted account snapshot and never reaches the UI.
mod loopback;
#[cfg(test)]
mod tests;

use crate::{
    desktop_integration::Desktop,
    profile::{Store, account::Record},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use newport_platform::account::{self as platform, Api, Failure};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use url::Url;
use zeroize::Zeroizing;

const PORTAL: &str = "https://account.newport.app";
/// Development portal override, honoured only with an explicit data directory.
const PORTAL_ENV: &str = "NEWPORT_ACCOUNT_URL";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const FIRST_REFRESH: Duration = Duration::from_secs(5);
const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the daily refresh checks the wall clock. Monotonic timers stop
/// while the Mac sleeps, so a 24-hour timer alone could fire days late.
const REFRESH_CHECK: Duration = Duration::from_secs(15 * 60);
/// Refresh early when the token ends within this long.
const REFRESH_BEFORE_END: i64 = 2 * 24 * 60 * 60;
/// A launch waits this long for a fresh token when the saved one has run out.
const LAUNCH_REFRESH_WAIT: Duration = Duration::from_secs(10);
/// A launch refreshes first when the saved token ends within this long.
const LAUNCH_REFRESH_BEFORE_END: i64 = 60 * 60;
/// Launches start a background refresh for a day-old token at most this often,
/// so a Mac clock running ahead doesn't make every launch ask.
const LAUNCH_REFRESH_GAP: Duration = Duration::from_secs(60 * 60);
/// Warn about being offline when the token ends within this long.
const OFFLINE_WARNING: i64 = 3 * 24 * 60 * 60;
/// The verifier's clock drift allowance (`newport_remote_license::LEEWAY`).
const LEEWAY: i64 = 5 * 60;
/// Window activations and menu opens within this gap share one request.
const PROMPT_REFRESH_GAP: Duration = Duration::from_secs(15);
/// After Billing opens: check every half minute for ten minutes.
const WATCH_INTERVAL: Duration = Duration::from_secs(30);
const WATCH_CHECKS: u32 = 20;
/// License fields the UI may show. The token stays in the backend.
const LICENSE_FIELDS: [&str; 8] = [
    "state",
    "plan",
    "seat",
    "validUntil",
    "renewsAt",
    "endsAt",
    "trialUsed",
    "message",
];

#[derive(Clone)]
pub struct Account(Arc<Inner>);

struct Inner {
    store: Arc<Store>,
    desktop: Desktop,
    portal: Url,
    api: Result<Api, String>,
    app_version: String,
    state: Mutex<State>,
    /// Serializes reads and writes of the saved sign-in with the API calls
    /// that decide them, so a refresh can't resurrect a signed-out record.
    storage: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    loaded: bool,
    session: Option<Record>,
    sign_in: Option<SignIn>,
    signing_out: bool,
    notice: Option<Notice>,
    notices: u64,
    attempts: u64,
    /// When the license was last asked for, so prompt refreshes combine.
    refreshed_at: Option<tokio::time::Instant>,
    /// Polls the license for a while after Billing opens in the browser.
    watch: Option<tokio::task::AbortHandle>,
    /// Grows whenever a different license token is saved, so the UI can renew
    /// running Remote sessions without ever seeing the token.
    license_revision: u64,
    /// The account site refused a trial as already used by this account; the
    /// page then offers an upgrade instead.
    trial_refused: bool,
    /// The account site refused a trial because this computer already used
    /// one, under any account.
    trial_refused_here: bool,
    /// The last refresh the account site answered, by the monotonic clock
    /// (was this token issued just now?) and by the wall clock (is it a day
    /// old, across sleep?).
    answered_at: Option<(tokio::time::Instant, i64)>,
    /// When a launch last started a background refresh.
    launch_refreshed_at: Option<tokio::time::Instant>,
    /// The offline warning was shown since the last answered refresh.
    offline_warned: bool,
}

struct SignIn {
    attempt: u64,
    url: Url,
    task: tokio::task::AbortHandle,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Notice {
    pub id: u64,
    pub kind: &'static str,
    pub message: String,
    /// The next step the toast offers: `manageComputers` or `tryAgain`.
    pub action: Option<&'static str>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// `loading`, `signedOut`, `waiting` or `signedIn`.
    pub status: &'static str,
    pub email: Option<String>,
    /// The name Google, GitHub or Apple gave the account, when the site sends one.
    pub name: Option<String>,
    pub license: Option<Value>,
    pub signing_out: bool,
    /// The latest outcome to show once; its id only grows.
    pub notice: Option<Notice>,
    /// Grows whenever a different license token is saved.
    pub license_revision: u64,
}

impl Account {
    pub fn new(store: Arc<Store>, desktop: Desktop, app_version: String) -> Self {
        Self::with_portal(store, desktop, app_version, portal())
    }

    fn with_portal(store: Arc<Store>, desktop: Desktop, app_version: String, portal: Url) -> Self {
        let api = Api::new(portal.clone(), &app_version);
        Self(Arc::new(Inner {
            store,
            desktop,
            portal,
            api,
            app_version,
            state: Mutex::new(State {
                // Seeded from the clock, so a restarted backend never reuses
                // a revision the UI already saw and renewals aren't missed.
                license_revision: u64::try_from(unix_now()).unwrap_or(0) * 1_000,
                ..State::default()
            }),
            storage: tokio::sync::Mutex::new(()),
        }))
    }

    pub fn status(&self) -> Status {
        let state = self.0.state.lock().unwrap();
        let license = state.session.as_ref().map(|record| {
            let mut fields = LICENSE_FIELDS
                .iter()
                .filter_map(|key| Some(((*key).to_owned(), record.license.get(*key)?.clone())))
                .collect::<serde_json::Map<_, _>>();
            if state.trial_refused {
                fields.insert("trialUsed".into(), Value::Bool(true));
            }
            if state.trial_refused_here {
                fields.insert("trialUsedHere".into(), Value::Bool(true));
            }
            Value::Object(fields)
        });
        let text = |key: &str| {
            state.session.as_ref().and_then(|record| {
                record
                    .license
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            })
        };
        Status {
            status: if !state.loaded {
                "loading"
            } else if state.session.is_some() {
                "signedIn"
            } else if state.sign_in.is_some() {
                "waiting"
            } else {
                "signedOut"
            },
            email: text("email"),
            name: text("name"),
            license,
            signing_out: state.signing_out,
            notice: state.notice.clone(),
            license_revision: state.license_revision,
        }
    }

    /// Loads the saved sign-in, then refreshes it at launch and whenever the
    /// last answered refresh is a day old by the wall clock, or the token ends
    /// within two days. Checking often by the wall clock covers sleep and a
    /// network that comes back; a failed refresh is retried at the next check.
    pub async fn maintain(self) {
        self.load().await;
        tokio::time::sleep(FIRST_REFRESH).await;
        self.refresh().await;
        loop {
            tokio::time::sleep(REFRESH_CHECK).await;
            if self.refresh_due(unix_now()) {
                self.refresh().await;
            }
        }
    }

    fn refresh_due(&self, now: i64) -> bool {
        let state = self.0.state.lock().unwrap();
        let Some(record) = &state.session else {
            return false;
        };
        let day_old = state
            .answered_at
            .is_none_or(|(_, at)| now - at >= REFRESH_INTERVAL.as_secs() as i64);
        let ending = token(&record.license)
            .and_then(|token| claim_time(token, "exp"))
            .is_some_and(|exp| exp - now < REFRESH_BEFORE_END);
        day_old || ending
    }

    async fn load(&self) {
        // Once loaded, never wait behind a refresh or sign-in that holds the
        // storage lock for a network call: a launch must not wait on the network.
        if self.0.state.lock().unwrap().loaded {
            return;
        }
        let _storage = self.0.storage.lock().await;
        if self.0.state.lock().unwrap().loaded {
            return;
        }
        let store = self.0.store.clone();
        let record = match tokio::task::spawn_blocking(move || store.account()).await {
            Ok(Ok(record)) => record,
            Ok(Err(error)) => {
                log::warn!("Cannot read the saved Newport sign-in: {error}");
                None
            }
            Err(error) => {
                log::warn!("Newport sign-in load task failed: {error}");
                None
            }
        };
        let mut state = self.0.state.lock().unwrap();
        if record
            .as_ref()
            .is_some_and(|record| token(&record.license).is_some())
        {
            state.license_revision += 1;
        }
        state.session = record;
        state.loaded = true;
    }

    /// Opens the account site to approve this computer. While a sign-in is
    /// waiting, this opens the same page again.
    pub async fn sign_in(&self) -> Result<(), String> {
        self.load().await;
        let url = {
            let state = self.0.state.lock().unwrap();
            if state.session.is_some() {
                return Err("This computer is already signed in.".into());
            }
            state.sign_in.as_ref().map(|sign_in| sign_in.url.clone())
        };
        if let Some(url) = url {
            return self.open(url).await;
        }
        self.0.api.as_ref().map_err(Clone::clone)?;
        let listener = loopback::Listener::bind()
            .await
            .map_err(|error| format!("Couldn’t start signing in: {error}"))?;
        let verifier = Zeroizing::new(random_token());
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state_token = random_token();
        let mut url = self.0.portal.join("/connect").map_err(|e| e.to_string())?;
        url.query_pairs_mut()
            .append_pair("challenge", &challenge)
            .append_pair("port", &listener.port().to_string())
            .append_pair("state", &state_token)
            // When this computer stops waiting, so the page can say a link
            // left open too long has expired rather than fail to connect.
            .append_pair(
                "until",
                &(unix_now() + SIGN_IN_TIMEOUT.as_secs() as i64).to_string(),
            );
        let attempt = {
            let mut state = self.0.state.lock().unwrap();
            if state.session.is_some() || state.sign_in.is_some() {
                // Another request started first; it owns the browser.
                return Ok(());
            }
            state.attempts += 1;
            let attempt = state.attempts;
            let task = tokio::spawn(self.clone().wait_for_browser(
                attempt,
                listener,
                verifier,
                state_token,
            ));
            state.sign_in = Some(SignIn {
                attempt,
                url: url.clone(),
                task: task.abort_handle(),
            });
            attempt
        };
        if let Err(error) = self.open(url).await {
            self.end_sign_in(attempt);
            return Err(error);
        }
        Ok(())
    }

    pub fn cancel_sign_in(&self) {
        let mut state = self.0.state.lock().unwrap();
        if let Some(sign_in) = state.sign_in.take() {
            sign_in.task.abort();
        }
    }

    pub async fn sign_out(&self) -> Result<(), String> {
        let _storage = self.0.storage.lock().await;
        let credential = {
            let mut state = self.0.state.lock().unwrap();
            let Some(record) = &state.session else {
                return Ok(());
            };
            let credential = Zeroizing::new(record.credential.clone());
            state.signing_out = true;
            credential
        };
        // Free the place on the account first; this computer signs out either way.
        let account = self.clone();
        let remote =
            blocking(move || account.api()?.delete("/api/v1/devices/self", &credential)).await;
        let store = self.0.store.clone();
        let local = tokio::task::spawn_blocking(move || store.remove_account())
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
        let mut state = self.0.state.lock().unwrap();
        state.signing_out = false;
        if let Err(error) = local {
            log::warn!("Cannot remove the saved Newport sign-in: {error}");
            notify(&mut state, "error", "Couldn’t sign out. Try again.");
            return Err(error);
        }
        state.session = None;
        state.trial_refused = false;
        match remote {
            Ok(()) | Err(Failure::Rejected { status: 401, .. }) => {
                notify(&mut state, "success", "Signed out of Newport.")
            }
            Err(error) => {
                log::warn!("Cannot sign this computer out of its Newport account: {error}");
                notify(
                    &mut state,
                    "warning",
                    "Signed out on this computer. Newport couldn’t reach your account, so sign it out there too.",
                )
            }
        }
        Ok(())
    }

    pub async fn open_portal(&self) -> Result<(), String> {
        self.open(self.0.portal.clone()).await
    }

    /// The account site's Billing page, where Newport Pro is bought.
    pub async fn open_billing(&self) -> Result<(), String> {
        let url = self.0.portal.join("/billing").map_err(|e| e.to_string())?;
        self.open(url).await?;
        self.watch_for_purchase();
        Ok(())
    }

    /// Asks the account site now, for a window that became active again or
    /// an account menu that opened. Requests within a few seconds combine.
    pub async fn refresh_now(&self) {
        let recent = self
            .0
            .state
            .lock()
            .unwrap()
            .refreshed_at
            .is_some_and(|at| at.elapsed() < PROMPT_REFRESH_GAP);
        if !recent {
            self.refresh().await;
        }
    }

    /// A purchase finishes in the browser while Newport may sit in the
    /// background, so check every half minute for a while, until the plan changes.
    fn watch_for_purchase(&self) {
        let account = self.clone();
        let task = tokio::spawn(async move {
            let before = account.licensed();
            for _ in 0..WATCH_CHECKS {
                tokio::time::sleep(WATCH_INTERVAL).await;
                account.refresh().await;
                if account.licensed() != before {
                    break;
                }
            }
        });
        if let Some(previous) = self
            .0
            .state
            .lock()
            .unwrap()
            .watch
            .replace(task.abort_handle())
        {
            previous.abort();
        }
    }

    /// `None` when signed out.
    fn licensed(&self) -> Option<bool> {
        let state = self.0.state.lock().unwrap();
        state
            .session
            .as_ref()
            .map(|record| grants_pro(&record.license))
    }

    /// This computer's Remote apps license, for Newport Remote: the signed
    /// token, the machine it names and the latest time the account site
    /// vouched for, so a client whose clock was set back can't take an ended
    /// lease for current. Answers from the saved license at once, so a launch
    /// never waits on the network or a vault save, and asks the account site in
    /// the background; requests within a few seconds combine.
    pub async fn remote_license(&self) -> Result<RemoteLicense, LicenseRefusal> {
        self.load().await;
        let machine = machine();
        // A token that has run out, or nearly, is no use to a launch: wait a
        // little for a fresh one, rather than answer with it.
        let ending = {
            let state = self.0.state.lock().unwrap();
            state.session.as_ref().is_some_and(|record| {
                grants_pro(&record.license)
                    && token(&record.license)
                        .and_then(|token| claim_time(token, "exp"))
                        .is_none_or(|exp| exp - effective_now(record) < LAUNCH_REFRESH_BEFORE_END)
            })
        };
        if ending {
            let _ = tokio::time::timeout(LAUNCH_REFRESH_WAIT, self.refresh_now()).await;
        }
        let mut state = self.0.state.lock().unwrap();
        // Otherwise ask the account site again only for a day-old token, in
        // the background and at most hourly. Every check-in issues a new
        // token, and renewing open sessions asks here too, so refreshing on
        // every call would never settle.
        let stale = state.session.as_ref().is_some_and(|record| {
            token(&record.license)
                .and_then(issued_at)
                .is_none_or(|issued| unix_now() - issued > REFRESH_INTERVAL.as_secs() as i64)
        });
        let throttled = state
            .launch_refreshed_at
            .is_some_and(|at| at.elapsed() < LAUNCH_REFRESH_GAP);
        if stale && !throttled {
            state.launch_refreshed_at = Some(tokio::time::Instant::now());
            let account = self.clone();
            tokio::spawn(async move { account.refresh_now().await });
        }
        let answered_recently = state
            .answered_at
            .is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(10 * 60));
        let Some(record) = state.session.as_ref() else {
            return Err(LicenseRefusal::new(
                "signed_out",
                "Remote apps are part of Newport Pro. Sign in to use them.",
            ));
        };
        let license = &record.license;
        if !grants_pro(license) {
            let field = |key: &str| license.get(key).and_then(Value::as_str);
            return Err(match (field("state"), field("seat")) {
                (Some("trial"), Some("free")) => LicenseRefusal::new(
                    "trial_elsewhere",
                    "Your free trial runs on the computer that started it.",
                ),
                (Some("active" | "grace"), Some("free")) => LicenseRefusal::new(
                    "free_seat",
                    license
                        .get("message")
                        .and_then(Value::as_str)
                        .filter(|message| !message.trim().is_empty())
                        .unwrap_or(
                            "Newport Pro is in use on your other computers. Move a Pro seat here to open remote apps on this one.",
                        ),
                ),
                _ => LicenseRefusal::new("free", "Remote apps are part of Newport Pro."),
            });
        }
        let Some(token) = token(license) else {
            return Err(LicenseRefusal::new(
                "unavailable",
                license
                    .get("message")
                    .and_then(Value::as_str)
                    .filter(|message| !message.trim().is_empty())
                    .unwrap_or(
                        "Newport doesn’t have this computer’s license yet. Try again in a moment.",
                    ),
            ));
        };
        // Still out of date after asking: either the Mac's clock is wrong (the
        // account site just answered with this token) or it couldn't be asked.
        if claim_time(token, "exp").is_some_and(|exp| effective_now(record) > exp + LEEWAY) {
            return Err(if answered_recently {
                LicenseRefusal::new(
                    "clock",
                    "This Mac’s clock looks wrong, so its Newport Pro license seems to have run out. Set the date and time automatically in System Settings, then try again.",
                )
            } else {
                LicenseRefusal::new(
                    "offline",
                    "Newport couldn’t check your plan online, and this computer’s license has run out. Connect to the internet, then try again.",
                )
            });
        }
        let machine = machine.map_err(|error| {
            log::warn!("Cannot read this computer's machine ID: {error}");
            LicenseRefusal::new("unavailable", "Newport couldn’t identify this computer.")
        })?;
        Ok(RemoteLicense {
            trusted_time: record
                .trusted_time
                .unwrap_or(0)
                .max(issued_at(token).unwrap_or(0)),
            expires_at: claim_time(token, "exp").unwrap_or(0),
            token: Zeroizing::new(token.to_owned()),
            machine,
        })
    }

    /// Starts this account's free trial on this computer. The account site's
    /// own wording explains a refusal, such as a trial already used.
    pub async fn start_trial(&self) -> Result<(), String> {
        self.load().await;
        let _storage = self.0.storage.lock().await;
        let Some(credential) = self
            .0
            .state
            .lock()
            .unwrap()
            .session
            .as_ref()
            .map(|record| Zeroizing::new(record.credential.clone()))
        else {
            return Err("Sign in to Newport first.".into());
        };
        let account = self.clone();
        let started = blocking(move || {
            let body = json!({ "machine": machine()? });
            account
                .api()?
                .post_dated("/api/v1/trial", Some(&credential), &body)
        })
        .await;
        // The app shows a refusal from the returned error, so no notice too.
        let failed = |message: String| Err(message);
        match started {
            Ok((value, date)) => {
                let Some(license) = value.get("license").filter(|license| license.is_object())
                else {
                    log::warn!("The trial response returned no license.");
                    return failed("Couldn’t start the free trial. Try again.".into());
                };
                self.keep_license(license.clone(), date).await;
                notify(
                    &mut self.0.state.lock().unwrap(),
                    "success",
                    "Your free trial has started.",
                );
                Ok(())
            }
            Err(Failure::Rejected { code, message, .. }) if !message.trim().is_empty() => {
                // A trial is once per account and once per computer; the
                // license only says the former, so remember which here.
                {
                    let mut state = self.0.state.lock().unwrap();
                    match code.as_str() {
                        "trial_used" => state.trial_refused = true,
                        "machine_trial_used" => state.trial_refused_here = true,
                        _ => {}
                    }
                }
                drop(_storage);
                self.settle_trial(message).await
            }
            Err(error) => {
                log::warn!("Cannot start the Newport trial: {error}");
                drop(_storage);
                self.settle_trial("Couldn’t start the free trial. Try again.".into())
                    .await
            }
        }
    }

    /// After a refused or failed trial request, asks the account site where
    /// things stand: the trial may have started anyway (a lost reply, a retry)
    /// or Pro may already run here. Then nothing failed, and one notice says
    /// so rather than an error followed by a notice.
    async fn settle_trial(&self, error: String) -> Result<(), String> {
        let notices = self.0.state.lock().unwrap().notices;
        self.refresh().await;
        let mut state = self.0.state.lock().unwrap();
        let pro_here = state
            .session
            .as_ref()
            .is_some_and(|record| grants_pro(&record.license));
        if !pro_here {
            return Err(error);
        }
        state.trial_refused = false;
        state.trial_refused_here = false;
        if state.notices == notices {
            notify(&mut state, "success", "You’re on Newport Pro.");
        }
        Ok(())
    }

    /// Ends a waiting sign-in and any purchase watch when the application shuts down.
    pub fn shutdown(&self) {
        self.cancel_sign_in();
        if let Some(watch) = self.0.state.lock().unwrap().watch.take() {
            watch.abort();
        }
    }

    async fn refresh(&self) {
        let _storage = self.0.storage.lock().await;
        let Some(credential) = ({
            let mut state = self.0.state.lock().unwrap();
            state.refreshed_at = Some(tokio::time::Instant::now());
            state
                .session
                .as_ref()
                .map(|record| Zeroizing::new(record.credential.clone()))
        }) else {
            return;
        };
        let account = self.clone();
        let refreshed = blocking(move || {
            let body = json!({ "machine": machine()?, "appVersion": account.0.app_version });
            account
                .api()?
                .post_dated("/api/v1/license", Some(&credential), &body)
        })
        .await;
        match refreshed {
            Ok((value, date)) => {
                let Some(license) = value.get("license").filter(|license| license.is_object())
                else {
                    log::warn!("The Newport license refresh returned no license.");
                    return;
                };
                let Some((was_pro, is_pro)) = self.keep_license(license.clone(), date).await else {
                    return;
                };
                let mut state = self.0.state.lock().unwrap();
                state.answered_at = Some((tokio::time::Instant::now(), unix_now()));
                state.offline_warned = false;
                // Say so when Pro starts or stops on this computer.
                match (was_pro, is_pro) {
                    (false, true) => notify(&mut state, "success", "You’re on Newport Pro."),
                    (true, false) => {
                        notify(&mut state, "info", "Newport Pro ended on this computer.")
                    }
                    _ => {}
                }
            }
            // The computer was signed out from the account site.
            Err(Failure::Rejected { status: 401, .. }) => {
                let store = self.0.store.clone();
                if let Ok(Err(error)) =
                    tokio::task::spawn_blocking(move || store.remove_account()).await
                {
                    log::warn!("Cannot remove the saved Newport sign-in: {error}");
                }
                let mut state = self.0.state.lock().unwrap();
                state.session = None;
                state.trial_refused = false;
                state.trial_refused_here = false;
                notify(
                    &mut state,
                    "info",
                    "This computer was signed out of your Newport account.",
                );
            }
            Err(error) => {
                log::warn!("Cannot refresh the Newport license: {error}");
                self.warn_offline(unix_now());
            }
        }
    }

    /// The account site can't be reached and this computer's license runs out
    /// soon: say so once, while there is still time to get online.
    fn warn_offline(&self, now: i64) {
        let mut state = self.0.state.lock().unwrap();
        let ends = state
            .session
            .as_ref()
            .filter(|record| grants_pro(&record.license))
            .and_then(|record| token(&record.license).and_then(|token| claim_time(token, "exp")));
        let Some(ends) = ends.filter(|ends| ends - now < OFFLINE_WARNING) else {
            return;
        };
        if std::mem::replace(&mut state.offline_warned, true) {
            return;
        }
        let days = (ends - now).max(0) / 86_400;
        let left = match days {
            0 => "less than a day".to_owned(),
            1 => "1 more day".to_owned(),
            days => format!("{days} more days"),
        };
        notify(
            &mut state,
            "warning",
            &format!(
                "Newport can’t reach your account. Remote apps keep working for {left}; connect to the internet before then."
            ),
        );
    }

    /// Saves a license the account site just sent, with the server time it
    /// vouched for, and counts a new token. The caller holds the storage lock.
    /// Returns whether Pro ran here before and after, or `None` when signed out.
    async fn keep_license(&self, license: Value, date: Option<String>) -> Option<(bool, bool)> {
        let mut record = self.0.state.lock().unwrap().session.clone()?;
        let was_pro = grants_pro(&record.license);
        let mut license = license;
        // Pro still runs here but the answer carried no token (a signing
        // problem on the account site, or the last minute of a lease): keep
        // the saved one while it lasts, rather than stop Remote apps early.
        if grants_pro(&license)
            && token(&license).is_none()
            && let Some(saved) = token(&record.license)
                .filter(|saved| claim_time(saved, "exp").is_some_and(|exp| exp > unix_now()))
        {
            license["token"] = Value::String(saved.to_owned());
        }
        let renewed = token(&license).is_some() && token(&license) != token(&record.license);
        record.trusted_time = trusted_time(record.trusted_time, &license, date.as_deref());
        record.license = license;
        let is_pro = grants_pro(&record.license);
        let store = self.0.store.clone();
        let saved = record.clone();
        match tokio::task::spawn_blocking(move || store.save_account(&saved)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => log::warn!("Cannot save the Newport license: {error}"),
            Err(error) => log::warn!("Newport license save task failed: {error}"),
        }
        let mut state = self.0.state.lock().unwrap();
        state.session = Some(record);
        if renewed {
            state.license_revision += 1;
        }
        Some((was_pro, is_pro))
    }

    async fn wait_for_browser(
        self,
        attempt: u64,
        mut listener: loopback::Listener,
        verifier: Zeroizing<String>,
        state_token: String,
    ) {
        let waited = tokio::time::timeout(SIGN_IN_TIMEOUT, async {
            loop {
                let request = listener.next().await;
                match callback(&request.target, &state_token) {
                    Callback::Other => request.not_found().await,
                    Callback::Invalid => request.invalid().await,
                    Callback::Denied => {
                        request.redirect(&self.connected(Some("canceled"))).await;
                        return None;
                    }
                    Callback::Code(code) => {
                        // Failures name what happened; the toast's button is the next step.
                        let (error, notice) = match self.exchange(code, &verifier).await {
                            Ok(email) => {
                                (None, ("success", format!("Signed in as {email}."), None))
                            }
                            Err(Rejection::Expired) => (
                                Some("expired"),
                                ("error", "That sign-in expired.".into(), Some(TRY_AGAIN)),
                            ),
                            Err(Rejection::DeviceLimit(message)) => (
                                Some("device_limit"),
                                ("error", message, Some(MANAGE_COMPUTERS)),
                            ),
                            Err(Rejection::Failed(error)) => {
                                log::warn!("Newport sign-in failed: {error}");
                                (
                                    Some("failed"),
                                    ("error", "Couldn’t sign in.".into(), Some(TRY_AGAIN)),
                                )
                            }
                        };
                        request.redirect(&self.connected(error)).await;
                        return Some(notice);
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| Some(("error", "The sign-in timed out.".into(), Some(TRY_AGAIN))));
        let mut state = self.0.state.lock().unwrap();
        if state
            .sign_in
            .as_ref()
            .is_some_and(|sign_in| sign_in.attempt == attempt)
        {
            state.sign_in = None;
            if let Some((kind, message, action)) = waited {
                notify_with(&mut state, kind, &message, action);
            }
        }
    }

    async fn exchange(&self, code: String, verifier: &str) -> Result<String, Rejection> {
        let _storage = self.0.storage.lock().await;
        let account = self.clone();
        let verifier = Zeroizing::new(verifier.to_owned());
        let response = blocking(move || {
            let body = json!({
                "code": code,
                "verifier": *verifier,
                "machine": machine()?,
                "name": platform::computer_name(),
                "platform": platform::platform(),
                "appVersion": account.0.app_version,
            });
            account.api()?.post_dated("/api/v1/devices", None, &body)
        })
        .await
        .map_err(|failure| match failure {
            Failure::Rejected { code, .. } if code == "invalid_code" => Rejection::Expired,
            Failure::Rejected { code, message, .. } if code == "device_limit" => {
                Rejection::DeviceLimit(message)
            }
            failure => Rejection::Failed(failure.to_string()),
        })?;
        let (response, date) = response;
        let (Some(credential), Some(device_id), Some(license)) = (
            response.get("credential").and_then(Value::as_str),
            response.get("deviceId").and_then(Value::as_str),
            response
                .get("license")
                .filter(|license| license.is_object()),
        ) else {
            return Err(Rejection::Failed(
                "The sign-in response is incomplete.".into(),
            ));
        };
        let mut record = Record::new(credential.to_owned(), device_id.to_owned(), license.clone());
        record.trusted_time = trusted_time(None, license, date.as_deref());
        let email = license
            .get("email")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let store = self.0.store.clone();
        let saved = record.clone();
        let stored = tokio::task::spawn_blocking(move || store.save_account(&saved))
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
        if let Err(error) = stored {
            // The unsaved computer idles out on the account by itself. Releasing
            // it here would use up one of the account's early seat releases.
            return Err(Rejection::Failed(error));
        }
        let mut state = self.0.state.lock().unwrap();
        if token(&record.license).is_some() {
            state.license_revision += 1;
        }
        state.session = Some(record);
        // A new sign-in starts clean: nothing the previous account learned
        // about trials applies to this one.
        state.trial_refused = false;
        state.trial_refused_here = false;
        state.offline_warned = false;
        state.answered_at = Some((tokio::time::Instant::now(), unix_now()));
        drop(state);
        Ok(email)
    }

    fn end_sign_in(&self, attempt: u64) {
        let mut state = self.0.state.lock().unwrap();
        if state
            .sign_in
            .as_ref()
            .is_some_and(|sign_in| sign_in.attempt == attempt)
        {
            state.sign_in.take().unwrap().task.abort();
        }
    }

    async fn open(&self, url: Url) -> Result<(), String> {
        let desktop = self.0.desktop.clone();
        tokio::task::spawn_blocking(move || desktop.open_url(url.as_str()))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| {
                log::warn!("Cannot open the browser: {error}");
                "Couldn’t open your browser.".to_owned()
            })
    }

    fn connected(&self, error: Option<&str>) -> Url {
        let mut url = self.0.portal.join("/connected").expect("static path");
        if let Some(error) = error {
            url.query_pairs_mut().append_pair("error", error);
        }
        url
    }

    fn api(&self) -> Result<&Api, Failure> {
        self.0
            .api
            .as_ref()
            .map_err(|error| Failure::Unavailable(error.clone()))
    }
}

/// What Newport Remote needs to open a session. The token leaves the backend
/// only through the owner-only socket.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteLicense {
    #[serde(serialize_with = "plain")]
    pub token: Zeroizing<String>,
    pub machine: String,
    /// Unix seconds from the account site; 0 when unknown.
    pub trusted_time: i64,
    /// When the token runs out, as UTC Unix seconds, for the app to say when
    /// open sessions close; 0 when unknown. Newport Remote checks it itself.
    pub expires_at: i64,
}

fn plain<S: serde::Serializer>(
    value: &Zeroizing<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(value)
}

impl std::fmt::Debug for RemoteLicense {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteLicense")
            .field("trusted_time", &self.trusted_time)
            .finish_non_exhaustive()
    }
}

/// Why there is no Remote apps license: `signed_out`, `free`, `free_seat` or
/// `unavailable`, with a sentence to show.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct LicenseRefusal {
    pub reason: &'static str,
    pub message: String,
}

impl LicenseRefusal {
    fn new(reason: &'static str, message: &str) -> Self {
        Self {
            reason,
            message: message.to_owned(),
        }
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// The license's Remote token, when it has one.
fn token(license: &Value) -> Option<&str> {
    license
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
}

/// The latest server time to trust: what was trusted before, the response's
/// `Date` and the token's `iat`, whichever is latest. Never this computer's clock.
fn trusted_time(before: Option<i64>, license: &Value, date: Option<&str>) -> Option<i64> {
    let issued = token(license).and_then(issued_at);
    // The account site never issues a token that has already expired, so a
    // `Date` past this token's expiry is wrong (a proxy, a bad clock). Trusted
    // time only grows, so it would make every later token look expired.
    let expires = token(license).and_then(|token| claim_time(token, "exp"));
    let date = date
        .and_then(http_date)
        .filter(|date| expires.is_some_and(|expires| *date <= expires));
    [before, date, issued].into_iter().flatten().max()
}

/// Now for checking this computer's token: the later of the Mac's clock and
/// the latest time the account site vouched for, as Newport Remote checks.
fn effective_now(record: &Record) -> i64 {
    unix_now().max(record.trusted_time.unwrap_or(0))
}

/// An HTTP `Date` header (`Tue, 20 Oct 2026 09:30:00 GMT`) as Unix seconds.
fn http_date(date: &str) -> Option<i64> {
    let mut words = date.split_whitespace().skip(1);
    let (day, month, year, time) = (words.next()?, words.next()?, words.next()?, words.next()?);
    if words.next()? != "GMT" {
        return None;
    }
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = MONTHS.iter().position(|name| *name == month)? + 1;
    unix_seconds(&format!("{year}-{month:02}-{day:0>2}T{time}Z"))
}

/// The token's `iat` claim as Unix seconds. A PASETO v4.public token carries
/// base64url claims followed by a 64-byte signature; Newport Remote checks the
/// signature, so this only reads when the account site issued it.
fn issued_at(token: &str) -> Option<i64> {
    claim_time(token, "iat")
}

/// A time claim of the token (`iat`, `exp`) as Unix seconds.
fn claim_time(token: &str, claim: &str) -> Option<i64> {
    let body = token.strip_prefix("v4.public.")?.split('.').next()?;
    let bytes = URL_SAFE_NO_PAD.decode(body).ok()?;
    let claims: Value = serde_json::from_slice(bytes.get(..bytes.len().checked_sub(64)?)?).ok()?;
    unix_seconds(claims.get(claim)?.as_str()?)
}

/// `YYYY-MM-DDTHH:MM:SS[.fff]Z` as Unix seconds.
fn unix_seconds(utc: &str) -> Option<i64> {
    let utc = utc.strip_suffix('Z')?;
    let utc = utc.split_once('.').map_or(utc, |(whole, _)| whole);
    let (date, time) = utc.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let mut time = time.splitn(3, ':').map(str::parse::<i64>);
    let (hour, minute, second) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Days from the civil date, after Howard Hinnant's algorithm.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

enum Rejection {
    Expired,
    DeviceLimit(String),
    Failed(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Callback {
    /// Anything but the callback, such as a favicon request.
    Other,
    /// A callback for another attempt, or a malformed one.
    Invalid,
    Denied,
    Code(String),
}

fn callback(target: &str, expected_state: &str) -> Callback {
    let Ok(url) = Url::parse("http://127.0.0.1").and_then(|base| base.join(target)) else {
        return Callback::Invalid;
    };
    if url.path() != "/callback" {
        return Callback::Other;
    }
    let (mut state, mut code, mut error) = (None, None, None);
    for (key, value) in url.query_pairs() {
        match &*key {
            "state" => state = Some(value.into_owned()),
            "code" => code = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }
    if !state.is_some_and(|state| same(state.as_bytes(), expected_state.as_bytes())) {
        return Callback::Invalid;
    }
    if error.is_some() {
        return Callback::Denied;
    }
    match code {
        Some(code)
            if (16..=128).contains(&code.len())
                && code
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') =>
        {
            Callback::Code(code)
        }
        _ => Callback::Invalid,
    }
}

/// Compares without stopping at the first difference.
fn same(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
}

fn notify(state: &mut State, kind: &'static str, message: &str) {
    notify_with(state, kind, message, None);
}

fn notify_with(state: &mut State, kind: &'static str, message: &str, action: Option<&'static str>) {
    state.notices += 1;
    state.notice = Some(Notice {
        id: state.notices,
        kind,
        message: message.to_owned(),
        action,
    });
}

/// Toast actions the app knows how to perform.
const TRY_AGAIN: &str = "tryAgain";
const MANAGE_COMPUTERS: &str = "manageComputers";

/// Whether a license runs Newport Pro on this computer: a paid, complimentary
/// or trial plan, unless the account placed this computer on a free seat.
fn grants_pro(license: &Value) -> bool {
    matches!(
        license.get("state").and_then(Value::as_str),
        Some("active" | "grace" | "trial")
    ) && license.get("seat").and_then(Value::as_str) != Some("free")
}

fn machine() -> Result<String, Failure> {
    platform::machine_id().map_err(Failure::Unavailable)
}

/// 32 random bytes, base64url: a PKCE verifier or a state value.
fn random_token() -> String {
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *bytes).expect("the operating system provides randomness");
    URL_SAFE_NO_PAD.encode(*bytes)
}

async fn blocking<T: Send + 'static>(
    call: impl FnOnce() -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    tokio::task::spawn_blocking(call)
        .await
        .map_err(|error| Failure::Unavailable(error.to_string()))?
}

fn portal() -> Url {
    let production = Url::parse(PORTAL).expect("valid portal URL");
    if crate::profile::data_directory_override().is_none() {
        return production;
    }
    let Some(value) = std::env::var_os(PORTAL_ENV) else {
        return production;
    };
    match value
        .to_str()
        .and_then(|value| Url::parse(value).ok())
        .filter(|url| platform::validate(url).is_ok())
    {
        Some(url) => url,
        None => {
            log::warn!("Ignoring {PORTAL_ENV}: it must be an HTTPS or loopback URL.");
            production
        }
    }
}
