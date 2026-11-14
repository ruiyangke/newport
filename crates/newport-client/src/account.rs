//! The desktop session's Newport account. The backend owns sign-in and the
//! credential; this controller polls its status and runs one action at a time.
//! Outcomes arrive as backend notices, each shown once.
use crate::backend::Backend;
use newport_protocol::{
    common::Present,
    desktop::{self, Kind, account as protocol},
};
use serde::Deserialize;
use std::{
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

/// While a sign-in waits on the browser, its result should appear promptly.
const ACTIVE_INTERVAL: Duration = Duration::from_secs(1);
const IDLE_INTERVAL: Duration = Duration::from_secs(15);

const UNREADABLE_STATUS: &str = "Newport couldn’t read your account status.";

/// An account reply: its status, or `None` for an action that answers
/// nothing about the account.
type Outcome = Result<Option<protocol::Status>, String>;
type Dispatch = dyn Fn(protocol::Operation) -> Result<mpsc::Receiver<Outcome>, String>;

/// A toast to show once: a backend notice, or an action's failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub kind: protocol::NoticeKind,
    pub message: String,
    /// The toast's button, when the next step is one click away.
    pub action: Option<protocol::NoticeAction>,
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
    StartTrial,
}
impl Action {
    fn operation(self) -> protocol::Operation {
        let present = Box::new(Present {});
        match self {
            Self::SignIn => protocol::Operation::SignIn(present),
            Self::CancelSignIn => protocol::Operation::CancelSignIn(present),
            Self::SignOut => protocol::Operation::SignOut(present),
            Self::OpenPortal => protocol::Operation::OpenPortal(present),
            Self::OpenBilling => protocol::Operation::OpenBilling(present),
            Self::StartTrial => protocol::Operation::StartTrial(present),
        }
    }

    /// Opening a page in the browser answers nothing about the account.
    fn returns_status(self) -> bool {
        !matches!(self, Self::OpenPortal | Self::OpenBilling)
    }
}

