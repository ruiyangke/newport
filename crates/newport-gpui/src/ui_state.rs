//! Where the window and workspace were left, in one file the native client
//! owns: `ui-state.json` in the profile. It is disposable: losing it costs a
//! click, unlike preferences, which the backend keeps in `preferences.json`.
//!
//! One background writer saves it. The window and the workspace send their
//! part as it changes; a burst becomes one atomic write, and Quit waits for
//! the last one.
use super::{window_state::Placement, workspace_state::Selection};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

const FILE: &str = "ui-state.json";
/// Files of earlier builds, deleted at startup without reading them: losing
/// where you left off is cheap.
const RETIRED: &[&str] = &["gpui-workspace-state.json", "gpui-window-state.json"];
/// Changes closer together than this are written once.
const SETTLE: Duration = Duration::from_millis(300);

#[derive(Default, Serialize, Deserialize)]
pub(super) struct UiState {
    #[serde(default)]
    pub window: Option<Placement>,
    #[serde(default)]
    pub workspace: Selection,
    /// The held shell this Mac last had selected on each server, by server
    /// id. The shells, their order and their names are the server's.
    #[serde(default)]
    pub terminals: BTreeMap<String, SavedTerminals>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct SavedTerminals {
    /// The selected tab's terminal.
    #[serde(default)]
    pub selected: Option<uuid::Uuid>,
}

fn path() -> Option<PathBuf> {
    newport_client::profile::directory().map(|base| base.join(FILE))
}

/// The saved state, or defaults. Also removes earlier builds' files and
/// temporary files a crash left behind: one Newport runs per profile, so any
/// temporary file at startup is an orphan.
pub(super) fn load() -> UiState {
    let Some(path) = path() else {
        return UiState::default();
    };
    if let Some(directory) = path.parent() {
        remove_leftovers(directory);
    }
    match fs::read(&path).and_then(|bytes| decode(&bytes)) {
        Ok(state) => state,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!("Could not restore the window and workspace: {error}");
            }
            UiState::default()
        }
    }
}

