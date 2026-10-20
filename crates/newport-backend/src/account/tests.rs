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
}
impl DesktopIntegration for Browser {
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
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
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
