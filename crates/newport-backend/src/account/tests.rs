use super::*;
use crate::desktop_integration::DesktopIntegration;
use std::{
    io::{Read as _, Write as _},
    net::TcpListener,
    sync::Mutex,
};

#[derive(Default)]
struct Browser {
    urls: Mutex<Vec<String>>,
    shared: std::sync::atomic::AtomicBool,
    factories: std::sync::atomic::AtomicUsize,
}
impl DesktopIntegration for Browser {
    fn supports_shared_graphical(&self) -> bool {
        self.shared.load(std::sync::atomic::Ordering::Acquire)
    }
    fn graphical_factory(
        &self,
        _: RemoteLicense,
        _: crate::preferences::remote_apps::Settings,
    ) -> crate::graphical_session::FactoryFuture {
        self.factories
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Box::pin(async { Err("Fixture GUI refusal".into()) })
    }
    fn on_main_thread(&self, action: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        action();
        Ok(())
    }
    fn clipboard_revision(&self) -> Option<isize> {
        None
    }
    fn clipboard_offer(&self, _: Option<isize>) -> Option<(isize, Vec<String>)> {
        None
    }
    fn read_clipboard_format(&self, _: isize, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn open_url(&self, url: &str) -> Result<(), String> {
        self.urls.lock().unwrap().push(url.into());
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    authorization: Option<String>,
    body: Value,
}

type Script = dyn Fn(&Seen) -> (u16, Value) + Send + Sync;

/// Every fake response's `Date`: the account site's clock.
const PORTAL_DATE: &str = "Tue, 20 Oct 2026 09:30:00 GMT";
const PORTAL_TIME: i64 = 1_792_488_600;

/// A stand-in for the account site's desktop API.
struct Portal {
    url: Url,
    seen: Arc<Mutex<Vec<Seen>>>,
}
impl Portal {
    fn start(script: impl Fn(&Seen) -> (u16, Value) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!(
            "http://127.0.0.1:{}",
            listener.local_addr().unwrap().port()
        ))
        .unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let script: Box<Script> = Box::new(script);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut data = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, body) = loop {
                    let count = stream.read(&mut chunk).unwrap();
                    data.extend_from_slice(&chunk[..count]);
                    if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&data[..end]).into_owned();
                        let length = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(|v| v.parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        while data.len() < end + 4 + length {
                            let count = stream.read(&mut chunk).unwrap();
                            data.extend_from_slice(&chunk[..count]);
                        }
                        break (head, data[end + 4..end + 4 + length].to_vec());
                    }
                };
                let line: Vec<_> = head.lines().next().unwrap().split(' ').collect();
                let seen = Seen {
                    method: line[0].into(),
                    path: line[1].into(),
                    authorization: head.lines().find_map(|line| {
                        line.strip_prefix("authorization: ")
                            .or_else(|| line.strip_prefix("Authorization: "))
                            .map(str::to_owned)
                    }),
                    body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                };
                let (status, reply) = script(&seen);
                record.lock().unwrap().push(seen);
                let reply = if reply.is_null() {
                    String::new()
                } else {
                    reply.to_string()
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\ndate: {PORTAL_DATE}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Self { url, seen }
    }

    fn seen(&self, path: &str) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| seen.path == path)
            .cloned()
            .collect()
    }
}

fn license(email: &str) -> Value {
    json!({"state":"trial","plan":"trial","validUntil":"2026-11-02T00:00:00.000Z","renewsAt":null,"endsAt":"2026-11-02T00:00:00.000Z","trialUsed":true,"email":email,"token":"v4.public.secret"})
}

fn approved(seen: &Seen) -> (u16, Value) {
    match (seen.method.as_str(), seen.path.as_str()) {
        ("POST", "/api/v1/devices") => (
            200,
            json!({"credential":"device-credential","deviceId":"d1","license":license("ada@example.com")}),
        ),
        ("POST", "/api/v1/license") => (
            200,
            json!({"deviceId":"d1","license":license("ada@example.com")}),
        ),
        ("DELETE", "/api/v1/devices/self") => (204, Value::Null),
        _ => (404, json!({"error":"invalid_request","message":"?"})),
    }
}

struct Fixture {
    account: Account,
    browser: Arc<Browser>,
    store: Arc<Store>,
    _dir: tempfile::TempDir,
}
fn fixture(portal: &Portal) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::for_test(dir.path().to_owned()));
    let browser = Arc::new(Browser::default());
    let account = Account::with_portal(
        store.clone(),
        browser.clone(),
        "9.9.9".into(),
        portal.url.clone(),
    );
    Fixture {
        account,
        browser,
        store,
        _dir: dir,
    }
}

/// Plays the browser's part: follows the Connect page's redirect to the app.
async fn callback(connect: &str, query: impl Fn(&str) -> String) -> String {
    let url = Url::parse(connect).unwrap();
    let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let target = format!("/callback?{}", query(&pairs["state"]));
    let port: u16 = pairs["port"].parse().unwrap();
    tokio::task::spawn_blocking(move || {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    })
    .await
    .unwrap()
}

fn location(response: &str) -> Option<&str> {
    response
        .lines()
        .find_map(|line| line.strip_prefix("Location: "))
}