fn decode(bytes: &[u8]) -> io::Result<UiState> {
    let mut state: UiState =
        serde_json::from_slice(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    state.workspace = super::workspace_state::sanitize(state.workspace)?;
    Ok(state)
}

fn remove_leftovers(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let temporary = name.ends_with(".tmp")
            && ["ui-state.", "gpui-workspace-state.", "gpui-window-state."]
                .iter()
                .any(|prefix| name.starts_with(prefix));
        if temporary || RETIRED.contains(&name) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn write(path: &Path, state: &UiState) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec(state)?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

enum Message {
    Window(Placement),
    Workspace(Box<Selection>),
    Terminals(String, SavedTerminals),
    /// Write what is pending now, then answer.
    Flush(mpsc::Sender<()>),
}

/// Saves the state off the UI thread. Without a profile directory it does
/// nothing. Clones send to the same writer.
#[derive(Clone)]
pub(super) struct Writer {
    sender: Option<mpsc::Sender<Message>>,
}

impl Writer {
    pub fn start(state: UiState) -> Self {
        Self::start_at(path(), state)
    }

    fn start_at(path: Option<PathBuf>, state: UiState) -> Self {
        let Some(path) = path else {
            return Self { sender: None };
        };
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("ui-state".into())
            .spawn(move || run(&path, state, &receiver))
            .map_or_else(
                |error| {
                    eprintln!("UI state is not saved this session: {error}");
                    Self { sender: None }
                },
                |_| Self {
                    sender: Some(sender),
                },
            )
    }

    /// A writer that saves nothing, for tests that build a window.
    #[cfg(test)]
    pub fn disabled() -> Self {
        Self { sender: None }
    }

    pub fn window(&self, placement: Placement) {
        self.send(Message::Window(placement));
    }

    pub fn workspace(&self, selection: Selection) {
        self.send(Message::Workspace(Box::new(selection)));
    }

    /// A server's tab order and selection; no tabs forgets the server's.
    pub fn terminals(&self, server: String, saved: SavedTerminals) {
        self.send(Message::Terminals(server, saved));
    }

    /// Waits until everything sent so far is on disk, as Quit needs.
    pub fn flush(&self) {
        let (done, wait) = mpsc::channel();
        if self.send(Message::Flush(done)) {
            let _ = wait.recv_timeout(Duration::from_secs(5));
        }
    }

    fn send(&self, message: Message) -> bool {
        self.sender
            .as_ref()
            .is_some_and(|sender| sender.send(message).is_ok())
    }
}

fn run(path: &Path, mut state: UiState, receiver: &mpsc::Receiver<Message>) {
    let save = |state: &UiState| {
        if let Err(error) = write(path, state) {
            eprintln!("Could not save the window and workspace: {error}");
        }
    };
    // Waiting for the first change of a burst, then for the burst to end.
    let mut dirty = false;
    loop {
        let message = if dirty {
            match receiver.recv_timeout(SETTLE) {
                Ok(message) => message,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    save(&state);
                    dirty = false;
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match receiver.recv() {
                Ok(message) => message,
                Err(_) => return,
            }
        };
        match message {
            Message::Window(placement) => {
                state.window = Some(placement);
                dirty = true;
            }
            Message::Workspace(selection) => {
                state.workspace = *selection;
                dirty = true;
            }
            Message::Terminals(server, saved) => {
                if saved.selected.is_none() {
                    state.terminals.remove(&server);
                } else {
                    state.terminals.insert(server, saved);
                }
                dirty = true;
            }
            Message::Flush(done) => {
                if dirty {
                    save(&state);
                    dirty = false;
                }
                let _ = done.send(());
            }
        }
    }
    if dirty {
        save(&state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Page;

    fn selection(server: &str) -> Selection {
        Selection {
            server: server.into(),
            page: Page::Files,
            workspace_page: Page::Files,
            ..Default::default()
        }
    }

    /// A folder of its own, removed when the test ends.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("newport-ui-state-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn saved(path: &Path) -> UiState {
        decode(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn a_burst_of_changes_is_one_state_on_disk_after_flush() {
        let directory = Scratch::new();
        let path = directory.path().join(FILE);
        let writer = Writer::start_at(Some(path.clone()), UiState::default());
        for n in 0..50 {
            writer.workspace(selection(&format!("server-{n}")));
        }
        writer.window(Placement::new(10., 20., 900., 700.));
        writer.flush();
        let state = saved(&path);
        assert_eq!(state.workspace.server, "server-49");
        assert_eq!(state.window.unwrap().width(), 900.);
        // Only the state itself: no temporary file stays behind.
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn one_part_changing_keeps_the_other() {
        let directory = Scratch::new();
        let path = directory.path().join(FILE);
        let restored = UiState {
            window: Some(Placement::new(1., 2., 800., 600.)),
            workspace: selection("kept"),
            terminals: Default::default(),
        };
        let writer = Writer::start_at(Some(path.clone()), restored);
        writer.window(Placement::new(5., 5., 1000., 700.));
        writer.flush();
        let state = saved(&path);
        assert_eq!(state.workspace.server, "kept");
        assert_eq!(state.window.unwrap().width(), 1000.);
    }

    #[test]
    fn each_servers_tabs_are_kept_until_it_has_none() {
        let directory = Scratch::new();
        let path = directory.path().join(FILE);
        let writer = Writer::start_at(Some(path.clone()), UiState::default());
        let tabs = || SavedTerminals {
            selected: Some(uuid::Uuid::new_v4()),
        };
        let first = tabs();
        writer.terminals("a".into(), first.clone());
        writer.terminals("b".into(), tabs());
        writer.flush();
        let state = saved(&path);
        assert_eq!(state.terminals["a"], first);
        assert_eq!(state.terminals.len(), 2);
        writer.terminals("b".into(), SavedTerminals::default());
        writer.flush();
        let state = saved(&path);
        assert!(!state.terminals.contains_key("b"));
        assert_eq!(state.terminals["a"], first);
        // A file from before terminals were saved still loads.
        assert!(decode(b"{}").unwrap().terminals.is_empty());
    }

    #[test]
    fn changes_are_written_without_a_flush_once_they_settle() {
        let directory = Scratch::new();
        let path = directory.path().join(FILE);
        let writer = Writer::start_at(Some(path.clone()), UiState::default());
        writer.workspace(selection("settled"));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(saved(&path).workspace.server, "settled");
        drop(writer);
    }

    #[test]
    fn startup_removes_retired_and_orphaned_files_only() {
        let directory = Scratch::new();
        let keep = ["ui-state.json", "preferences.json", "notes.tmp"];
        let remove = [
            "gpui-workspace-state.json",
            "gpui-window-state.json",
            "gpui-workspace-state.317a3693.tmp",
            "ui-state.0b1c.tmp",
        ];
        for name in keep.iter().chain(&remove) {
            fs::write(directory.path().join(name), b"{}").unwrap();
        }
        remove_leftovers(directory.path());
        for name in keep {
            assert!(directory.path().join(name).exists(), "{name} was removed");
        }
        for name in remove {
            assert!(!directory.path().join(name).exists(), "{name} stayed");
        }
    }

    #[test]
    fn a_damaged_file_restores_nothing_rather_than_failing() {
        assert!(decode(b"{not json").is_err());
        assert!(
            decode(br#"{"workspace":{"server":"x","page":"removed","workspace_page":"files"}}"#)
                .is_err()
        );
        let empty = decode(b"{}").unwrap();
        assert!(empty.window.is_none());
        assert!(empty.workspace.server.is_empty());
    }
}
