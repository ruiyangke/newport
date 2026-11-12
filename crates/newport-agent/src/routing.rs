//! Integration routing. A request never guesses between computers. A
//! Newport terminal's requests go to the Mac it is used on, chosen when each
//! request is made: a held shell's active Mac, or the Mac
//! a plain terminal runs on. A program on the account's stable display
//! follows the Mac viewing the display. Anything else goes to the only
//! connected Mac.
mod history;
use serde::{Deserialize, Serialize};
use std::{
    env, fs, io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The folder of the stable display a program runs on.
pub(crate) const STABLE_DISPLAY: &str = "NEWPORT_DISPLAY";
#[derive(Clone)]
pub(crate) struct Scope {
    pub session: String,
    pub runtime: String,
    pub directory: PathBuf,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    session: String,
    runtime: String,
    client: String,
    root: PathBuf,
}
pub(crate) struct Selection {
    pub root: PathBuf,
    /// For a terminal or the stable display, the Mac it follows, by name.
    pub mac: Option<String>,
}
impl Selection {
    /// Why a clipboard read here failed. A followed Mac is named, and one
    /// that doesn't share its own clipboard, serving only what apps on the
    /// server copied, says how to share it.
    pub(crate) fn clipboard_unavailable(&self, error: io::Error) -> io::Error {
        let message = match &self.mac {
            Some(mac) if !shares_clipboard(&self.root) => refused(mac, "clipboard"),
            Some(mac) => format!("{mac}’s clipboard is unavailable: {error}"),
            None => return error,
        };
        io::Error::new(error.kind(), message)
    }
}

/// The Mac shares its own clipboard, not only the server's local one.
fn shares_clipboard(root: &Path) -> bool {
    fs::read_to_string(root.join("features"))
        .is_ok_and(|features| features.split_whitespace().any(|f| f == "clipboard"))
}
// Holding the published file's lock protects it before features become active
// and while permissions are disabled. Kernel release also detects SIGKILL.
pub(crate) struct Publication {
    path: PathBuf,
    identity: (u64, u64),
    _lock: fs::File,
    record: Record,
    next_check: std::time::Instant,
}
impl Publication {
    pub(crate) fn check_current(&mut self) -> io::Result<()> {
        let now = std::time::Instant::now();
        if now < self.next_check {
            return Ok(());
        }
        self.next_check = now + std::time::Duration::from_secs(1);
        if !history::is_current(&self.record)? {
            return Err(io::Error::other(
                "Integration session superseded by a newer connection",
            ));
        }
        Ok(())
    }
}
impl Drop for Publication {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|m| (m.dev(), m.ino()) == self.identity) {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn directory() -> io::Result<PathBuf> {
    Ok(
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?)
            .join(".cache/newport/sessions"),
    )
}
fn private(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| {
        m.is_dir() && m.uid() == crate::paths::effective_uid() && m.mode() & 0o077 == 0
    })
}
pub(crate) fn publish(scope: &Scope, client: &str, root: &Path) -> io::Result<Publication> {
    let directory = directory()?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)?;
    if !private(&directory) {
        return Err(io::Error::other(
            "Integration registry is not private to this account",
        ));
    }
    // Best effort: a bad unrelated record must not prevent a new session.
    let previous = records().unwrap_or_default();
    let file = tempfile::Builder::new()
        .prefix("session-")
        .suffix(".json")
        .tempfile_in(directory)?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    fs2::FileExt::try_lock_exclusive(file.as_file())?;
    let record = Record {
        session: scope.session.clone(),
        runtime: scope.runtime.clone(),
        client: client.into(),
        root: root.into(),
    };
    serde_json::to_writer(file.as_file(), &record)?;
    history::publish(&record, &previous)?;
    let m = file.as_file().metadata()?;
    let (file, path) = file.keep().map_err(io::Error::other)?;
    Ok(Publication {
        path,
        identity: (m.dev(), m.ino()),
        _lock: file,
        record,
        next_check: std::time::Instant::now(),
    })
}
fn active(root: &Path) -> bool {
    if !private(root) {
        return false;
    }
    let Ok(lock) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join("agent.sock.lock"))
    else {
        return false;
    };
    if !lock.metadata().is_ok_and(|m| {
        m.is_file() && m.uid() == crate::paths::effective_uid() && m.mode() & 0o077 == 0
    }) {
        return false;
    }
    matches!(fs2::FileExt::try_lock_exclusive(&lock), Err(e) if e.kind() == io::ErrorKind::WouldBlock)
}
fn enabled(root: &Path, capability: Option<&str>) -> bool {
    fs::read_to_string(root.join("features")).is_ok_and(|features| match capability {
        Some(capability) => features
            .split_whitespace()
            .any(|f| f == capability || capability == "clipboard" && f == "clipboard-local"),
        // Local broker reads do not enable host integrations or their display
        // environment. Only clipboard commands request this capability.
        None => features.split_whitespace().any(|f| f != "clipboard-local"),
    })
}
/// The one connection permitting `capability`, or the standalone bridge,
/// for a request that follows no Mac.
pub(crate) fn select(capability: Option<&str>) -> io::Result<Selection> {
    let mut candidates = history::reconcile(records()?)?
        .into_iter()
        .filter(|r| active(&r.root) && enabled(&r.root, capability))
        .map(|r| Selection {
            root: r.root,
            mac: None,
        })
        .collect::<Vec<_>>();
    // A standalone bridge is another possible destination.
    let home =
        PathBuf::from(env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?);
    let standalone = crate::paths::standalone_root(&home);
    if active(&standalone) && enabled(&standalone, capability) {
        candidates.push(Selection {
            root: standalone,
            mac: None,
        });
    }
    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 => Err(io::Error::other(
            "No permitted Newport integration is active for this session. Enable it in Newport.",
        )),
        _ => Err(io::Error::other(
            "Several computers are connected to this server. Run this in a Newport terminal on the computer you want.",
        )),
    }
}
/// How long a terminal's request waits for its Mac to take it.
pub(crate) const ROUTE_WAIT: Duration = Duration::from_secs(10);
const NO_MAC: &str = "Open this terminal in Newport to continue.";
const NO_VIEWER: &str = "Show this server's apps in Newport to continue.";

