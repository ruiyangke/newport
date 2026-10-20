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
/// Window activations and menu opens within this gap share one request.
const PROMPT_REFRESH_GAP: Duration = Duration::from_secs(15);
/// After Billing opens: check every half minute for ten minutes.
const WATCH_INTERVAL: Duration = Duration::from_secs(30);
const WATCH_CHECKS: u32 = 20;
/// License fields the UI may show. The token stays in the backend.
const LICENSE_FIELDS: [&str; 7] = [
    "state",
    "plan",
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
            state: Mutex::default(),
            storage: tokio::sync::Mutex::new(()),
        }))
    }

    pub fn status(&self) -> Status {
        let state = self.0.state.lock().unwrap();
        let license = state.session.as_ref().map(|record| {
            let fields = LICENSE_FIELDS
                .iter()
                .filter_map(|key| Some(((*key).to_owned(), record.license.get(*key)?.clone())))
                .collect::<serde_json::Map<_, _>>();
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
        }
    }

    /// Loads the saved sign-in, then refreshes it at launch and once a day.
    pub async fn maintain(self) {
        self.load().await;
        tokio::time::sleep(FIRST_REFRESH).await;
        let mut tick = tokio::time::interval(REFRESH_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            self.refresh().await;
        }
    }

    async fn load(&self) {
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
            .append_pair("state", &state_token);
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
                .post("/api/v1/license", Some(&credential), &body)
        })
        .await;
        match refreshed {
            Ok(value) => {
                let Some(license) = value.get("license").filter(|license| license.is_object())
                else {
                    log::warn!("The Newport license refresh returned no license.");
                    return;
                };
                let Some(mut record) = self.0.state.lock().unwrap().session.clone() else {
                    return;
                };
                let was_pro = grants_pro(&record.license);
                record.license = license.clone();
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
                notify(
                    &mut state,
                    "info",
                    "This computer was signed out of your Newport account.",
                );
            }
            Err(error) => log::warn!("Cannot refresh the Newport license: {error}"),
        }
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
            account.api()?.post("/api/v1/devices", None, &body)
        })
        .await
        .map_err(|failure| match failure {
            Failure::Rejected { code, .. } if code == "invalid_code" => Rejection::Expired,
            Failure::Rejected { code, message, .. } if code == "device_limit" => {
                Rejection::DeviceLimit(message)
            }
            failure => Rejection::Failed(failure.to_string()),
        })?;
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
        let record = Record::new(credential.to_owned(), device_id.to_owned(), license.clone());
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
            // Don't leave a place taken by a credential nobody kept.
            let account = self.clone();
            let credential = Zeroizing::new(record.credential.clone());
            let _ =
                blocking(move || account.api()?.delete("/api/v1/devices/self", &credential)).await;
            return Err(Rejection::Failed(error));
        }
        self.0.state.lock().unwrap().session = Some(record);
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