pub struct Account {
    status: protocol::Status,
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
            Box::new(move |operation| {
                let (sender, receiver) = mpsc::sync_channel(1);
                let backend = backend.clone();
                thread::Builder::new()
                    .name(format!("newport-{:?}", operation.kind()))
                    .spawn(move || {
                        // Signing out waits on the account site and credential storage.
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                backend
                                    .execute_with_timeout(
                                        desktop::Operation::Account(Box::new(operation)),
                                        Duration::from_secs(60),
                                    )
                                    .map_err(|error| error.to_string())
                                    .and_then(status_reply)
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

    /// Answers every request at once with the status from `reply`, for UI
    /// fixtures.
    pub fn with_replies(
        reply: impl Fn(Kind) -> Result<protocol::Status, String> + 'static,
    ) -> Self {
        Self::with_dispatch(
            Box::new(move |operation| {
                let (sender, receiver) = mpsc::sync_channel(1);
                let _ = sender.send(reply(operation.kind()).map(Some));
                Ok(receiver)
            }),
            Box::new(Instant::now),
        )
    }

    fn with_dispatch(dispatch: Box<Dispatch>, now: Box<dyn Fn() -> Instant>) -> Self {
        Self {
            status: protocol::Status::default(),
            read: None,
            action: None,
            read_at: None,
            shown_notice: 0,
            dispatch,
            now,
        }
    }

    pub fn status(&self) -> &protocol::Status {
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
        if self.status.state != protocol::Phase::SignedIn || self.action.is_some() {
            return;
        }
        if let Ok(receiver) = (self.dispatch)(protocol::Operation::Refresh(Box::new(Present {}))) {
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
        match (self.dispatch)(action.operation()) {
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
                    Ok(Some(status)) if action.returns_status() => {
                        self.accept(status, &mut effects)
                    }
                    Ok(_) => {}
                    Err(message) => {
                        effects.notice = Some(Notice {
                            kind: protocol::NoticeKind::Error,
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
                    if let Ok(Some(status)) = outcome {
                        self.accept(status, &mut effects);
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
            match (self.dispatch)(protocol::Operation::Status(Box::new(Present {}))) {
                Ok(receiver) => self.read = Some(receiver),
                Err(_) => self.read_at = Some((self.now)()),
            }
        }
        effects
    }

    fn due(&self) -> bool {
        let interval = match self.status.state {
            protocol::Phase::Loading | protocol::Phase::Waiting => ACTIVE_INTERVAL,
            _ if self.status.signing_out => ACTIVE_INTERVAL,
            _ => IDLE_INTERVAL,
        };
        self.read_at
            .is_none_or(|at| (self.now)().saturating_duration_since(at) >= interval)
    }

    fn accept(&mut self, status: protocol::Status, effects: &mut Effects) {
        if let Some(notice) = &status.notice
            && notice.id > self.shown_notice
        {
            self.shown_notice = notice.id;
            effects.notice = Some(Notice {
                kind: notice.kind,
                message: notice.message.clone(),
                action: notice.action,
            });
        }
        if status != self.status {
            self.status = status;
            effects.changed = true;
        }
    }
}

/// What a Remote apps session needs to prove this computer holds Newport Pro.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteLicense {
    pub token: String,
    /// This computer's machine hash, as the token names it.
    pub machine: String,
    /// The latest time the account site vouched for, as UTC Unix seconds.
    pub trusted_time: i64,
    /// When the token runs out, as UTC Unix seconds; 0 when unknown.
    #[serde(default)]
    pub expires_at: i64,
}

impl std::fmt::Debug for RemoteLicense {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteLicense")
            .field("trusted_time", &self.trusted_time)
            .finish_non_exhaustive()
    }
}

/// Why this computer has no Remote apps license: the backend's reason
/// (`signed_out`, `free`, `free_seat`, `trial_elsewhere`, `offline`, `clock`,
/// `unavailable`) and the sentence to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LicenseRefusal {
    pub reason: Option<String>,
    pub message: String,
}

impl LicenseRefusal {
    /// Worth asking again shortly: the license couldn't be read or the
    /// account site couldn't be reached. Plan, seat and sign-in refusals
    /// change only when the account does.
    pub fn transient(&self) -> bool {
        matches!(
            self.reason.as_deref(),
            None | Some("unavailable" | "offline")
        )
    }

    /// The account step that fixes this refusal, if there is one.
    pub fn action(&self) -> Option<Action> {
        match self.reason.as_deref()? {
            "signed_out" => Some(Action::SignIn),
            "free" | "trial_elsewhere" => Some(Action::OpenBilling),
            "free_seat" => Some(Action::OpenPortal),
            _ => None,
        }
    }
}

impl std::fmt::Display for LicenseRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// This computer's Remote apps license, or why it has none. Blocking: call it
/// off the UI thread.
pub fn remote_license(backend: &Backend) -> Result<RemoteLicense, LicenseRefusal> {
    let reply = backend
        .execute_with_timeout(
            desktop::Operation::Account(Box::new(protocol::Operation::RemoteLicense(Box::new(
                Present {},
            )))),
            Duration::from_secs(15),
        )
        .map_err(|error| LicenseRefusal {
            reason: error.license_refusal().map(refusal_name).map(str::to_owned),
            message: error.to_string(),
        })?;
    let desktop::Reply::Account(reply) = reply else {
        return Err(unreadable_license());
    };
    let protocol::Reply::RemoteLicense(value) = *reply else {
        return Err(unreadable_license());
    };
    let token = std::str::from_utf8(&value.token.value)
        .map_err(|_| unreadable_license())?
        .to_owned();
    Ok(RemoteLicense {
        token,
        machine: value.machine,
        trusted_time: value.trusted_time,
        expires_at: value.expires_at,
    })
}

fn unreadable_license() -> LicenseRefusal {
    LicenseRefusal {
        reason: None,
        message: "Newport couldn’t read this computer’s license.".into(),
    }
}
fn refusal_name(reason: protocol::RefusalReason) -> &'static str {
    match reason {
        protocol::RefusalReason::SignedOut => "signed_out",
        protocol::RefusalReason::TrialElsewhere => "trial_elsewhere",
        protocol::RefusalReason::FreeSeat => "free_seat",
        protocol::RefusalReason::Free => "free",
        protocol::RefusalReason::Unavailable => "unavailable",
        protocol::RefusalReason::Clock => "clock",
        protocol::RefusalReason::Offline => "offline",
    }
}
fn status_reply(reply: desktop::Reply) -> Outcome {
    let desktop::Reply::Account(reply) = reply else {
        return Err(UNREADABLE_STATUS.into());
    };
    match *reply {
        protocol::Reply::Status(value)
        | protocol::Reply::SignIn(value)
        | protocol::Reply::CancelSignIn(value)
        | protocol::Reply::SignOut(value)
        | protocol::Reply::Refresh(value)
        | protocol::Reply::StartTrial(value) => Ok(Some(*value)),
        protocol::Reply::OpenPortal(_) | protocol::Reply::OpenBilling(_) => Ok(None),
        protocol::Reply::RemoteLicense(_) => Err(UNREADABLE_STATUS.into()),
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
        Action::StartTrial => "Couldn’t start the free trial. Try again.",
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
        sent: Rc<RefCell<Vec<Kind>>>,
        replies: Rc<RefCell<HashMap<Kind, mpsc::SyncSender<Outcome>>>>,
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
                Box::new(move |operation| {
                    let (sender, receiver) = mpsc::sync_channel(1);
                    sent.borrow_mut().push(operation.kind());
                    replies.borrow_mut().insert(operation.kind(), sender);
                    Ok(receiver)
                }),
                Box::new(move || clock.get()),
            )
        }
        fn reply(&self, method: Kind, outcome: Outcome) {
            self.replies
                .borrow_mut()
                .remove(&method)
                .unwrap()
                .send(outcome)
                .unwrap();
        }
        fn advance(&self, by: Duration) {
            self.clock.set(self.clock.get() + by);
        }
        fn sent(&self) -> Vec<Kind> {
            self.sent.borrow().clone()
        }
    }

    fn status(phase: protocol::Phase, notice: Option<(u64, &str)>) -> protocol::Status {
        let signed_in = phase == protocol::Phase::SignedIn;
        protocol::Status {
            state: phase,
            email: signed_in.then(|| "ada@example.com".into()),
            license: signed_in.then(|| protocol::License {
                state: Some("active".into()),
                plan: Some("pro".into()),
                ..Default::default()
            }),
            notice: notice.map(|(id, message)| protocol::Notice {
                id,
                kind: protocol::NoticeKind::Success,
                message: message.into(),
                action: None,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn only_status_replies_carry_a_status() {
        let account = |reply| desktop::Reply::Account(Box::new(reply));
        assert_eq!(
            status_reply(account(protocol::Reply::OpenBilling(Box::new(Present {})))),
            Ok(None)
        );
        let signed_out = status(protocol::Phase::SignedOut, None);
        assert_eq!(
            status_reply(account(protocol::Reply::SignOut(Box::new(
                signed_out.clone()
            )))),
            Ok(Some(signed_out))
        );
        assert_eq!(
            status_reply(account(protocol::Reply::RemoteLicense(Box::new(
                protocol::RemoteLicense {
                    token: newport_protocol::common::Secret {
                        value: Vec::new().into()
                    },
                    machine: String::new(),
                    trusted_time: 0,
                    expires_at: 0,
                }
            )))),
            Err(UNREADABLE_STATUS.into())
        );
    }

    #[test]
    fn reads_status_and_polls_faster_while_waiting() {
        let harness = Harness::default();
        let mut account = harness.account();
        account.poll();
        harness.reply(
            Kind::AccountStatus,
            Ok(Some(status(protocol::Phase::SignedOut, None))),
        );
        assert!(account.poll().changed);
        assert_eq!(account.status().state, protocol::Phase::SignedOut);

        harness.advance(Duration::from_secs(2));
        account.poll();
        assert_eq!(harness.sent(), vec![Kind::AccountStatus]);

        assert!(account.run(Action::SignIn));
        assert!(!account.run(Action::SignIn), "one action at a time");
        harness.reply(
            Kind::AccountSignIn,
            Ok(Some(status(protocol::Phase::Waiting, None))),
        );
        assert!(account.poll().changed);
        assert_eq!(account.status().state, protocol::Phase::Waiting);
        harness.reply(
            Kind::AccountStatus,
            Ok(Some(status(protocol::Phase::Waiting, None))),
        );
        account.poll();
        harness.advance(Duration::from_millis(1100));
        account.poll();
        assert_eq!(
            harness.sent(),
            vec![
                Kind::AccountStatus,
                Kind::AccountSignIn,
                Kind::AccountStatus,
                Kind::AccountStatus
            ]
        );
    }

    #[test]
    fn notices_are_shown_once() {
        let harness = Harness::default();
        let mut account = harness.account();
        account.poll();
        harness.reply(
            Kind::AccountStatus,
            Ok(Some(status(
                protocol::Phase::SignedIn,
                Some((3, "Signed in as ada@example.com.")),
            ))),
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
            Kind::AccountStatus,
            Ok(Some(status(
                protocol::Phase::SignedIn,
                Some((3, "Signed in as ada@example.com.")),
            ))),
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
                .remove(&Kind::AccountStatus)
                .unwrap()
                .send(Ok(Some(status(protocol::Phase::SignedIn, None))))
                .is_err()
        );
        harness.reply(
            Kind::AccountSignOut,
            Err("Couldn’t sign out. Try again.".into()),
        );
        let effects = account.poll();
        assert_eq!(
            effects.notice,
            Some(Notice {
                kind: protocol::NoticeKind::Error,
                message: "Couldn’t sign out. Try again.".into(),
                action: None,
            })
        );
        assert_eq!(account.pending(), None);
        assert_eq!(harness.sent().last(), Some(&Kind::AccountStatus));
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
        harness.reply(
            Kind::AccountStatus,
            Ok(Some(status(protocol::Phase::SignedIn, None))),
        );
        account.poll();

        account.refresh();
        assert_eq!(harness.sent().last(), Some(&Kind::AccountRefresh));
        assert_eq!(account.pending(), None);
        assert!(
            account.run(Action::SignOut),
            "a refresh doesn't hold the action slot"
        );

        let mut denied = status(protocol::Phase::SignedOut, None);
        denied.notice = Some(protocol::Notice {
            id: 9,
            kind: protocol::NoticeKind::Error,
            message: "Couldn’t sign in.".into(),
            action: Some(protocol::NoticeAction::TryAgain),
        });
        harness.reply(Kind::AccountSignOut, Ok(Some(denied)));
        let notice = account.poll().notice.unwrap();
        assert_eq!(notice.action, Some(protocol::NoticeAction::TryAgain));
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
