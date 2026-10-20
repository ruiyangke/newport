//! The desktop session's Newport account. The backend owns sign-in and the
//! credential; this controller polls its status and runs one action at a time.
//! Outcomes arrive as backend notices, each shown once.
use crate::backend::Backend;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

/// While a sign-in waits on the browser, its result should appear promptly.
const ACTIVE_INTERVAL: Duration = Duration::from_secs(1);
const IDLE_INTERVAL: Duration = Duration::from_secs(15);

type Outcome = Result<Value, String>;
type Dispatch = dyn Fn(&'static str) -> Result<mpsc::Receiver<Outcome>, String>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    #[default]
    Loading,
    SignedOut,
    Waiting,
    SignedIn,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct License {
    #[serde(default)]
    pub state: String,
    pub plan: Option<String>,
    pub valid_until: Option<String>,
    pub renews_at: Option<String>,
    pub ends_at: Option<String>,
    #[serde(default)]
    pub trial_used: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    #[serde(rename = "status")]
    pub phase: Phase,
    pub email: Option<String>,
    /// From Google, GitHub or Apple; email sign-ins have none.
    pub name: Option<String>,
    pub license: Option<License>,
    #[serde(default)]
    pub signing_out: bool,
    notice: Option<RawNotice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct RawNotice {
    id: u64,
    kind: String,
    message: String,
    #[serde(default)]
    action: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Success,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub message: String,
    /// The toast's button, when the next step is one click away.
    pub action: Option<NoticeAction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    TryAgain,
    ManageComputers,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Effects {
    pub changed: bool,
    pub notice: Option<Notice>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    SignIn,
    CancelSignIn,
    SignOut,
    OpenPortal,
    OpenBilling,
}
impl Action {
    fn method(self) -> &'static str {
        match self {
            Self::SignIn => "account_sign_in",
            Self::CancelSignIn => "account_cancel_sign_in",
            Self::SignOut => "account_sign_out",
            Self::OpenPortal => "account_open_portal",
            Self::OpenBilling => "account_open_billing",
        }
    }

    /// Opening a page in the browser answers nothing about the account.
    fn returns_status(self) -> bool {
        !matches!(self, Self::OpenPortal | Self::OpenBilling)
    }
}

pub struct Account {
    status: Status,
    read: Option<mpsc::Receiver<Outcome>>,
    action: Option<(Action, mpsc::Receiver<Outcome>)>,
    read_at: Option<Instant>,
    shown_notice: u64,
    dispatch: Box<Dispatch>,
    now: Box<dyn Fn() -> Instant>,
}

impl Account {
    pub fn new(backend: Backend) -> Self {
        let backend = Arc::new(backend);
        Self::with_dispatch(
            Box::new(move |method| {
                let (sender, receiver) = mpsc::sync_channel(1);
                let backend = backend.clone();
                thread::Builder::new()
                    .name(format!("newport-{method}"))
                    .spawn(move || {
                        // Signing out waits on the account site (30 s) and the vault.
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                backend.call_with_timeout(
                                    method,
                                    json!({}),
                                    Duration::from_secs(60),
                                )
                            }))
                            .unwrap_or_else(|_| Err("The account request stopped.".into()));
                        let _ = sender.send(outcome);
                    })
                    .map_err(|error| format!("Could not start the account request: {error}"))?;
                Ok(receiver)
            }),
            Box::new(Instant::now),
        )
    }

    /// Answers every request at once from `reply`, for UI fixtures.
    pub fn with_replies(reply: impl Fn(&'static str) -> Outcome + 'static) -> Self {
        Self::with_dispatch(
            Box::new(move |method| {
                let (sender, receiver) = mpsc::sync_channel(1);
                let _ = sender.send(reply(method));
                Ok(receiver)
            }),
            Box::new(Instant::now),
        )
    }

    fn with_dispatch(dispatch: Box<Dispatch>, now: Box<dyn Fn() -> Instant>) -> Self {
        Self {
            status: Status::default(),
            read: None,
            action: None,
            read_at: None,
            shown_notice: 0,
            dispatch,
            now,
        }
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// The action in flight, if any. Only one runs at a time.
    pub fn pending(&self) -> Option<Action> {
        self.action.as_ref().map(|(action, _)| *action)
    }

    /// Asks the account site for the current plan, for a window that became
    /// active again or a menu that opened. It travels as a status read, so it
    /// never blocks an action, and the backend combines prompt repeats.
    pub fn refresh(&mut self) {
        if self.status.phase != Phase::SignedIn || self.action.is_some() {
            return;
        }
        if let Ok(receiver) = (self.dispatch)("account_refresh") {
            self.read = Some(receiver);
        }
    }

    /// Returns false when another action is still running.
    pub fn run(&mut self, action: Action) -> bool {
        if self.action.is_some() {
            return false;
        }
        // A read started before this action could report the state it replaces.
        self.read = None;
        match (self.dispatch)(action.method()) {
            Ok(receiver) => {
                self.action = Some((action, receiver));
                true
            }
            Err(_) => false,
        }
    }

    pub fn poll(&mut self) -> Effects {
        let mut effects = Effects::default();
        if let Some((action, receiver)) = &self.action {
            let action = *action;
            let outcome = match receiver.try_recv() {
                Ok(outcome) => Some(outcome),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("The account request stopped.".into()))
                }
            };
            if let Some(outcome) = outcome {
                self.action = None;
                effects.changed = true;
                match outcome {
                    Ok(value) if action.returns_status() => self.accept(value, &mut effects),
                    Ok(_) => {}
                    Err(message) => {
                        effects.notice = Some(Notice {
                            kind: NoticeKind::Error,
                            message: error_message(action, message),
                            action: None,
                        })
                    }
                }
                // Read what the action left behind, including its notice.
                self.read_at = None;
            }
        }
        if let Some(receiver) = &self.read {
            match receiver.try_recv() {
                Ok(outcome) => {
                    self.read = None;
                    self.read_at = Some((self.now)());
                    // A failed read keeps the last status; the next poll retries.
                    if let Ok(value) = outcome {
                        self.accept(value, &mut effects);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.read = None;
                    self.read_at = Some((self.now)());
                }
            }
        }
        if self.read.is_none() && self.action.is_none() && self.due() {
            match (self.dispatch)("account_status") {
                Ok(receiver) => self.read = Some(receiver),
                Err(_) => self.read_at = Some((self.now)()),
            }
        }
        effects
    }

    fn due(&self) -> bool {
        let interval = match self.status.phase {
            Phase::Loading | Phase::Waiting => ACTIVE_INTERVAL,
            _ if self.status.signing_out => ACTIVE_INTERVAL,
            _ => IDLE_INTERVAL,
        };
        self.read_at
            .is_none_or(|at| (self.now)().saturating_duration_since(at) >= interval)
    }

    fn accept(&mut self, value: Value, effects: &mut Effects) {
        let Ok(status) = serde_json::from_value::<Status>(value) else {
            return;
        };
        if let Some(notice) = &status.notice
            && notice.id > self.shown_notice
        {
            self.shown_notice = notice.id;
            effects.notice = Some(Notice {
                kind: match notice.kind.as_str() {
                    "success" => NoticeKind::Success,
                    "warning" => NoticeKind::Warning,
                    "error" => NoticeKind::Error,
                    _ => NoticeKind::Info,
                },
                message: notice.message.clone(),
                action: match notice.action.as_deref() {
                    Some("tryAgain") => Some(NoticeAction::TryAgain),
                    Some("manageComputers") => Some(NoticeAction::ManageComputers),
                    _ => None,
                },
            });
        }
        if status != self.status {
            self.status = status;
            effects.changed = true;
        }
    }
}