async fn settle(account: &Account, status: &str) {
    for _ in 0..200 {
        if account.status().status == status {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never reached {status}: {:?}", account.status());
}

#[tokio::test]
async fn signs_in_with_pkce_and_keeps_the_credential_out_of_the_status() {
    let portal = Portal::start(approved);
    let fixture = fixture(&portal);
    fixture.account.sign_in().await.unwrap();
    assert_eq!(fixture.account.status().status, "waiting");
    let connect = fixture.browser.urls.lock().unwrap()[0].clone();
    assert!(connect.starts_with(&format!("{}connect?", portal.url)));

    let response = callback(&connect, |state| {
        format!("code=abcdefghijklmnop1234&state={state}")
    })
    .await;
    assert!(response.starts_with("HTTP/1.1 303"));
    assert_eq!(
        location(&response),
        Some(format!("{}connected", portal.url).as_str())
    );
    settle(&fixture.account, "signedIn").await;

    let exchange = &portal.seen("/api/v1/devices")[0];
    let verifier = exchange.body["verifier"].as_str().unwrap();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    assert!(connect.contains(&format!("challenge={challenge}")));
    assert_eq!(exchange.body["code"], "abcdefghijklmnop1234");
    assert_eq!(exchange.body["appVersion"], "9.9.9");
    assert_eq!(exchange.body["machine"].as_str().unwrap().len(), 43);

    let status = fixture.account.status();
    assert_eq!(status.email.as_deref(), Some("ada@example.com"));
    assert_eq!(status.license.as_ref().unwrap()["state"], "trial");
    assert!(!serde_json::to_string(&status).unwrap().contains("secret"));
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains("device-credential")
    );
    assert_eq!(
        status.notice.unwrap().message,
        "Signed in as ada@example.com."
    );
    assert_eq!(
        fixture.store.account().unwrap().unwrap().credential,
        "device-credential"
    );

    // A second launch reads the saved sign-in.
    let again = Account::with_portal(
        fixture.store.clone(),
        fixture.browser.clone(),
        "9.9.9".into(),
        portal.url.clone(),
    );
    again.load().await;
    assert_eq!(again.status().status, "signedIn");
}

#[tokio::test]
async fn a_callback_for_another_attempt_is_refused_and_the_sign_in_keeps_waiting() {
    let portal = Portal::start(approved);
    let fixture = fixture(&portal);
    fixture.account.sign_in().await.unwrap();
    let connect = fixture.browser.urls.lock().unwrap()[0].clone();
    let response = callback(&connect, |_| {
        "code=abcdefghijklmnop1234&state=forged-state-value".into()
    })
    .await;
    assert!(response.starts_with("HTTP/1.1 400"));
    assert!(portal.seen("/api/v1/devices").is_empty());
    assert_eq!(fixture.account.status().status, "waiting");

    // Choosing Sign in again reopens the same page rather than a new attempt.
    fixture.account.sign_in().await.unwrap();
    let urls = fixture.browser.urls.lock().unwrap().clone();
    assert_eq!(urls, vec![connect.clone(), connect]);
}

#[tokio::test]
async fn canceling_in_the_browser_returns_to_signed_out_quietly() {
    let portal = Portal::start(approved);
    let fixture = fixture(&portal);
    fixture.account.sign_in().await.unwrap();
    let connect = fixture.browser.urls.lock().unwrap()[0].clone();
    let response = callback(&connect, |state| {
        format!("error=access_denied&state={state}")
    })
    .await;
    assert_eq!(
        location(&response),
        Some(format!("{}connected?error=canceled", portal.url).as_str())
    );
    settle(&fixture.account, "signedOut").await;
    assert!(fixture.account.status().notice.is_none());
}

#[tokio::test]
async fn a_full_account_points_the_browser_and_the_app_to_the_device_list() {
    let portal = Portal::start(|_| {
        (
            409,
            json!({"error":"device_limit","message":"5 computers are already signed in. Sign one out at account.newport.app."}),
        )
    });
    let fixture = fixture(&portal);
    fixture.account.sign_in().await.unwrap();
    let connect = fixture.browser.urls.lock().unwrap()[0].clone();
    let response = callback(&connect, |state| {
        format!("code=abcdefghijklmnop1234&state={state}")
    })
    .await;
    assert_eq!(
        location(&response),
        Some(format!("{}connected?error=device_limit", portal.url).as_str())
    );
    settle(&fixture.account, "signedOut").await;
    let notice = fixture.account.status().notice.unwrap();
    assert_eq!(notice.kind, "error");
    assert!(notice.message.starts_with("5 computers"));
    assert_eq!(notice.action, Some("manageComputers"));
    assert!(fixture.store.account().unwrap().is_none());
}

#[tokio::test]
async fn canceling_in_the_app_closes_the_listener() {
    let portal = Portal::start(approved);
    let fixture = fixture(&portal);
    fixture.account.sign_in().await.unwrap();
    let connect = Url::parse(&fixture.browser.urls.lock().unwrap()[0]).unwrap();
    let port: u16 = connect
        .query_pairs()
        .find(|(k, _)| k == "port")
        .unwrap()
        .1
        .parse()
        .unwrap();
    fixture.account.cancel_sign_in();
    assert_eq!(fixture.account.status().status, "signedOut");
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the sign-in listener is still open");
}

async fn signed_in(portal: &Portal) -> Fixture {
    let fixture = fixture(portal);
    fixture
        .store
        .save_account(&Record::new(
            "device-credential".into(),
            "d1".into(),
            license("ada@example.com"),
        ))
        .unwrap();
    fixture.account.load().await;
    fixture
}

#[tokio::test]
async fn signing_out_frees_the_place_and_forgets_the_credential() {
    let portal = Portal::start(approved);
    let fixture = signed_in(&portal).await;
    fixture.account.sign_out().await.unwrap();
    let delete = &portal.seen("/api/v1/devices/self")[0];
    assert_eq!(delete.method, "DELETE");
    assert_eq!(
        delete.authorization.as_deref(),
        Some("Bearer device-credential")
    );
    let status = fixture.account.status();
    assert_eq!(status.status, "signedOut");
    assert_eq!(status.notice.unwrap().message, "Signed out of Newport.");
    assert!(fixture.store.account().unwrap().is_none());
}

#[tokio::test]
async fn signing_out_offline_still_signs_this_computer_out() {
    let portal = Portal::start(|_| (503, Value::Null));
    let fixture = signed_in(&portal).await;
    fixture.account.sign_out().await.unwrap();
    let status = fixture.account.status();
    assert_eq!(status.status, "signedOut");
    assert_eq!(status.notice.unwrap().kind, "warning");
    assert!(fixture.store.account().unwrap().is_none());
}

#[tokio::test]
async fn refresh_updates_the_license_and_a_revoked_credential_signs_out() {
    let portal = Portal::start(|seen| match seen.path.as_str() {
        "/api/v1/license" => (
            200,
            json!({"deviceId":"d1","license":license("grace@example.com")}),
        ),
        _ => (404, Value::Null),
    });
    let fixture = signed_in(&portal).await;
    fixture.account.refresh().await;
    let refresh = &portal.seen("/api/v1/license")[0];
    assert_eq!(
        refresh.authorization.as_deref(),
        Some("Bearer device-credential")
    );
    assert_eq!(refresh.body["appVersion"], "9.9.9");
    assert_eq!(
        fixture.account.status().email.as_deref(),
        Some("grace@example.com")
    );
    assert_eq!(
        fixture.store.account().unwrap().unwrap().license["email"],
        "grace@example.com"
    );

    let revoked = Portal::start(|_| {
        (
            401,
            json!({"error":"unauthorized","message":"This computer is not signed in."}),
        )
    });
    let fixture = signed_in(&revoked).await;
    fixture.account.refresh().await;
    let status = fixture.account.status();
    assert_eq!(status.status, "signedOut");
    assert_eq!(status.notice.unwrap().kind, "info");
    assert!(fixture.store.account().unwrap().is_none());
}

#[tokio::test]
async fn an_outage_keeps_the_sign_in() {
    let portal = Portal::start(|_| (502, Value::Null));
    let fixture = signed_in(&portal).await;
    fixture.account.refresh().await;
    assert_eq!(fixture.account.status().status, "signedIn");
    assert!(fixture.account.status().notice.is_none());
}

#[test]
fn callbacks_must_match_the_attempt() {
    let state = "s".repeat(43);
    assert_eq!(callback_of("/favicon.ico", &state), Callback::Other);
    assert_eq!(
        callback_of(
            &format!("/callback?code=abcdefghijklmnop&state={state}x"),
            &state
        ),
        Callback::Invalid
    );
    assert_eq!(
        callback_of("/callback?code=abcdefghijklmnop", &state),
        Callback::Invalid
    );
    assert_eq!(
        callback_of(&format!("/callback?code=short&state={state}"), &state),
        Callback::Invalid
    );
    assert_eq!(
        callback_of(
            &format!("/callback?code=abc%3Cdefghijklmnop&state={state}"),
            &state
        ),
        Callback::Invalid
    );
    assert_eq!(
        callback_of(
            &format!("/callback?error=access_denied&state={state}"),
            &state
        ),
        Callback::Denied
    );
    assert_eq!(
        callback_of(
            &format!("/callback?code=abcdefghijklmnop&state={state}"),
            &state
        ),
        Callback::Code("abcdefghijklmnop".into())
    );
}

fn callback_of(target: &str, state: &str) -> Callback {
    super::callback(target, state)
}

#[test]
fn remote_license_times_are_read_as_utc() {
    assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(unix_seconds("2000-03-01T00:00:00Z"), Some(951_868_800));
    assert_eq!(
        unix_seconds("2026-10-20T08:30:15.123Z"),
        Some(1_792_485_015)
    );
    assert_eq!(unix_seconds("2026-10-20T08:30:15+02:00"), None);
    assert_eq!(unix_seconds("2026-13-01T00:00:00Z"), None);

    let claims = br#"{"iat":"2026-10-20T08:30:15Z","exp":"2026-11-20T08:30:15Z"}"#;
    let mut body = claims.to_vec();
    body.extend([0u8; 64]);
    let token = format!("v4.public.{}.e30", URL_SAFE_NO_PAD.encode(body));
    assert_eq!(issued_at(&token), Some(1_792_485_015));
    assert_eq!(issued_at("v4.public.AAAA"), None);
    assert_eq!(issued_at("v4.local.AAAA"), None);
}

fn free_license(email: &str) -> Value {
    json!({"state":"none","plan":null,"validUntil":null,"renewsAt":null,"endsAt":null,"trialUsed":false,"email":email,"token":null})
}

async fn signed_in_free(portal: &Portal) -> Fixture {
    let fixture = fixture(portal);
    fixture
        .store
        .save_account(&Record::new(
            "device-credential".into(),
            "d1".into(),
            free_license("ada@example.com"),
        ))
        .unwrap();
    fixture.account.load().await;
    fixture
}

#[tokio::test]
async fn buying_pro_is_announced_once_the_license_shows_it() {
    let portal = Portal::start(|seen| match seen.path.as_str() {
        "/api/v1/license" => (
            200,
            json!({"deviceId":"d1","license":{"state":"active","plan":"pro","email":"ada@example.com","seat":"pro","token":"v4.public.x"}}),
        ),
        _ => (404, Value::Null),
    });
    let fixture = signed_in_free(&portal).await;
    fixture.account.refresh_now().await;
    let notice = fixture.account.status().notice.unwrap();
    assert_eq!(
        (notice.kind, notice.message.as_str()),
        ("success", "You’re on Newport Pro.")
    );
    // The same plan again says nothing new.
    fixture.account.refresh().await;
    assert_eq!(fixture.account.status().notice.unwrap().id, notice.id);
}

#[tokio::test]
async fn a_pro_account_on_a_free_seat_is_not_pro_here() {
    assert!(grants_pro(&json!({"state":"active","seat":"pro"})));
    assert!(grants_pro(&json!({"state":"trial"})));
    assert!(!grants_pro(&json!({"state":"active","seat":"free"})));
    assert!(!grants_pro(&json!({"state":"expired"})));
}

#[tokio::test]
async fn prompt_refreshes_combine() {
    let portal = Portal::start(|seen| match seen.path.as_str() {
        "/api/v1/license" => (
            200,
            json!({"deviceId":"d1","license":free_license("ada@example.com")}),
        ),
        _ => (404, Value::Null),
    });
    let fixture = signed_in_free(&portal).await;
    fixture.account.refresh_now().await;
    fixture.account.refresh_now().await;
    fixture.account.refresh_now().await;
    assert_eq!(portal.seen("/api/v1/license").len(), 1);
    assert!(fixture.account.status().notice.is_none());
}

#[tokio::test]
async fn a_failed_exchange_offers_to_try_again() {
    let portal = Portal::start(|_| (500, json!({"error":"invalid_request","message":"?"})));
    let fixture = fixture(&portal);
    fixture.account.sign_in().await.unwrap();
    let connect = fixture.browser.urls.lock().unwrap()[0].clone();
    callback(&connect, |state| {
        format!("code=abcdefghijklmnop1234&state={state}")
    })
    .await;
    settle(&fixture.account, "signedOut").await;
    let notice = fixture.account.status().notice.unwrap();
    assert_eq!(notice.message, "Couldn’t sign in.");
    assert_eq!(notice.action, Some("tryAgain"));
}

/// A token as the account site shapes one, issued at `iat`. Newport never
/// checks the signature, so zeros stand in for it.
fn token_issued(iat: &str) -> String {
    let mut body = format!(r#"{{"iat":"{iat}","exp":"2026-11-20T08:30:15Z"}}"#).into_bytes();
    body.extend([0u8; 64]);
    format!("v4.public.{}.e30", URL_SAFE_NO_PAD.encode(body))
}

#[tokio::test]
async fn terminal_display_eligibility_is_local_and_refusal_leaves_an_ordinary_shell() {
    use crate::{
        preferences::{Preferences, remote_apps},
        terminal::display::{self, Display, Prepared},
    };
    use std::sync::atomic::Ordering;
    // Public policy fixture only. Production private factories still verify the
    // actual signed grant; this adapter deliberately refuses before any SSH.
    for (supported, installed, state, seat, expected_factory) in [
        (false, true, Some("active"), "pro", false),
        (true, false, Some("active"), "pro", false),
        (true, true, None, "pro", false),
        (true, true, Some("expired"), "pro", false),
        (true, true, Some("revoked"), "pro", false),
        (true, true, Some("active"), "free", false),
        (true, true, Some("active"), "pro", true),
        (true, true, Some("grace"), "pro", true),
    ] {
        let portal =
            Portal::start(|_| panic!("Terminal eligibility must not contact the account site"));
        let fixture = fixture(&portal);
        fixture.browser.shared.store(supported, Ordering::Release);
        if let Some(state) = state {
            // Keep this synthetic metadata away from the account refresh path;
            // it is never accepted by a production signature verifier.
            let mut body = serde_json::to_vec(&json!({
                "iat":"2098-01-01T00:00:00Z",
                "exp":"2099-01-01T00:00:00Z"
            }))
            .unwrap();
            body.extend([0u8; 64]);
            fixture.store.save_account(&Record::new("fixture".into(), "d1".into(), json!({
                "state":state,"seat":seat,"token":format!("v4.public.{}.e30", URL_SAFE_NO_PAD.encode(body))
            }))).unwrap();
        }
        let preferences = Preferences::open(fixture._dir.path()).unwrap();
        let id = uuid::Uuid::new_v4();
        // Keep an unrelated server and unknown preferences through selection.
        std::fs::write(
            fixture._dir.path().join("preferences.json"),
            br#"{"unrelated":{"clipboard":false}}"#,
        )
        .unwrap();
        remote_apps::remote_apps_installed_save(&preferences, uuid::Uuid::new_v4(), true)
            .await
            .unwrap();
        remote_apps::remote_apps_installed_save(&preferences, id, installed)
            .await
            .unwrap();
        let before = std::fs::read(fixture._dir.path().join("preferences.json")).unwrap();
        let server = serde_json::from_value(
            json!({"id":id,"name":"Configured","sshHost":"127.0.0.1","sshPort":22,"sshUser":"fixture"}),
        )
        .unwrap();
        let registry = crate::server_session::Registry::default();
        let owner = registry.session(&server, 0);
        let source = display::join_source(
            id,
            fixture.account.clone(),
            preferences,
            fixture.browser.clone(),
        );
        // With no display running, a new shell starts at once without asking.
        let prepared = Display::Implicit(source.clone()).prepare(&owner).await;
        assert!(matches!(prepared.unwrap(), Prepared::None));
        assert_eq!(fixture.browser.factories.load(Ordering::Acquire), 0);
        // Joining a running display is refused locally for an ineligible
        // server or account; an eligible one reaches the factory.
        let Err(error) = source().await else {
            panic!("the fixture never grants a display")
        };
        if expected_factory {
            assert_eq!(error, "Fixture GUI refusal");
        }
        assert_eq!(
            fixture.browser.factories.load(Ordering::Acquire),
            usize::from(expected_factory)
        );
        assert!(fixture.browser.urls.lock().unwrap().is_empty());
        assert!(portal.seen.lock().unwrap().is_empty());
        assert_eq!(owner.transport_snapshot().demand, 0);
        assert_eq!(owner.transport_snapshot().generation, 0);
        assert!(owner.graphical_sessions().is_empty());
        assert_eq!(
            std::fs::read(fixture._dir.path().join("preferences.json")).unwrap(),
            before
        );
        if !supported || !installed {
            assert_eq!(
                fixture.account.remote_session_policy().borrow().access,
                remote_sessions::Access::Loading
            );
        }
        registry.shutdown().await;
    }
}

#[tokio::test]
async fn remote_session_policy_preserves_expiry_but_remembers_coalesced_revocation() {
    use super::remote_sessions::Access;
    let answer = Arc::new(Mutex::new((503, Value::Null)));
    let reply = answer.clone();
    let portal = Portal::start(move |seen| {
        if seen.method == "DELETE" {
            return (204, Value::Null);
        }
        reply.lock().unwrap().clone()
    });
    let fixture = signed_in(&portal).await;
    let mut policy = fixture.account.remote_session_policy();
    let initial = policy.borrow_and_update().clone();
    assert_eq!(
        initial.access,
        Access::Renew {
            device: "d1".into()
        }
    );
    fixture.account.refresh().await;
    assert!(
        !policy.has_changed().unwrap(),
        "Offline refresh must preserve the signed lease"
    );

    let mut next = license("ada@example.com");
    next["token"] = json!(token_issued("2026-10-20T08:30:00Z"));
    *answer.lock().unwrap() = (200, json!({"deviceId":"d1", "license":next}));
    fixture.account.refresh().await;
    assert!(policy.has_changed().unwrap());
    let renewed = policy.borrow_and_update().clone();
    assert!(renewed.revision > initial.revision);
    assert_eq!(renewed.epoch, initial.epoch);
    *answer.lock().unwrap() = (
        200,
        json!({"deviceId":"d1", "license":{"state":"expired","plan":"free"}}),
    );
    fixture.account.refresh().await;
    assert_eq!(
        policy.borrow().access,
        Access::Keep {
            device: "d1".into()
        }
    );
    assert_eq!(policy.borrow().epoch, initial.epoch);

    // The watcher deliberately does not consume Stop before the next grant.
    *answer.lock().unwrap() = (
        200,
        json!({"deviceId":"d1", "license":{"state":"active","plan":"pro","seat":"free"}}),
    );
    fixture.account.refresh().await;
    assert_eq!(policy.borrow().access, Access::Stop);
    *answer.lock().unwrap() = (200, json!({"deviceId":"d1", "license":next}));
    fixture.account.refresh().await;
    assert_eq!(
        policy.borrow().access,
        Access::Renew {
            device: "d1".into()
        }
    );
    assert_eq!(policy.borrow().epoch, initial.epoch + 1);
    assert!(!format!("{:?}", *policy.borrow()).contains("v4.public"));
    fixture.account.sign_out().await.unwrap();
    assert_eq!(policy.borrow().access, Access::Stop);
    assert_eq!(policy.borrow().epoch, initial.epoch + 2);
}

#[tokio::test]
async fn remote_session_policy_loads_and_observes_account_site_sign_out() {
    use super::remote_sessions::Access;
    let portal = Portal::start(|_| (401, json!({"error":"signed_out"})));
    let fixture = fixture(&portal);
    let policy = fixture.account.remote_session_policy();
    assert_eq!(policy.borrow().access, Access::Loading);
    fixture
        .store
        .save_account(&Record::new(
            "c".into(),
            "d1".into(),
            license("ada@example.com"),
        ))
        .unwrap();
    fixture.account.load().await;
    assert_eq!(
        policy.borrow().access,
        Access::Renew {
            device: "d1".into()
        }
    );
    fixture.account.refresh().await;
    assert_eq!(policy.borrow().access, Access::Stop);
    assert_eq!(policy.borrow().epoch, 1);
}

#[test]
fn http_dates_are_read_as_utc() {
    assert_eq!(http_date(PORTAL_DATE), Some(PORTAL_TIME));
    assert_eq!(http_date("Sun, 6 Nov 1994 08:49:37 GMT"), Some(784_111_777));
    assert_eq!(http_date("Tue, 20 Oct 2026 09:30:00 PST"), None);
    assert_eq!(http_date("yesterday"), None);
}

#[tokio::test]
async fn remote_license_says_why_this_computer_has_none() {
    let portal = Portal::start(|_| (503, Value::Null));
    let refusal = |fixture: &Fixture| {
        let account = fixture.account.clone();
        async move { account.remote_license().await.unwrap_err().reason }
    };
    let signed_out = fixture(&portal);
    assert_eq!(refusal(&signed_out).await, "signed_out");
    assert_eq!(refusal(&signed_in_free(&portal).await).await, "free");
    for (license, reason) in [
        (
            json!({"state":"active","plan":"pro","seat":"free","token":null}),
            "free_seat",
        ),
        (
            json!({"state":"trial","plan":"trial","seat":"free","token":null}),
            "trial_elsewhere",
        ),
        (
            json!({"state":"active","plan":"pro","seat":"pro","token":null}),
            "unavailable",
        ),
    ] {
        let fixture = fixture(&portal);
        fixture
            .store
            .save_account(&Record::new("c".into(), "d1".into(), license))
            .unwrap();
        assert_eq!(refusal(&fixture).await, reason);
    }
}

#[tokio::test]
async fn remote_license_trusts_the_latest_server_time() {
    // The token was issued an hour before the response's Date.
    let token = token_issued("2026-10-20T08:30:00Z");
    let reply = token.clone();
    let portal = Portal::start(move |seen| match seen.path.as_str() {
        "/api/v1/license" => (
            200,
            json!({"deviceId":"d1","license":{"state":"active","plan":"pro","seat":"pro","token":reply}}),
        ),
        _ => (404, Value::Null),
    });
    let fixture = signed_in_free(&portal).await;
    fixture.account.refresh().await;
    let granted = fixture.account.remote_license().await.unwrap();
    assert_eq!(*granted.token, token);
    assert_eq!(granted.trusted_time, PORTAL_TIME);
    assert_eq!(
        granted.expires_at,
        unix_seconds("2026-11-20T08:30:15Z").unwrap()
    );
    assert_eq!(granted.machine, machine().unwrap());
    assert_eq!(
        fixture.store.account().unwrap().unwrap().trusted_time,
        Some(PORTAL_TIME)
    );
    // The token never appears in debug output.
    assert!(!format!("{granted:?}").contains("v4.public"));
}

#[tokio::test]
async fn a_new_token_counts_as_a_new_revision() {
    let next = Arc::new(Mutex::new("v4.public.secret".to_owned()));
    let issued = next.clone();
    let portal = Portal::start(move |_| {
        let mut license = license("ada@example.com");
        license["token"] = json!(*issued.lock().unwrap());
        (200, json!({"deviceId":"d1","license":license}))
    });
    let fixture = signed_in(&portal).await;
    let loaded = fixture.account.status().license_revision;
    fixture.account.refresh().await;
    assert_eq!(
        fixture.account.status().license_revision,
        loaded,
        "same token"
    );
    *next.lock().unwrap() = "v4.public.renewed".into();
    fixture.account.refresh().await;
    assert_eq!(fixture.account.status().license_revision, loaded + 1);
}

#[tokio::test]
async fn starting_a_trial_saves_its_license() {
    let portal = Portal::start(|seen| match seen.path.as_str() {
        "/api/v1/trial" => (
            200,
            json!({"deviceId":"d1","license":license("ada@example.com")}),
        ),
        _ => (404, Value::Null),
    });
    let fixture = signed_in_free(&portal).await;
    let before = fixture.account.status().license_revision;
    fixture.account.start_trial().await.unwrap();
    let trial = &portal.seen("/api/v1/trial")[0];
    assert_eq!(
        trial.authorization.as_deref(),
        Some("Bearer device-credential")
    );
    assert_eq!(trial.body["machine"], machine().unwrap());
    let status = fixture.account.status();
    assert_eq!(status.license.unwrap()["state"], "trial");
    assert_eq!(status.license_revision, before + 1);
    assert_eq!(
        status.notice.unwrap().message,
        "Your free trial has started."
    );
}

#[tokio::test]
async fn a_refused_trial_passes_on_the_account_sites_words() {
    let portal = Portal::start(|seen| match seen.path.as_str() {
        "/api/v1/trial" => (
            409,
            json!({"error":"trial_used","message":"This computer has already used a free trial."}),
        ),
        "/api/v1/license" => (
            200,
            json!({"deviceId":"d1","license":free_license("ada@example.com")}),
        ),
        _ => (404, Value::Null),
    });
    let fixture = signed_in_free(&portal).await;
    let error = fixture.account.start_trial().await.unwrap_err();
    assert_eq!(error, "This computer has already used a free trial.");
    // The app shows the returned error itself; no notice repeats it.
    assert!(fixture.account.status().notice.is_none());
    // The page learns the current state from the refresh that followed.
    assert_eq!(portal.seen("/api/v1/license").len(), 1);
}

#[tokio::test]
async fn a_trial_refused_because_pro_already_runs_here_is_one_notice() {
    let portal = Portal::start(|seen| match seen.path.as_str() {
        "/api/v1/trial" => (
            409,
            json!({"error":"already_licensed","message":"This computer already has Newport Pro."}),
        ),
        "/api/v1/license" => (
            200,
            json!({"deviceId":"d1","license":{"state":"active","plan":"pro","seat":"pro","token":token_issued(&now_utc(0))}}),
        ),
        _ => (404, Value::Null),
    });
    let fixture = signed_in_free(&portal).await;
    fixture.account.start_trial().await.unwrap();
    let notice = fixture.account.status().notice.unwrap();
    assert_eq!(notice.message, "You’re on Newport Pro.");
}

#[tokio::test]
async fn a_trial_refused_as_already_licensed_without_pro_stays_an_error() {
    let portal = Portal::start(|seen| match seen.path.as_str() {
        "/api/v1/trial" => (
            409,
            json!({"error":"already_licensed","message":"This computer already has Newport Pro."}),
        ),
        "/api/v1/license" => (
            200,
            json!({"deviceId":"d1","license":free_license("ada@example.com")}),
        ),
        _ => (404, Value::Null),
    });
    let fixture = signed_in_free(&portal).await;
    let error = fixture.account.start_trial().await.unwrap_err();
    assert_eq!(error, "This computer already has Newport Pro.");
    assert!(fixture.account.status().notice.is_none());
}

#[tokio::test]
async fn a_fresh_token_answers_without_asking_the_account_site() {
    let fresh = token_issued(&now_utc(0));
    let reply = fresh.clone();
    let portal = Portal::start(move |seen| match seen.path.as_str() {
        "/api/v1/license" => (
            200,
            json!({"deviceId":"d1","license":{"state":"active","plan":"pro","seat":"pro","token":reply}}),
        ),
        _ => (404, Value::Null),
    });
    let fixture = fixture(&portal);
    fixture
        .store
        .save_account(&Record::new(
            "c".into(),
            "d1".into(),
            json!({"state":"active","plan":"pro","seat":"pro","token":fresh}),
        ))
        .unwrap();
    // Renewing open sessions asks often; a fresh token must not cause a check-in.
    for _ in 0..3 {
        fixture.account.remote_license().await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(portal.seen("/api/v1/license").is_empty());

    // A day-old token is refreshed, once, in the background.
    let old = token_issued(&now_utc(-2 * 86_400));
    let stale = super::tests::fixture(&portal);
    stale
        .store
        .save_account(&Record::new(
            "c".into(),
            "d1".into(),
            json!({"state":"active","plan":"pro","seat":"pro","token":old}),
        ))
        .unwrap();
    stale.account.remote_license().await.unwrap();
    for _ in 0..100 {
        if !portal.seen("/api/v1/license").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(portal.seen("/api/v1/license").len(), 1);
}

#[tokio::test]
async fn a_trial_this_computer_used_reads_as_used() {
    let portal = Portal::start(|seen| match seen.path.as_str() {
        "/api/v1/trial" => (
            409,
            json!({"error":"trial_used","message":"This computer has already used a free trial."}),
        ),
        "/api/v1/license" => (
            200,
            json!({"deviceId":"d1","license":free_license("ada@example.com")}),
        ),
        _ => (404, Value::Null),
    });
    let fixture = signed_in_free(&portal).await;
    assert_eq!(
        fixture.account.status().license.unwrap()["trialUsed"],
        false
    );
    fixture.account.start_trial().await.unwrap_err();
    assert_eq!(fixture.account.status().license.unwrap()["trialUsed"], true);
}

/// Now, shifted by `offset` seconds, as a claim time (`2026-10-20T09:30:00Z`).
fn now_utc(offset: i64) -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + offset;
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Civil date from days since 1970 (Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest / 60 % 60,
        rest % 60
    )
}

/// A token issued at `iat` and ending at `exp`, both as claim timestamps.
fn token_between(iat: &str, exp: &str) -> String {
    let mut body = format!(r#"{{"iat":"{iat}","exp":"{exp}"}}"#).into_bytes();
    body.extend([0u8; 64]);
    format!("v4.public.{}.e30", URL_SAFE_NO_PAD.encode(body))
}

fn pro_with(token: &str) -> Value {
    json!({"state":"active","plan":"pro","seat":"pro","email":"ada@example.com","token":token})
}

async fn signed_in_with(portal: &Portal, license: Value) -> Fixture {
    let fixture = fixture(portal);
    fixture
        .store
        .save_account(&Record::new(
            "device-credential".into(),
            "d1".into(),
            license,
        ))
        .unwrap();
    fixture.account.load().await;
    fixture
}

#[tokio::test]
async fn the_daily_refresh_is_due_by_the_wall_clock_or_a_near_end() {
    let portal = Portal::start(|_| (404, Value::Null));
    let fresh = token_between(&now_utc(0), &now_utc(20 * 86_400));
    let fixture = signed_in_with(&portal, pro_with(&fresh)).await;
    let now = unix_now();
    assert!(fixture.account.refresh_due(now), "never answered yet");
    fixture.account.0.state.lock().unwrap().answered_at = Some((tokio::time::Instant::now(), now));
    assert!(!fixture.account.refresh_due(now + 3_600));
    // A day later by the wall clock, however long the Mac slept.
    assert!(fixture.account.refresh_due(now + 86_400));
    // A token ending within two days is refreshed early.
    assert!(fixture.account.refresh_due(now + 19 * 86_400));
}

#[tokio::test]
async fn a_launch_with_a_run_out_token_and_no_network_says_so() {
    let portal = Portal::start(|_| (503, json!({"error":"unavailable","message":"Down."})));
    let expired = token_between(&now_utc(-40 * 86_400), &now_utc(-10 * 86_400));
    let fixture = signed_in_with(&portal, pro_with(&expired)).await;
    let refusal = fixture.account.remote_license().await.unwrap_err();
    assert_eq!(refusal.reason, "offline");
    assert!(refusal.message.contains("Connect to the internet"));
    // It asked first.
    assert!(!portal.seen("/api/v1/license").is_empty());
}

#[tokio::test]
async fn a_launch_waits_for_a_fresh_token_instead_of_failing() {
    let fresh = token_between(&now_utc(0), &now_utc(30 * 86_400));
    let reply = fresh.clone();
    let portal = Portal::start(move |seen| match seen.path.as_str() {
        "/api/v1/license" => (200, json!({"deviceId":"d1","license":pro_with(&reply)})),
        _ => (404, Value::Null),
    });
    let expired = token_between(&now_utc(-40 * 86_400), &now_utc(-10 * 86_400));
    let fixture = signed_in_with(&portal, pro_with(&expired)).await;
    let granted = fixture.account.remote_license().await.unwrap();
    assert_eq!(*granted.token, fresh);
}

#[tokio::test]
async fn a_fresh_token_that_still_looks_expired_means_the_macs_clock_is_wrong() {
    // The account site answers with a token that ended a month ago by this
    // computer's clock: the clock is a month ahead.
    let behind = token_between(&now_utc(-31 * 86_400), &now_utc(-86_400));
    let reply = behind.clone();
    let portal = Portal::start(move |seen| match seen.path.as_str() {
        "/api/v1/license" => (200, json!({"deviceId":"d1","license":pro_with(&reply)})),
        _ => (404, Value::Null),
    });
    let fixture = signed_in_with(&portal, pro_with(&behind)).await;
    let refusal = fixture.account.remote_license().await.unwrap_err();
    assert_eq!(refusal.reason, "clock");
    assert!(refusal.message.contains("clock"));
}

#[tokio::test]
async fn pro_without_a_token_in_the_answer_keeps_the_saved_one() {
    let portal = Portal::start(|seen| match seen.path.as_str() {
        "/api/v1/license" => (
            200,
            json!({"deviceId":"d1","license":{"state":"active","plan":"pro","seat":"pro","token":null}}),
        ),
        _ => (404, Value::Null),
    });
    let saved = token_between(&now_utc(-86_400), &now_utc(20 * 86_400));
    let fixture = signed_in_with(&portal, pro_with(&saved)).await;
    fixture.account.refresh().await;
    assert_eq!(
        *fixture.account.remote_license().await.unwrap().token,
        saved
    );
}

#[test]
fn a_date_past_the_tokens_expiry_is_not_trusted() {
    let license = pro_with(&token_between(
        "2026-10-20T08:30:00Z",
        "2026-10-21T08:30:00Z",
    ));
    let iat = unix_seconds("2026-10-20T08:30:00Z").unwrap();
    // Within the token's life: trusted.
    assert_eq!(
        trusted_time(None, &license, Some(PORTAL_DATE)),
        Some(PORTAL_TIME)
    );
    // A month ahead: ignored, the token's own issue time stands.
    assert_eq!(
        trusted_time(None, &license, Some("Fri, 20 Nov 2026 09:30:00 GMT")),
        Some(iat)
    );
}

#[tokio::test]
async fn the_revision_never_starts_low_after_a_restart() {
    let portal = Portal::start(|_| (404, Value::Null));
    let fixture = signed_in(&portal).await;
    // Seeded from the clock: a restarted backend can't reuse a revision the
    // UI saw from the previous one.
    assert!(fixture.account.status().license_revision > 1_000_000);
}

#[tokio::test]
async fn a_sign_out_from_the_account_site_forgets_trial_refusals() {
    let portal = Portal::start(|seen| match seen.path.as_str() {
        "/api/v1/license" => (401, json!({"error":"unauthorized","message":"Signed out."})),
        _ => (404, Value::Null),
    });
    let fixture = signed_in_free(&portal).await;
    {
        let mut state = fixture.account.0.state.lock().unwrap();
        state.trial_refused = true;
        state.trial_refused_here = true;
    }
    fixture.account.refresh().await;
    let state = fixture.account.0.state.lock().unwrap();
    assert!(state.session.is_none());
    assert!(!state.trial_refused && !state.trial_refused_here);
}

#[tokio::test]
async fn a_trial_this_computer_used_under_another_account_reads_as_used_here() {
    let portal = Portal::start(|seen| match seen.path.as_str() {
        "/api/v1/trial" => (
            409,
            json!({"error":"machine_trial_used","message":"This computer has already used a free trial."}),
        ),
        "/api/v1/license" => (
            200,
            json!({"deviceId":"d1","license":free_license("ada@example.com")}),
        ),
        _ => (404, Value::Null),
    });
    let fixture = signed_in_free(&portal).await;
    let error = fixture.account.start_trial().await.unwrap_err();
    assert_eq!(error, "This computer has already used a free trial.");
    let license = fixture.account.status().license.unwrap();
    assert_eq!(license["trialUsedHere"], true);
    assert_ne!(
        license["trialUsed"], true,
        "the account itself never had one"
    );
}

#[tokio::test]
async fn a_trial_that_started_despite_an_error_is_reported_as_started() {
    // The reply was lost; the retry is refused as used, but the refresh shows
    // the trial running here.
    let portal = Portal::start(|seen| match seen.path.as_str() {
        "/api/v1/trial" => (
            409,
            json!({"error":"trial_used","message":"This account has already used its free trial."}),
        ),
        "/api/v1/license" => (
            200,
            json!({"deviceId":"d1","license":{"state":"trial","plan":"trial","seat":"trial","token":token_issued(&now_utc(0))}}),
        ),
        _ => (404, Value::Null),
    });
    let fixture = signed_in_free(&portal).await;
    fixture.account.start_trial().await.unwrap();
    assert_eq!(
        fixture.account.status().notice.unwrap().message,
        "You’re on Newport Pro."
    );
}

#[tokio::test]
async fn an_offline_refresh_warns_once_before_the_license_runs_out() {
    let portal = Portal::start(|_| (503, json!({"error":"unavailable","message":"Down."})));
    let ending = token_between(&now_utc(-28 * 86_400), &now_utc(2 * 86_400));
    let fixture = signed_in_with(&portal, pro_with(&ending)).await;
    fixture.account.refresh().await;
    let notice = fixture.account.status().notice.unwrap();
    assert_eq!(notice.kind, "warning");
    assert!(notice.message.contains("2 more days") || notice.message.contains("1 more day"));
    let id = notice.id;
    fixture.account.refresh().await;
    assert_eq!(fixture.account.status().notice.unwrap().id, id, "once");
}
