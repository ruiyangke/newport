//! Typed desktop account contracts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Loading,
    SignedOut,
    Waiting,
    SignedIn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Success,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeAction {
    ManageComputers,
    TryAgain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalReason {
    SignedOut,
    TrialElsewhere,
    FreeSeat,
    Free,
    Unavailable,
    Clock,
    Offline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub id: u64,
    pub kind: crate::desktop::account::NoticeKind,
    pub message: String,
    pub action: Option<crate::desktop::account::NoticeAction>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct License {
    pub state: Option<String>,
    pub plan: Option<String>,
    pub seat: Option<String>,
    pub valid_until: Option<String>,
    pub renews_at: Option<String>,
    pub ends_at: Option<String>,
    pub trial_used: Option<bool>,
    pub message: Option<String>,
    pub trial_used_here: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub state: crate::desktop::account::Phase,
    pub email: Option<String>,
    pub name: Option<String>,
    pub license: Option<crate::desktop::account::License>,
    pub signing_out: bool,
    pub notice: Option<crate::desktop::account::Notice>,
    pub license_revision: u64,
}

impl License {
    /// Whether this computer runs Newport Pro: a paid, complimentary or trial
    /// plan, on a seat that isn't free.
    pub fn holds_pro(&self) -> bool {
        matches!(self.state.as_deref(), Some("active" | "grace" | "trial"))
            && self.seat.as_deref() != Some("free")
    }
}

#[derive(Clone, Debug)]
pub struct RemoteLicense {
    pub token: crate::common::Secret,
    pub machine: String,
    pub trusted_time: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Status(Box<crate::common::Present>),
    SignIn(Box<crate::common::Present>),
    CancelSignIn(Box<crate::common::Present>),
    SignOut(Box<crate::common::Present>),
    OpenPortal(Box<crate::common::Present>),
    OpenBilling(Box<crate::common::Present>),
    Refresh(Box<crate::common::Present>),
    StartTrial(Box<crate::common::Present>),
    RemoteLicense(Box<crate::common::Present>),
}

#[derive(Clone, Debug)]
pub enum Reply {
    Status(Box<crate::desktop::account::Status>),
    SignIn(Box<crate::desktop::account::Status>),
    CancelSignIn(Box<crate::desktop::account::Status>),
    SignOut(Box<crate::desktop::account::Status>),
    OpenPortal(Box<crate::common::Present>),
    OpenBilling(Box<crate::common::Present>),
    Refresh(Box<crate::desktop::account::Status>),
    StartTrial(Box<crate::desktop::account::Status>),
    RemoteLicense(Box<crate::desktop::account::RemoteLicense>),
}