/// Backend errors are already worded for people; transport failures are not.
fn error_message(action: Action, message: String) -> String {
    if message.ends_with('.') && !message.contains("Backend") && !message.contains("IPC") {
        return message;
    }
    match action {
        Action::SignIn => "Couldn’t start signing in. Try again.",
        Action::SignOut => "Couldn’t sign out. Try again.",
        Action::CancelSignIn => "Couldn’t cancel signing in. Try again.",
        Action::OpenPortal => "Couldn’t open your account. Try again.",
        Action::OpenBilling => "Couldn’t open Billing. Try again.",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        rc::Rc,
    };

    #[derive(Clone)]
    struct Harness {
        sent: Rc<RefCell<Vec<&'static str>>>,
        replies: Rc<RefCell<HashMap<&'static str, mpsc::SyncSender<Outcome>>>>,
        clock: Rc<Cell<Instant>>,
    }
    impl Default for Harness {
        fn default() -> Self {
            Self {
                sent: Default::default(),
                replies: Default::default(),
                clock: Rc::new(Cell::new(Instant::now())),
            }
        }
    }
    impl Harness {
        fn account(&self) -> Account {
            let (sent, replies, clock) =
                (self.sent.clone(), self.replies.clone(), self.clock.clone());
            Account::with_dispatch(
                Box::new(move |method| {
                    let (sender, receiver) = mpsc::sync_channel(1);
                    sent.borrow_mut().push(method);
                    replies.borrow_mut().insert(method, sender);
                    Ok(receiver)
                }),
                Box::new(move || clock.get()),
            )
        }
        fn reply(&self, method: &str, outcome: Outcome) {
            self.replies
                .borrow_mut()
                .remove(method)
                .unwrap()
                .send(outcome)
                .unwrap();
        }
        fn advance(&self, by: Duration) {
            self.clock.set(self.clock.get() + by);
        }
        fn sent(&self) -> Vec<&'static str> {
            self.sent.borrow().clone()
        }
    }

    fn status(phase: &str, notice: Option<(u64, &str)>) -> Value {
        json!({
            "status": phase,
            "email": if phase == "signedIn" { json!("ada@example.com") } else { Value::Null },
            "license": if phase == "signedIn" { json!({"state":"active","plan":"pro"}) } else { Value::Null },
            "signingOut": false,
            "notice": notice.map(|(id, message)| json!({"id":id,"kind":"success","message":message})),
        })
    }

    #[test]
    fn reads_status_and_polls_faster_while_waiting() {
        let harness = Harness::default();
        let mut account = harness.account();
        account.poll();
        harness.reply("account_status", Ok(status("signedOut", None)));
        assert!(account.poll().changed);
        assert_eq!(account.status().phase, Phase::SignedOut);

        harness.advance(Duration::from_secs(2));
        account.poll();
        assert_eq!(harness.sent(), vec!["account_status"]);

        assert!(account.run(Action::SignIn));
        assert!(!account.run(Action::SignIn), "one action at a time");
        harness.reply("account_sign_in", Ok(status("waiting", None)));
        assert!(account.poll().changed);
        assert_eq!(account.status().phase, Phase::Waiting);
        harness.reply("account_status", Ok(status("waiting", None)));
        account.poll();
        harness.advance(Duration::from_millis(1100));
        account.poll();
        assert_eq!(
            harness.sent(),
            vec![
                "account_status",
                "account_sign_in",
                "account_status",
                "account_status"
            ]
        );
    }

    #[test]
    fn notices_are_shown_once() {
        let harness = Harness::default();
        let mut account = harness.account();
        account.poll();
        harness.reply(
            "account_status",
            Ok(status(
                "signedIn",
                Some((3, "Signed in as ada@example.com.")),
            )),
        );
        let effects = account.poll();
        assert_eq!(
            effects.notice.unwrap().message,
            "Signed in as ada@example.com."
        );
        assert_eq!(account.status().email.as_deref(), Some("ada@example.com"));
        harness.advance(IDLE_INTERVAL);
        account.poll();
        harness.reply(
            "account_status",
            Ok(status(
                "signedIn",
                Some((3, "Signed in as ada@example.com.")),
            )),
        );
        assert_eq!(account.poll().notice, None);
    }

    #[test]
    fn an_action_discards_an_older_read_and_reports_its_error() {
        let harness = Harness::default();
        let mut account = harness.account();
        account.poll();
        assert!(account.run(Action::SignOut));
        assert_eq!(account.pending(), Some(Action::SignOut));
        // The read was dropped; its late reply has no receiver.
        assert!(
            harness
                .replies
                .borrow_mut()
                .remove("account_status")
                .unwrap()
                .send(Ok(status("signedIn", None)))
                .is_err()
        );
        harness.reply(
            "account_sign_out",
            Err("Couldn’t sign out. Try again.".into()),
        );
        let effects = account.poll();
        assert_eq!(
            effects.notice,
            Some(Notice {
                kind: NoticeKind::Error,
                message: "Couldn’t sign out. Try again.".into(),
                action: None,
            })
        );
        assert_eq!(account.pending(), None);
        assert_eq!(harness.sent().last(), Some(&"account_status"));
    }

    #[test]
    fn a_refresh_never_blocks_an_action_and_carries_toast_actions() {
        let harness = Harness::default();
        let mut account = harness.account();
        account.refresh();
        assert!(
            harness.sent().is_empty(),
            "nothing to refresh while signed out"
        );
        account.poll();
        harness.reply("account_status", Ok(status("signedIn", None)));
        account.poll();

        account.refresh();
        assert_eq!(harness.sent().last(), Some(&"account_refresh"));
        assert_eq!(account.pending(), None);
        assert!(
            account.run(Action::SignOut),
            "a refresh doesn't hold the action slot"
        );

        let mut denied = status("signedOut", None);
        denied["notice"] =
            json!({"id":9,"kind":"error","message":"Couldn’t sign in.","action":"tryAgain"});
        harness.reply("account_sign_out", Ok(denied));
        let notice = account.poll().notice.unwrap();
        assert_eq!(notice.action, Some(NoticeAction::TryAgain));
    }

    #[test]
    fn transport_errors_get_plain_wording() {
        assert_eq!(
            error_message(Action::SignIn, "Backend IPC timed out".into()),
            "Couldn’t start signing in. Try again."
        );
        assert_eq!(
            error_message(Action::SignIn, "Couldn’t open your browser.".into()),
            "Couldn’t open your browser."
        );
    }
}