/// What a request that names no session follows, chosen when it is made.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Follow {
    /// The Mac viewing the stable display in this folder.
    Viewer(PathBuf),
    /// The Mac someone last used this terminal on: a held shell's active
    /// Mac, or the one a plain terminal runs on.
    Terminal(String),
}

impl Follow {
    pub(crate) fn current() -> Option<Self> {
        Self::from_environment(|name| env::var_os(name))
    }
    /// In order: the stable display's folder, then `NEWPORT_TERMINAL`.
    /// Otherwise a request follows no Mac. A terminal whose holder and route
    /// are gone still follows its own Mac, so its requests fail rather than
    /// go to another.
    fn from_environment(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<Self> {
        if let Some(folder) = var(STABLE_DISPLAY)
            .map(PathBuf::from)
            .filter(|folder| folder.is_absolute())
        {
            return Some(Self::Viewer(folder));
        }
        let terminal = var("NEWPORT_TERMINAL")?.into_string().ok()?;
        Some(Self::Terminal(terminal))
    }
    pub(crate) fn select_once(&self, capability: &str) -> io::Result<Held> {
        match self {
            Self::Viewer(folder) => select_viewer_once(folder, capability),
            Self::Terminal(terminal) => select_held_once(terminal, capability),
        }
    }
    /// Why a request that waited `ROUTE_WAIT` fails.
    pub(crate) fn unavailable(&self, waiting: Option<&str>, capability: &str) -> io::Error {
        match (self, waiting) {
            (Self::Viewer(_), None) => io::Error::other(NO_VIEWER),
            _ => Held::unavailable(waiting, capability),
        }
    }
    /// As `select_once`, waiting up to `ROUTE_WAIT` for the Mac to be ready,
    /// as after a wake or a reattach, and following another Mac if one takes
    /// its place meanwhile.
    pub(crate) fn select(&self, capability: &str) -> io::Result<Selection> {
        wait_until(
            Instant::now() + ROUTE_WAIT,
            || self.select_once(capability),
            |waiting| self.unavailable(waiting, capability),
        )
    }
}

/// Asks `once` until a Mac is ready, or fails as `unavailable` says once
/// `deadline` passes.
fn wait_until(
    deadline: Instant,
    mut once: impl FnMut() -> io::Result<Held>,
    unavailable: impl Fn(Option<&str>) -> io::Error,
) -> io::Result<Selection> {
    loop {
        match once()? {
            Held::Ready(selection) => return Ok(selection),
            Held::Waiting(mac) if Instant::now() >= deadline => {
                return Err(unavailable(mac.as_deref()));
            }
            Held::Waiting(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// Where a terminal's request goes now.
pub(crate) enum Held {
    /// The Mac someone last used the terminal on, connected and permitting it.
    Ready(Selection),
    /// That Mac has no integration connection yet, or no Mac is attached:
    /// its name, when there is one.
    Waiting(Option<String>),
}

impl Held {
    /// Why a request that waited `ROUTE_WAIT` fails.
    pub(crate) fn unavailable(waiting: Option<&str>, capability: &str) -> io::Error {
        io::Error::other(match waiting {
            Some(mac) => refused(mac, capability),
            None => NO_MAC.into(),
        })
    }
}

/// Resolves a terminal's request to the Mac someone last used it on: the
/// first its holder lists, or the one a plain terminal's route names.
/// Requests follow that Mac only: another attached Mac would act on a screen
/// nobody is looking at. Its id is resolved through the route history to its
/// current connection, which may be newer than the terminal, but never to a
/// standalone bridge.
pub(crate) fn select_held_once(terminal: &str, capability: &str) -> io::Result<Held> {
    let Some(attached) = crate::terminal::attached(terminal) else {
        return Err(io::Error::other(NO_MAC));
    };
    choose(&history::directory()?, &records()?, &attached, capability)
}

/// The Mac viewing the stable display in `folder`, as its `viewer.json`
/// names it while one views.
#[derive(Deserialize)]
pub(crate) struct Viewer {
    /// The Mac's session on this server, unique to the Mac.
    pub(crate) session: String,
    /// Empty when the Mac gave no name.
    pub(crate) computer: String,
}

/// The stable display's viewer in `folder`, while one views it.
pub(crate) fn viewer(folder: &Path) -> Option<Viewer> {
    // Written whole on every takeover; absent while none views.
    fs::read(folder.join("viewer.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

/// Resolves a request from the stable display to the Mac viewing it, as a
/// terminal's goes to its Mac.
pub(crate) fn select_viewer_once(folder: &Path, capability: &str) -> io::Result<Held> {
    choose_viewer(&history::directory()?, &records()?, folder, capability)
}

fn choose_viewer(
    history: &Path,
    records: &[Record],
    folder: &Path,
    capability: &str,
) -> io::Result<Held> {
    let Some(viewer) = viewer(folder) else {
        return Ok(Held::Waiting(None));
    };
    resolve(
        history,
        records,
        Some(&newport_protocol::terminal::Attached {
            id: viewer.session,
            name: viewer.computer,
            active: true,
        }),
        capability,
    )
}

fn choose(
    history: &Path,
    records: &[Record],
    attached: &[newport_protocol::terminal::Attached],
    capability: &str,
) -> io::Result<Held> {
    resolve(history, records, attached.first(), capability)
}

fn resolve(
    history: &Path,
    records: &[Record],
    mac: Option<&newport_protocol::terminal::Attached>,
    capability: &str,
) -> io::Result<Held> {
    let Some(mac) = mac else {
        return Ok(Held::Waiting(None));
    };
    let waiting = Held::Waiting(Some(mac.name.clone()));
    if uuid::Uuid::parse_str(&mac.id).is_err() {
        return Ok(waiting);
    }
    // Attached, so connected: without an integration connection it is still
    // starting one, or runs none because its integrations are off. A Mac
    // whose route history can't be read can't be resolved either.
    let routed: Vec<_> = history::reconcile_in(history, records.to_vec(), Some(&mac.id))
        .unwrap_or_default()
        .into_iter()
        .filter(|r| active(&r.root))
        .collect();
    let [record] = routed.as_slice() else {
        return Ok(waiting);
    };
    // A connection publishes its features once it is ready.
    if !record.root.join("features").exists() {
        return Ok(waiting);
    }
    if !enabled(&record.root, Some(capability)) {
        return Err(io::Error::other(refused(&mac.name, capability)));
    }
    Ok(Held::Ready(Selection {
        root: record.root.clone(),
        mac: Some(mac.name.clone()),
    }))
}

/// The selection for a request needing `capability`: the Mac it follows, or
/// else as `select` chooses.
pub(crate) fn select_for(capability: &str) -> io::Result<Selection> {
    match Follow::current() {
        Some(follow) => follow.select(capability),
        None => select(Some(capability)),
    }
}

fn refused(mac: &str, capability: &str) -> String {
    let mac = if mac.is_empty() {
        "The computer in use"
    } else {
        mac
    };
    match capability {
        "browser" => format!(
            "{mac} doesn’t open links from this server. Turn on Browser links on Newport’s Integrations page there."
        ),
        "clipboard" => format!(
            "{mac} doesn’t share its clipboard with this server. Turn on Clipboard access on Newport’s Integrations page there."
        ),
        _ => format!("{mac} can’t open apps from this server."),
    }
}

fn records() -> io::Result<Vec<Record>> {
    let directory = directory()?;
    if !directory.exists() {
        return Ok(Vec::new());
    }
    if !private(&directory) {
        return Err(io::Error::other(
            "Integration registry is not private to this account",
        ));
    }
    let mut records = Vec::new();
    for (index, entry) in fs::read_dir(directory)?.enumerate() {
        if index >= 4096 {
            return Err(io::Error::other("Integration registry needs cleanup"));
        }
        let path = entry?.path();
        let Ok(file) = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        else {
            continue;
        };
        let m = file.metadata()?;
        if !m.is_file()
            || m.uid() != crate::paths::effective_uid()
            || m.mode() & 0o077 != 0
            || m.len() > 4096
        {
            continue;
        }
        let Ok(record) = serde_json::from_reader::<_, Record>(&file) else {
            continue;
        };
        if uuid::Uuid::parse_str(&record.session).is_ok()
            && uuid::Uuid::parse_str(&record.runtime).is_ok()
            && record.root.is_absolute()
        {
            // A record whose endpoint no longer holds its ownership lock is stale.
            // Delete only our recognized, unchanged record.
            if !active(&record.root)
                && path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with("session-") && s.ends_with(".json"))
                && fs2::FileExt::try_lock_exclusive(&file).is_ok()
                && fs::symlink_metadata(&path)
                    .is_ok_and(|current| (current.dev(), current.ino()) == (m.dev(), m.ino()))
            {
                let _ = fs::remove_file(&path);
                continue;
            }
            records.push(record);
        }
    }
    Ok(records)
}
#[cfg(test)]
mod tests {
    use super::*;
    use newport_protocol::terminal::Attached;

    #[test]
    fn a_held_shells_clipboard_failure_names_its_mac_and_why() {
        let dir = tempfile::tempdir().unwrap();
        let held = |features: &str| {
            fs::write(dir.path().join("features"), features).unwrap();
            Selection {
                root: dir.path().into(),
                mac: Some("Air".into()),
            }
        };
        let failed = || io::Error::other("clipboard request failed");
        // Sharing off: only the server's own clipboard, which had nothing.
        let error = held("clipboard-local").clipboard_unavailable(failed());
        assert!(
            error
                .to_string()
                .starts_with("Air doesn’t share its clipboard"),
            "{error}"
        );
        let error = held("clipboard clipboard-local").clipboard_unavailable(failed());
        assert_eq!(
            error.to_string(),
            "Air’s clipboard is unavailable: clipboard request failed"
        );
        // A request that follows no Mac keeps its own error.
        let mut chosen = held("clipboard-local");
        chosen.mac = None;
        let error = chosen.clipboard_unavailable(failed());
        assert_eq!(error.to_string(), "clipboard request failed");
    }

    /// A connection's directory, live while the returned lock is held.
    fn connection(root: &Path, features: Option<&str>) -> fs::File {
        fs::DirBuilder::new().mode(0o700).create(root).unwrap();
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(root.join("agent.sock.lock"))
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&lock).unwrap();
        if let Some(features) = features {
            fs::write(root.join("features"), features).unwrap();
        }
        lock
    }
    fn record(session: &str, root: &Path) -> Record {
        Record {
            session: session.into(),
            runtime: uuid::Uuid::new_v4().to_string(),
            client: format!("client-{session}"),
            root: root.into(),
        }
    }
    fn mac(id: &str, name: &str) -> Attached {
        Attached {
            id: id.into(),
            name: name.into(),
            active: false,
        }
    }
    #[test]
    fn a_held_shells_requests_follow_only_the_mac_last_used() {
        let directory = tempfile::tempdir().unwrap();
        let history = directory.path().join("routes");
        fs::create_dir(&history).unwrap();
        let [studio, air, mini, quiet] = [(); 4].map(|_| uuid::Uuid::new_v4().to_string());
        let studio_root = directory.path().join("studio");
        let air_root = directory.path().join("air");
        let mini_root = directory.path().join("mini");
        let _studio = connection(&studio_root, Some("clipboard browser"));
        let _air = connection(&air_root, Some("clipboard"));
        let _mini = connection(&mini_root, None);
        let records = [
            record(&studio, &studio_root),
            record(&air, &air_root),
            record(&mini, &mini_root),
        ];
        let choose =
            |attached: &[Attached], capability| choose(&history, &records, attached, capability);
        let waiting = |held: Held| match held {
            Held::Waiting(mac) => mac,
            Held::Ready(selection) => panic!("sent to {}", selection.root.display()),
        };

        let attached = [mac(&studio, "Studio"), mac(&air, "Air")];
        match choose(&attached, "browser").unwrap() {
            Held::Ready(selection) => assert_eq!(selection.root, studio_root),
            Held::Waiting(mac) => panic!("waiting for {mac:?}"),
        }
        // A Mac that doesn't permit it fails the request, naming it.
        let attached = [mac(&air, "Air"), mac(&studio, "Studio")];
        let error = choose(&attached, "browser").err().unwrap().to_string();
        assert!(error.starts_with("Air doesn’t open links"), "{error}");
        // One with integrations off runs no integration connection, and one
        // still connecting hasn't published its features: either is waited
        // for, never stood in for by another attached Mac.
        let attached = [mac(&quiet, "Quiet"), mac(&studio, "Studio")];
        assert_eq!(
            waiting(choose(&attached, "browser").unwrap()).as_deref(),
            Some("Quiet")
        );
        let attached = [mac(&mini, "Mini"), mac(&studio, "Studio")];
        assert_eq!(
            waiting(choose(&attached, "browser").unwrap()).as_deref(),
            Some("Mini")
        );
        assert_eq!(waiting(choose(&[], "browser").unwrap()), None);
        let error = Held::unavailable(Some("Quiet"), "browser").to_string();
        assert!(error.starts_with("Quiet doesn’t open links"), "{error}");
    }

    #[test]
    fn the_stable_display_comes_before_a_terminal_and_a_terminal_before_no_mac() {
        let terminal = uuid::Uuid::new_v4().to_string();
        let follow = |vars: &[(&str, &str)]| {
            Follow::from_environment(|name| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.into())
            })
        };
        let folder = "/run/user/1000/newport-display";
        let all = [
            (STABLE_DISPLAY, folder),
            ("NEWPORT_TERMINAL", terminal.as_str()),
        ];
        assert_eq!(follow(&all), Some(Follow::Viewer(folder.into())));
        assert_eq!(follow(&all[1..]), Some(Follow::Terminal(terminal.clone())));
        assert_eq!(follow(&[]), None);
        // A folder that isn't absolute names no display.
        let relative = [
            (STABLE_DISPLAY, "newport-display"),
            ("NEWPORT_TERMINAL", terminal.as_str()),
        ];
        assert_eq!(follow(&relative), Some(Follow::Terminal(terminal)));
    }

    #[test]
    fn the_stable_displays_requests_go_to_its_viewer_or_wait_for_one() {
        let directory = tempfile::tempdir().unwrap();
        let history = directory.path().join("routes");
        fs::create_dir(&history).unwrap();
        let folder = directory.path().join("newport-display");
        fs::create_dir(&folder).unwrap();
        let [studio, air, mini] = [(); 3].map(|_| uuid::Uuid::new_v4().to_string());
        let studio_root = directory.path().join("studio");
        let air_root = directory.path().join("air");
        let mini_root = directory.path().join("mini");
        let _studio = connection(&studio_root, Some("clipboard browser"));
        let _air = connection(&air_root, Some("clipboard"));
        let _mini = connection(&mini_root, None);
        let records = [
            record(&studio, &studio_root),
            record(&air, &air_root),
            record(&mini, &mini_root),
        ];
        let view = |session: &str, computer: &str| {
            fs::write(
                folder.join("viewer.json"),
                serde_json::json!({"session": session, "computer": computer}).to_string(),
            )
            .unwrap();
        };
        let choose = |capability| choose_viewer(&history, &records, &folder, capability);
        let waiting = |held: Held| match held {
            Held::Waiting(mac) => mac,
            Held::Ready(selection) => panic!("sent to {}", selection.root.display()),
        };

        // None views: the request waits, then asks for the display to be shown.
        assert_eq!(waiting(choose("browser").unwrap()), None);
        let follow = Follow::Viewer(folder.clone());
        assert_eq!(
            follow.unavailable(None, "browser").to_string(),
            "Show this server's apps in Newport to continue."
        );
        view(&studio, "Studio");
        match choose("browser").unwrap() {
            Held::Ready(selection) => {
                assert_eq!(selection.root, studio_root);
                assert_eq!(selection.mac.as_deref(), Some("Studio"));
            }
            Held::Waiting(mac) => panic!("waiting for {mac:?}"),
        }
        // A takeover moves the next request to the new viewer.
        view(&air, "Air");
        let error = choose("browser").err().unwrap().to_string();
        assert!(error.starts_with("Air doesn’t open links"), "{error}");
        match choose("clipboard").unwrap() {
            Held::Ready(selection) => assert_eq!(selection.root, air_root),
            Held::Waiting(mac) => panic!("waiting for {mac:?}"),
        }
        // A viewer whose integration connection isn't ready is waited for.
        view(&mini, "Mini");
        assert_eq!(waiting(choose("browser").unwrap()).as_deref(), Some("Mini"));
        let error = follow.unavailable(Some("Mini"), "browser").to_string();
        assert!(error.starts_with("Mini doesn’t open links"), "{error}");
        fs::write(folder.join("viewer.json"), "{").unwrap();
        assert_eq!(waiting(choose("browser").unwrap()), None);
        fs::remove_file(folder.join("viewer.json")).unwrap();
        assert_eq!(waiting(choose("browser").unwrap()), None);
    }

    #[test]
    fn a_request_with_no_viewer_fails_after_the_route_wait() {
        let directory = tempfile::tempdir().unwrap();
        let history = directory.path().join("routes");
        fs::create_dir(&history).unwrap();
        let follow = Follow::Viewer(directory.path().into());
        let started = Instant::now();
        let mut attempts = 0;
        let error = wait_until(
            started + Duration::from_millis(300),
            || {
                attempts += 1;
                choose_viewer(&history, &[], directory.path(), "browser")
            },
            |waiting| follow.unavailable(waiting, "browser"),
        )
        .err()
        .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert!(attempts > 1);
        assert_eq!(
            error.to_string(),
            "Show this server's apps in Newport to continue."
        );
    }
}
