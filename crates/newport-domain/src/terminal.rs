/// Product resource bound, independent of sshd's per-connection session limit.
pub const MAX_TERMINALS: usize = 32;

/// A Mac as holders know it: its integration routing id for the server, and
/// the name people gave the computer.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mac {
    pub id: String,
    pub name: String,
}

/// A Mac attached to a held shell now. Lists put the most recently used
/// first.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AttachedMac {
    pub id: String,
    pub name: String,
    /// The Mac someone last used the shell on: it sets the shell's size, and
    /// the shell's requests go there first.
    pub active: bool,
}

/// A shell held on a server, as every Mac connected to it lists it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HeldShell {
    pub terminal: uuid::Uuid,
    /// Set when someone renamed the shell.
    pub name: Option<String>,
    /// The title the shell's program set last, for a tab not attached yet.
    pub title: Option<String>,
    /// What the shell's coding agent last reported, for a tab not attached
    /// yet: an OSC 7501 program status report.
    pub agent: Option<String>,
    pub creator: Mac,
    pub attached: Vec<AttachedMac>,
    /// Seconds since the Unix epoch.
    pub started: u64,
    /// The shell has exited, and its holder waits for a Mac to see how.
    pub exited: bool,
}

/// A server's held shells, for `terminal_list`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HeldShells {
    /// This Mac's id, as it appears in `attached`.
    pub desktop: String,
    /// The server holds shells: Needle is set up and approved there, and its
    /// list hasn't failed to come. Until the list arrives, a Mac waits for it
    /// rather than start a shell.
    #[serde(default)]
    pub holds: bool,
    /// `None` until the server has answered, and where it can't hold shells.
    pub shells: Option<Vec<HeldShell>>,
}
