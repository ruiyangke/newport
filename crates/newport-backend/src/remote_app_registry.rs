//! The remote apps registry in the profile, `remote-apps.json` (see
//! `newport_domain::remote_app_registry`), and each app's icon file under
//! `remote-app-icons`, which a taskbar pin's icon resource names.
//!
//! [`RemoteApps`] keeps the registry in memory for the UI thread, which never
//! waits on a file: one worker thread writes the registry and icons in order,
//! and the Start menu shortcuts of the apps the person added (see
//! `newport_domain::remote_app_shortcuts`).
use newport_domain::{
    remote_app_identity as identity,
    remote_app_registry::{self as registry, App, Entry, Registry},
    remote_app_shortcuts::{self as shortcuts, Link},
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};

pub mod ico;

pub const FILE: &str = "remote-apps.json";
/// A registry that couldn't be read, kept for inspection when it is replaced.
pub const DAMAGED: &str = "remote-apps.damaged.json";
pub const ICONS: &str = "remote-app-icons";
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// The registry's files in one profile.
#[derive(Clone, Debug)]
pub struct Store {
    directory: PathBuf,
}

impl Store {
    pub fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
        }
    }

    /// The registry on disk, or an empty one. A file that can't be read as a
    /// registry is kept as [`DAMAGED`] and replaced by an empty registry.
    pub fn load(&self) -> Registry {
        let path = self.directory.join(FILE);
        let bytes = match fs::File::open(&path).and_then(|file| {
            let mut bytes = Vec::new();
            file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
            Ok(bytes)
        }) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Registry::default();
            }
            Err(error) => {
                log::warn!("Cannot read {}: {error}", path.display());
                return Registry::default();
            }
        };
        let read = if bytes.len() as u64 > MAX_BYTES {
            Err("The remote apps registry is too large".to_owned())
        } else {
            Registry::from_json(&bytes)
        };
        match read {
            Ok(registry) => registry,
            Err(error) => {
                log::warn!("{error}; it is kept as {DAMAGED} and replaced by an empty one");
                let empty = Registry::default();
                if let Err(error) = fs::rename(&path, self.directory.join(DAMAGED))
                    .map_err(|error| error.to_string())
                    .and_then(|()| self.save(&empty))
                {
                    log::warn!("Cannot replace the damaged remote apps registry: {error}");
                }
                empty
            }
        }
    }

    /// Writes the registry whole: a temporary file renamed over the old one.
    pub fn save(&self, registry: &Registry) -> Result<(), String> {
        self.publish(&self.directory.join(FILE), &registry.to_json())
    }

    /// The `.ico` file of the app with `id`.
    pub fn icon_path(&self, id: &str) -> Result<PathBuf, String> {
        // Newport's IDs are dotted hex; nothing else becomes a file name.
        if id.is_empty()
            || id.len() > newport_domain::remote_app_identity::MAX_ID_LENGTH
            || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
            || id.starts_with('.')
        {
            return Err(format!("Not a remote app ID: {id}"));
        }
        Ok(self.directory.join(ICONS).join(format!("{id}.ico")))
    }

    /// The `.png` file of the app with `id`, the image its notifications
    /// show.
    pub fn image_path(&self, id: &str) -> Result<PathBuf, String> {
        Ok(self.icon_path(id)?.with_extension("png"))
    }

    /// Writes the app's icon, from an image, as its `.ico` file, and its
    /// largest size as its `.png` file.
    pub fn write_icon(&self, id: &str, image: &[u8]) -> Result<PathBuf, String> {
        let path = self.icon_path(id)?;
        let ico = ico::encode(image)?;
        fs::create_dir_all(self.directory.join(ICONS)).map_err(|error| error.to_string())?;
        if let Some(png) = ico::largest(&ico) {
            self.publish(&self.image_path(id)?, png)?;
        }
        self.publish(&path, &ico)?;
        Ok(path)
    }

    pub fn remove_icon(&self, id: &str) {
        for path in [self.icon_path(id), self.image_path(id)]
            .into_iter()
            .flatten()
        {
            if let Err(error) = fs::remove_file(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                log::warn!("Cannot remove {}: {error}", path.display());
            }
        }
    }

    /// The temporary file sits in the profile folder, whose startup sweep
    /// removes one an interrupted write left.
    fn publish(&self, path: &Path, bytes: &[u8]) -> Result<(), String> {
        fs::create_dir_all(&self.directory).map_err(|error| error.to_string())?;
        let mut file =
            tempfile::NamedTempFile::new_in(&self.directory).map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.as_file()
            .sync_all()
            .map_err(|error| error.to_string())?;
        file.persist(path).map_err(|error| error.to_string())?;
        Ok(())
    }
}

/// What Newport's shortcuts run: this executable, opening this profile.
#[derive(Clone, Debug)]
pub struct Launcher {
    pub executable: PathBuf,
    /// The profile directory Newport was given (`NEWPORT_DATA_DIR` or
    /// `--data-dir`), as `profile::canonical` gives it; `None` for the
    /// default profile.
    pub data_directory: Option<String>,
    /// The Start menu's programs folder and the shell's pins (see
    /// `newport_platform::app_shortcuts::pinned_folder`), `None` where the
    /// system has none.
    pub programs_folder: Option<PathBuf>,
    pub pinned_folder: Option<PathBuf>,
    /// Where notifications look up an app's name and icon (see
    /// `newport_platform::app_shortcuts::APP_IDS_KEY`), `None` to register
    /// none.
    pub app_ids_key: Option<String>,
    /// Whether added apps get jump lists.
    pub jump_lists: bool,
}

impl Launcher {
    /// This executable, opening the profile this process was given.
    pub fn current() -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("Cannot locate Newport's executable: {error}"))?;
        // A shortcut may start Newport in any folder: the path is absolute.
        let data_directory = crate::profile::data_directory_override()
            .map(|directory| directory.to_string_lossy().into_owned());
        Ok(Self {
            executable,
            data_directory,
            programs_folder: newport_platform::app_shortcuts::programs_folder(),
            pinned_folder: newport_platform::app_shortcuts::pinned_folder(),
            app_ids_key: cfg!(windows)
                .then(|| newport_platform::app_shortcuts::APP_IDS_KEY.to_owned()),
            jump_lists: cfg!(windows),
        })
    }

    /// The same executable and profile, touching nothing outside it: no
    /// shortcuts, pins, jump lists or registrations.
    pub fn without_shell(self) -> Self {
        Self {
            programs_folder: None,
            pinned_folder: None,
            app_ids_key: None,
            jump_lists: false,
            ..self
        }
    }

    /// The AppUserModelID of Newport's own windows in this profile.
    pub fn app_id(&self) -> String {
        identity::newport_app_user_model_id(self.data_directory.as_deref())
    }

    fn opens(&self, target: &Path) -> bool {
        // Windows paths ignore case.
        target.to_string_lossy().to_lowercase() == self.executable.to_string_lossy().to_lowercase()
    }
}

/// The key of a server's apps in the profile this process opened (see
/// [`identity::server_key`]).
pub fn server_key(server: &str) -> Result<String, String> {
    let profile = crate::profile::directory()?;
    Ok(identity::server_key(&profile.to_string_lossy(), server))
}

/// An app whose window opened, as [`RemoteApps::record`] keeps it.
#[derive(Clone, Debug)]
pub struct RecordedApp {
    pub id: String,
    pub server: String,
    pub server_name: String,
    pub app: App,
    pub name: String,
    /// Its catalogue's actions; none keeps those known.
    pub actions: Vec<registry::Action>,
}

/// An app the person adds to the Start menu, or takes off it.
#[derive(Clone, Debug)]
pub struct StartMenuApp {
    pub id: String,
    pub server: String,
    pub server_name: String,
    pub app: App,
    pub name: String,
    /// The app's icon image, for its `.ico` file.
    pub icon: Option<Arc<[u8]>>,
    /// Its desktop actions, which its jump list runs.
    pub actions: Vec<registry::Action>,
}

/// An icon file asked for, by the image it was written from.
enum Icon {
    Writing(Arc<[u8]>),
    Written(Arc<[u8]>, PathBuf),
    Failed(Arc<[u8]>),
}

enum Job {
    Save(Registry),
    Icon {
        id: String,
        image: Arc<[u8]>,
    },
    RemoveIcon(String),
    /// Reads the pins, gives Newport's own shortcuts its ID, applies the
    /// registry's limits and brings the Start menu up to date.
    Startup,
    /// Brings the Start menu up to date with the added apps.
    StartMenu,
}

/// The IDs taskbar pins and Start shortcuts use, as last read: their entries
/// stay however long they go unused.
#[derive(Default)]
struct InUse {
    pins: HashSet<String>,
    start: HashSet<String>,
    /// The Mac's launchers: each opens its app by its ID.
    launchers: HashSet<String>,
}

impl InUse {
    fn all(&self) -> HashSet<String> {
        self.pins
            .iter()
            .chain(&self.start)
            .chain(&self.launchers)
            .cloned()
            .collect()
    }
}

struct Inner {
    store: Store,
    launcher: Launcher,
    registry: Mutex<Registry>,
    icons: Mutex<HashMap<String, Icon>>,
    in_use: Mutex<InUse>,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
}

/// The profile's registry, shared by the UI thread and the backend. Reads
/// are in memory; writes go to a worker thread, in order, and so does every
/// shortcut.
#[derive(Clone)]
pub struct RemoteApps {
    inner: Arc<Inner>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

impl RemoteApps {
    /// Reads the registry. Called once the profile is owned, before the UI
    /// starts; the worker then reads the pins, removes entries past their
    /// limits and repairs the shortcuts.
    pub fn open(directory: &Path, launcher: Launcher) -> Self {
        let store = Store::new(directory);
        let registry = store.load();
        let apps = Self {
            inner: Arc::new(Inner {
                store,
                launcher,
                registry: Mutex::new(registry),
                icons: Mutex::default(),
                in_use: Mutex::default(),
                jobs: Mutex::default(),
            }),
        };
        apps.send(Job::Startup);
        apps
    }

    pub fn launcher(&self) -> &Launcher {
        &self.inner.launcher
    }

    pub fn find(&self, id: &str) -> Option<Entry> {
        self.inner
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .find(id)
            .cloned()
    }

    /// Whether the person added the app to the Start menu.
    pub fn is_added(&self, id: &str) -> bool {
        self.find(id).is_some_and(|entry| entry.added)
    }

    /// Every app's server and name, for names that must differ between
    /// servers.
    pub fn names(&self) -> Vec<(String, String)> {
        self.inner
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .iter()
            .map(|entry| (entry.server.clone(), entry.name.clone()))
            .collect()
    }

    /// An app whose window opened, used now, with the actions its catalogue
    /// lists: see [`Registry::record`]. Entries past their limits go, with
    /// their icons; an added app's shortcut and jump list follow its name and
    /// actions.
    pub fn record(&self, app: RecordedApp) {
        let mut registry = self
            .inner
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = app.id.clone();
        let changed = registry.record(Entry {
            id: app.id,
            server: app.server,
            server_name: app.server_name,
            app: app.app,
            name: app.name,
            added: false,
            last_used: now(),
            actions: app.actions,
        });
        let added = changed && registry.find(&id).is_some_and(|entry| entry.added);
        self.clean_up(registry, changed);
        if added {
            self.send(Job::StartMenu);
        }
    }

    /// Adds the app to the Start menu, or takes it off: its shortcut is
    /// written or removed, and its entry kept or made temporary again, since
    /// a pin or a window may still name it. Never waits on the shortcut.
    pub fn set_added(&self, app: StartMenuApp, added: bool) -> Result<(), String> {
        let mut registry = self
            .inner
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let already = registry.find(&app.id).is_some_and(|entry| entry.added);
        if added && !already && registry.added().count() >= registry::MAX_ADDED {
            return Err(format!(
                "You can add up to {} apps to the Start menu.",
                registry::MAX_ADDED
            ));
        }
        let changed = registry.set_added(
            Entry {
                id: app.id.clone(),
                server: app.server,
                server_name: app.server_name,
                app: app.app,
                name: app.name,
                added,
                last_used: now(),
                actions: app.actions,
            },
            added,
        );
        self.clean_up(registry, changed);
        if added && let Some(image) = &app.icon {
            self.icon_file(&app.id, image);
        }
        self.send(Job::StartMenu);
        Ok(())
    }

    /// The IDs the Mac's launchers open their apps by: their entries stay
    /// however long they go unused.
    pub fn set_launchers(&self, ids: HashSet<String>) {
        self.inner
            .in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .launchers = ids;
    }

    /// A server was removed from Newport: its apps' entries, icons and Start
    /// shortcuts go. Pins of them can't be removed; they say the app is gone.
    pub fn forget_server(&self, server: &str) {
        let mut registry = self
            .inner
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let removed = registry.forget_server(server);
        if removed.is_empty() {
            return;
        }
        self.send(Job::Save(registry.clone()));
        drop(registry);
        for id in removed {
            self.inner
                .icons
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
            self.send(Job::RemoveIcon(id));
        }
        self.send(Job::StartMenu);
    }

    /// A server was saved under a new name: its apps take it, and their Start
    /// shortcuts are renamed now rather than at the next start.
    pub fn rename_server(&self, server: &str, name: &str) {
        let mut registry = self
            .inner
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !registry.rename_server(server, name) {
            return;
        }
        self.send(Job::Save(registry.clone()));
        drop(registry);
        self.send(Job::StartMenu);
    }

    /// Removes entries past their limits and their icons, then saves when
    /// anything changed.
    fn clean_up(&self, mut registry: std::sync::MutexGuard<'_, Registry>, changed: bool) {
        let in_use = self
            .inner
            .in_use
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .all();
        let removed = registry::plan_cleanup(&registry, now(), &in_use);
        registry.remove(&removed);
        if changed || !removed.is_empty() {
            self.send(Job::Save(registry.clone()));
        }
        drop(registry);
        for id in removed {
            self.inner
                .icons
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&id);
            self.send(Job::RemoveIcon(id));
        }
    }

    /// The app's `.ico` file once it is written from `image`; until then
    /// `None`, and the write is started. Never waits.
    pub fn icon_file(&self, id: &str, image: &Arc<[u8]>) -> Option<PathBuf> {
        let mut icons = self
            .inner
            .icons
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match icons.get(id) {
            Some(Icon::Written(written, path)) if written == image => return Some(path.clone()),
            Some(Icon::Writing(written) | Icon::Failed(written)) if written == image => {
                return None;
            }
            _ => {}
        }
        icons.insert(id.to_owned(), Icon::Writing(image.clone()));
        drop(icons);
        self.send(Job::Icon {
            id: id.to_owned(),
            image: image.clone(),
        });
        None
    }

    fn send(&self, job: Job) {
        let mut jobs = self
            .inner
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sender = jobs.get_or_insert_with(|| {
            let (sender, receiver) = mpsc::channel();
            let store = self.inner.store.clone();
            let inner = Arc::downgrade(&self.inner);
            if let Err(error) = std::thread::Builder::new()
                .name("remote-app-registry".into())
                .spawn(move || work(&store, &inner, &receiver))
            {
                log::warn!("Remote apps registry writes unavailable: {error}");
            }
            sender
        });
        let _ = sender.send(job);
    }

    /// Waits until every write asked for so far is done: they run in
    /// order, and a last icon for no app fails.
    #[cfg(test)]
    fn settle(&self) {
        let marker: Arc<[u8]> = Arc::from(&b""[..]);
        self.inner
            .icons
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(String::new(), Icon::Writing(marker.clone()));
        self.send(Job::Icon {
            id: String::new(),
            image: marker,
        });
        let started = std::time::Instant::now();
        while !matches!(
            self.inner
                .icons
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(""),
            Some(Icon::Failed(_))
        ) {
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// Writes queued before the registry was dropped still happen; what needs
/// the registry itself is skipped once it is gone.
fn work(store: &Store, inner: &Weak<Inner>, jobs: &mpsc::Receiver<Job>) {
    let mut shell = Shell::default();
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Save(registry) => {
                if let Err(error) = store.save(&registry) {
                    log::warn!("Cannot save the remote apps registry: {error}");
                }
            }
            Job::Icon { id, image } => {
                let missing = |path: Result<PathBuf, String>| path.is_ok_and(|path| !path.exists());
                let new = missing(store.icon_path(&id)) || missing(store.image_path(&id));
                let written = store.write_icon(&id, &image);
                let Some(inner) = inner.upgrade() else {
                    continue;
                };
                // An added app's shortcut, jump list and notifications show
                // its icon once there is one.
                if new
                    && written.is_ok()
                    && inner
                        .registry
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .find(&id)
                        .is_some_and(|entry| entry.added)
                {
                    sync_start_menu(&inner);
                    shell.sync(&inner);
                }
                let mut icons = inner
                    .icons
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // A newer image asked for meanwhile has its own write queued.
                if matches!(icons.get(&id), Some(Icon::Writing(asked)) if *asked == image) {
                    let icon = match written {
                        Ok(path) => Icon::Written(image, path),
                        Err(error) => {
                            log::warn!("Cannot write the icon of {id}: {error}");
                            Icon::Failed(image)
                        }
                    };
                    icons.insert(id, icon);
                }
            }
            Job::RemoveIcon(id) => store.remove_icon(&id),
            Job::Startup => {
                let Some(inner) = inner.upgrade() else {
                    continue;
                };
                inner
                    .in_use
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pins = read_pins(&inner);
                sync_start_menu(&inner);
                shell.sync(&inner);
                let in_use = inner
                    .in_use
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .all();
                let mut registry = inner
                    .registry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let removed = registry::plan_cleanup(&registry, now(), &in_use);
                if removed.is_empty() {
                    continue;
                }
                registry.remove(&removed);
                let snapshot = registry.clone();
                drop(registry);
                // A later change queues its own save, after this one.
                if let Err(error) = store.save(&snapshot) {
                    log::warn!("Cannot save the remote apps registry: {error}");
                }
                for id in removed {
                    inner
                        .icons
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&id);
                    store.remove_icon(&id);
                }
            }
            Job::StartMenu => {
                if let Some(inner) = inner.upgrade() {
                    sync_start_menu(&inner);
                    shell.sync(&inner);
                }
            }
        }
    }
}

/// The IDs of the current user's taskbar pins: their shortcuts in
/// `TaskBar`, and those the shell made for pinned windows. Pins and the Start
/// menu shortcut of Newport itself, in this profile, get its ID first: pins
/// made before Newport had one would otherwise group its windows apart. Pins
/// of this profile's apps are repaired (see [`repair_pin`]).
fn read_pins(inner: &Inner) -> HashSet<String> {
    use newport_platform::app_shortcuts;
    let launcher = &inner.launcher;
    let Some(pinned) = &launcher.pinned_folder else {
        return HashSet::new();
    };
    let pins = app_shortcuts::list(&pinned.join("TaskBar"));
    let implicit: Vec<PathBuf> = fs::read_dir(pinned.join("ImplicitAppShortcuts"))
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|folder| app_shortcuts::list(&folder.path()))
        .collect();
    let installed = launcher
        .programs_folder
        .as_ref()
        .map(|folder| folder.join(NEWPORT_SHORTCUT))
        .filter(|path| path.is_file());
    let ours = launcher.app_id();
    let mut ids = HashSet::new();
    for path in &implicit {
        if let Ok(found) = app_shortcuts::read(path)
            && !found.app_id.is_empty()
        {
            repair_pin(inner, path, link(found.clone()));
            ids.insert(found.app_id);
        }
    }
    for path in pins.iter().chain(&installed) {
        let Ok(found) = app_shortcuts::read(path) else {
            continue;
        };
        if launcher.opens(&found.target)
            && identity::needs_newport_id(
                &found.arguments,
                &found.app_id,
                launcher.data_directory.as_deref(),
            )
        {
            match app_shortcuts::set_app_id(path, &ours) {
                Ok(()) => log::info!("Gave {} Newport's taskbar ID", path.display()),
                Err(error) => log::warn!("{error}"),
            }
        } else if !found.app_id.is_empty() {
            repair_pin(inner, path, link(found.clone()));
            ids.insert(found.app_id);
        }
    }
    ids
}

/// The shell keeps a pin's shortcut, and ignores the command a window
/// offers once it has one: a pin of one of this profile's apps whose
/// Newport moved, whose profile option is missing or another, or whose icon
/// is gone, is rewritten in place, keeping its ID and file name. Pins of
/// other profiles' apps, and IDs the registry doesn't know, are left alone.
fn repair_pin(inner: &Inner, path: &Path, found: Link) {
    if inner
        .registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .find(&found.app_id)
        .is_none()
    {
        return;
    }
    let icon = inner
        .store
        .icon_path(&found.app_id)
        .map(|icon| icon.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(repaired) = shortcuts::repaired_pin(
        &found,
        &inner.launcher.executable.to_string_lossy(),
        inner.launcher.data_directory.as_deref(),
        &icon,
        |file| Path::new(file).is_file(),
    ) else {
        return;
    };
    match newport_platform::app_shortcuts::write(path, &shortcut(&repaired)) {
        Ok(()) => log::info!("Repaired the pin {}", path.display()),
        Err(error) => log::warn!("{error}"),
    }
}

/// Where the added apps' shortcuts go.
fn start_menu_folder(launcher: &Launcher) -> Option<PathBuf> {
    launcher
        .programs_folder
        .as_ref()
        .map(|folder| folder.join(shortcuts::FOLDER))
}

/// The shortcut the installer puts in the Start menu.
const NEWPORT_SHORTCUT: &str = "Newport.lnk";

fn link(shortcut: newport_platform::app_shortcuts::Shortcut) -> Link {
    Link {
        target: shortcut.target.to_string_lossy().into_owned(),
        arguments: shortcut.arguments,
        icon: shortcut.icon.map_or((String::new(), 0), |(path, index)| {
            (path.to_string_lossy().into_owned(), index)
        }),
        description: shortcut.description,
        app_id: shortcut.app_id,
    }
}

fn shortcut(link: &Link) -> newport_platform::app_shortcuts::Shortcut {
    newport_platform::app_shortcuts::Shortcut {
        target: PathBuf::from(&link.target),
        arguments: link.arguments.clone(),
        icon: (!link.icon.0.is_empty()).then(|| (PathBuf::from(&link.icon.0), link.icon.1)),
        description: link.description.clone(),
        app_id: link.app_id.clone(),
    }
}

/// Brings `Newport Apps` in the Start menu to the added apps of this
/// profile: missing or outdated shortcuts are written, and this profile's
/// shortcuts no added app wants are removed. Other profiles' are left alone.
fn sync_start_menu(inner: &Inner) {
    use newport_platform::app_shortcuts;
    let Some(folder) = start_menu_folder(&inner.launcher) else {
        return;
    };
    let data_directory = inner.launcher.data_directory.as_deref();
    let mut owned = Vec::new();
    let mut taken = Vec::new();
    let mut in_use = HashSet::new();
    for path in app_shortcuts::list(&folder) {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        match app_shortcuts::read(&path).map(link) {
            Ok(link) if shortcuts::owned_by(&link, data_directory) => owned.push((name, link)),
            Ok(link) => {
                in_use.insert(link.app_id);
                taken.push(name);
            }
            // Unreadable: someone else's, kept.
            Err(_) => taken.push(name),
        }
    }
    let added: Vec<Entry> = inner
        .registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .added()
        .cloned()
        .collect();
    let added_refs: Vec<&Entry> = added.iter().collect();
    let folder_length = folder.as_os_str().to_string_lossy().encode_utf16().count();
    let executable = inner.launcher.executable.to_string_lossy().into_owned();
    let wanted: Vec<(String, Link)> = shortcuts::file_names(&added_refs, &taken, folder_length)
        .into_iter()
        .filter_map(|(id, name)| {
            let entry = added.iter().find(|entry| entry.id == id)?;
            let icon = inner
                .store
                .icon_path(&id)
                .ok()
                .filter(|icon| icon.is_file())
                .map_or((String::new(), 0), |icon| {
                    (icon.to_string_lossy().into_owned(), 0)
                });
            let description = if entry.server_name.is_empty() {
                entry.name.clone()
            } else {
                format!("{} on {}", entry.name, entry.server_name)
            };
            Some((
                name,
                Link {
                    target: executable.clone(),
                    arguments: identity::relaunch_arguments(data_directory, &id, None),
                    icon,
                    description,
                    app_id: id,
                },
            ))
        })
        .collect();
    in_use.extend(wanted.iter().map(|(_, link)| link.app_id.clone()));
    let plan = shortcuts::plan(&owned, &wanted);
    for name in &plan.remove {
        if let Err(error) = app_shortcuts::remove(&folder.join(name)) {
            log::warn!("{error}");
        }
    }
    for (name, link) in &plan.write {
        if let Err(error) = app_shortcuts::write(&folder.join(name), &shortcut(link)) {
            log::warn!("{error}");
        }
    }
    // An empty folder leaves the Start menu; never anything in it.
    if app_shortcuts::list(&folder).is_empty() {
        let _ = fs::remove_dir(&folder);
    }
    inner
        .in_use
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .start = in_use;
}

/// What the shell keeps by an added app's ID, outside its shortcut.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Integration {
    /// The jump list's tasks: the app's actions.
    tasks: Vec<newport_platform::app_shortcuts::Task>,
    /// The name and image its notifications show.
    name: String,
    image: Option<PathBuf>,
}

/// The jump lists and registrations this process wrote, by ID: the worker
/// compares against them, so each is written once and removed with its app.
#[derive(Default)]
struct Shell {
    written: HashMap<String, Integration>,
}

impl Shell {
    /// Brings the jump lists and notification registrations to the added
    /// apps: what an app wants and lacks is written, and what a removed app
    /// had goes. A failed write is tried again at the next change.
    fn sync(&mut self, inner: &Inner) {
        use newport_platform::app_shortcuts;
        let launcher = &inner.launcher;
        if !launcher.jump_lists && launcher.app_ids_key.is_none() {
            return;
        }
        let added: Vec<Entry> = inner
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .added()
            .cloned()
            .collect();
        let named: Vec<_> = added
            .iter()
            .map(|entry| identity::NamedApp {
                server_id: &entry.server,
                server_name: &entry.server_name,
                name: &entry.name,
            })
            .collect();
        let file = |path: Result<PathBuf, String>| path.ok().filter(|path| path.is_file());
        let wanted: HashMap<String, Integration> = added
            .iter()
            .zip(&named)
            .map(|(entry, app)| {
                let icon = file(inner.store.icon_path(&entry.id));
                let tasks = shortcuts::jump_list(entry, launcher.data_directory.as_deref())
                    .into_iter()
                    .map(|task| app_shortcuts::Task {
                        title: task.title,
                        target: launcher.executable.clone(),
                        arguments: task.arguments,
                        icon: icon.clone().map(|icon| (icon, 0)),
                    })
                    .collect();
                (
                    entry.id.clone(),
                    Integration {
                        tasks,
                        name: identity::distinct_name(*app, &named),
                        image: file(inner.store.image_path(&entry.id)),
                    },
                )
            })
            .collect();
        let gone: Vec<String> = self
            .written
            .keys()
            .filter(|id| !wanted.contains_key(*id))
            .cloned()
            .collect();
        for id in gone {
            let written = self.written.remove(&id).expect("Listed above");
            if launcher.jump_lists
                && !written.tasks.is_empty()
                && let Err(error) = app_shortcuts::remove_jump_list(&id)
            {
                log::warn!("{error}");
            }
            if let Some(key) = &launcher.app_ids_key
                && let Err(error) = app_shortcuts::unregister_app_id(key, &id)
            {
                log::warn!("{error}");
            }
        }
        for (id, want) in wanted {
            let known = self.written.get(&id);
            let mut done = true;
            // Unknown at startup: a list written by an earlier run may be
            // out of date, or belong to actions the app no longer has.
            if launcher.jump_lists
                && known.is_none_or(|known| known.tasks != want.tasks)
                && let Err(error) = app_shortcuts::write_jump_list(&id, &want.tasks)
            {
                log::warn!("{error}");
                done = false;
            }
            if let Some(key) = &launcher.app_ids_key
                && known
                    .is_none_or(|known| (&known.name, &known.image) != (&want.name, &want.image))
                && let Err(error) =
                    app_shortcuts::register_app_id(key, &id, &want.name, want.image.as_deref())
            {
                log::warn!("{error}");
                done = false;
            }
            if done {
                self.written.insert(id, want);
            } else {
                self.written.remove(&id);
            }
        }
    }
}

/// Removes every remote app's Start shortcut and notification registration,
/// whichever profile wrote it, and this profile's app icons and jump lists,
/// for the uninstaller. The registry stays, so a later install puts the
/// added apps back. Returns how many shortcuts and icons went.
pub fn remove_shortcuts(directory: &Path, launcher: &Launcher) -> (usize, usize) {
    use newport_platform::app_shortcuts;
    if launcher.jump_lists {
        for entry in Store::new(directory).load().entries {
            if let Err(error) = app_shortcuts::remove_jump_list(&entry.id) {
                log::warn!("{error}");
            }
        }
    }
    if let Some(key) = &launcher.app_ids_key {
        for id in app_shortcuts::registered_app_ids(key) {
            if identity::is_remote_app_id(&id)
                && let Err(error) = app_shortcuts::unregister_app_id(key, &id)
            {
                log::warn!("{error}");
            }
        }
    }
    let mut shortcuts_removed = 0;
    if let Some(folder) = start_menu_folder(launcher) {
        for path in app_shortcuts::list(&folder) {
            let ours =
                app_shortcuts::read_app_id(&path).is_ok_and(|id| identity::is_remote_app_id(&id));
            if ours {
                match app_shortcuts::remove(&path) {
                    Ok(()) => shortcuts_removed += 1,
                    Err(error) => log::warn!("{error}"),
                }
            }
        }
        let _ = fs::remove_dir(&folder);
    }
    let icons = directory.join(ICONS);
    let mut icons_removed = 0;
    for entry in fs::read_dir(&icons).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(extension) = path.extension() else {
            continue;
        };
        if extension == "ico" || extension == "png" {
            match fs::remove_file(&path) {
                // Each app has one icon, in two formats.
                Ok(()) if extension == "ico" => icons_removed += 1,
                Ok(()) => {}
                Err(error) => log::warn!("Cannot remove {}: {error}", path.display()),
            }
        }
    }
    let _ = fs::remove_dir(&icons);
    (shortcuts_removed, icons_removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Arc<[u8]> {
        coloured(120)
    }

    fn coloured(green: u8) -> Arc<[u8]> {
        let image = image::RgbaImage::from_pixel(48, 48, image::Rgba([0, green, 200, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png.into()
    }

    const ID: &str = "Newport.Remote.6fe02d2fc8ee3025544aa1e7.db207836d948804bf9711c8c";

    /// The Start menu and the pins live in the test's own folder; nothing
    /// is registered and no jump list written.
    fn launcher(directory: &Path) -> Launcher {
        Launcher {
            executable: PathBuf::from(r"C:\Newport\newport.exe"),
            data_directory: None,
            programs_folder: Some(directory.join("Programs")),
            pinned_folder: Some(directory.join("Pinned")),
            app_ids_key: None,
            jump_lists: false,
        }
    }

    fn recorded(id: &str, server: &str, app: App, name: &str) -> RecordedApp {
        RecordedApp {
            id: id.into(),
            server: server.into(),
            server_name: "lab".into(),
            app,
            name: name.into(),
            actions: Vec::new(),
        }
    }

    fn open(directory: &Path) -> RemoteApps {
        RemoteApps::open(directory, launcher(directory))
    }

    /// The added apps' shortcuts: only Windows writes them.
    fn start_menu(directory: &Path) -> Vec<String> {
        newport_platform::app_shortcuts::list(&directory.join("Programs").join(shortcuts::FOLDER))
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    fn on_windows(names: &[&str]) -> Vec<String> {
        if cfg!(windows) {
            names.iter().map(|name| (*name).to_owned()).collect()
        } else {
            Vec::new()
        }
    }

    #[test]
    fn a_saved_registry_loads_back_and_leaves_no_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path());
        assert_eq!(store.load(), Registry::default());
        assert!(
            !directory.path().join(FILE).exists(),
            "reading writes nothing"
        );
        let mut registry = Registry::default();
        registry.record(Entry {
            id: ID.into(),
            server: "6f1c1f4e-1f43-4b8e-9d55-7f6a63a1b2c4".into(),
            server_name: "lab".into(),
            app: App::DesktopId("org.gnome.Calculator.desktop".into()),
            name: "Calculator".into(),
            added: false,
            last_used: now(),
            actions: Vec::new(),
        });
        store.save(&registry).unwrap();
        assert_eq!(store.load(), registry);
        let names: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE]);
    }

    #[test]
    fn a_damaged_registry_is_kept_aside_and_replaced_by_an_empty_one() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path());
        for damaged in [&b"{\"version\":1,"[..], b"{\"version\":9,\"entries\":[]}"] {
            fs::write(directory.path().join(FILE), damaged).unwrap();
            assert_eq!(store.load(), Registry::default());
            assert_eq!(fs::read(directory.path().join(DAMAGED)).unwrap(), damaged);
            assert_eq!(
                Registry::from_json(&fs::read(directory.path().join(FILE)).unwrap()).unwrap(),
                Registry::default()
            );
        }
    }

    #[test]
    fn icons_are_written_as_ico_files_named_by_id() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path());
        let path = store.write_icon(ID, &png()).unwrap();
        assert_eq!(path, directory.path().join(ICONS).join(format!("{ID}.ico")));
        assert_eq!(&fs::read(&path).unwrap()[..4], &[0, 0, 1, 0]);
        for id in ["", "..", "../x", r"a\b", "a b", ".hidden"] {
            assert!(store.write_icon(id, &png()).is_err(), "{id}");
        }
        store.remove_icon(ID);
        assert!(!path.exists());
        store.remove_icon(ID);
    }

    #[test]
    fn recorded_apps_and_their_icons_are_written_off_the_callers_thread() {
        let directory = tempfile::tempdir().unwrap();
        let apps = open(directory.path());
        apps.record(recorded(
            ID,
            "s1",
            App::DesktopId("calc.desktop".into()),
            "Calculator",
        ));
        let image = png();
        assert_eq!(apps.icon_file(ID, &image), None);
        apps.settle();
        let path = apps.icon_file(ID, &image).unwrap();
        assert!(path.is_file());
        assert_eq!(apps.find(ID).unwrap().name, "Calculator");
        assert_eq!(apps.names(), [("s1".to_owned(), "Calculator".to_owned())]);
        // Another process start reads what was written.
        let again = open(directory.path());
        let entry = again.find(ID).unwrap();
        assert!(!entry.added);
        assert_eq!(entry.app, App::DesktopId("calc.desktop".into()));
        // A changed image is written again.
        let other: Arc<[u8]> = Arc::from(&*png());
        assert_eq!(apps.icon_file(ID, &other), Some(path.clone()));
        let changed = coloured(30);
        assert_eq!(apps.icon_file(ID, &changed), None);
        apps.settle();
        assert_eq!(apps.icon_file(ID, &changed), Some(path));
    }

    #[test]
    fn unused_apps_and_their_icons_go_when_the_registry_opens() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path());
        let icon = store.write_icon(ID, &png()).unwrap();
        let mut registry = Registry::default();
        registry.record(Entry {
            id: ID.into(),
            server: "s1".into(),
            server_name: "lab".into(),
            app: App::Server,
            name: "lab apps".into(),
            added: false,
            last_used: now() - registry::MAX_IDLE_SECONDS - 60,
            actions: Vec::new(),
        });
        store.save(&registry).unwrap();
        let apps = open(directory.path());
        apps.settle();
        assert!(apps.find(ID).is_none());
        assert!(!icon.exists());
        assert_eq!(store.load(), Registry::default());
    }

    fn calculator(id: &str, server: &str) -> StartMenuApp {
        StartMenuApp {
            id: id.into(),
            server: server.into(),
            server_name: "lab".into(),
            app: App::DesktopId("org.gnome.Calculator.desktop".into()),
            name: "Calculator".into(),
            icon: Some(png()),
            actions: Vec::new(),
        }
    }

    #[test]
    fn an_added_app_is_kept_with_its_icon_until_taken_off() {
        let directory = tempfile::tempdir().unwrap();
        let apps = open(directory.path());
        assert!(!apps.is_added(ID));
        apps.set_added(calculator(ID, "s1"), true).unwrap();
        apps.settle();
        assert!(apps.is_added(ID));
        assert_eq!(
            start_menu(directory.path()),
            on_windows(&["Calculator.lnk"])
        );
        let icon = Store::new(directory.path()).icon_path(ID).unwrap();
        assert!(icon.is_file());
        let saved = Store::new(directory.path()).load();
        assert!(saved.find(ID).unwrap().added);
        assert_eq!(saved.find(ID).unwrap().server_name, "lab");
        // Adding again changes nothing; taking it off keeps a temporary entry.
        apps.set_added(calculator(ID, "s1"), true).unwrap();
        apps.set_added(calculator(ID, "s1"), false).unwrap();
        apps.settle();
        assert!(!apps.is_added(ID));
        assert!(apps.find(ID).is_some());
        assert!(icon.is_file(), "A pin may still show it");
        assert!(start_menu(directory.path()).is_empty());
        assert!(
            !directory
                .path()
                .join("Programs")
                .join(shortcuts::FOLDER)
                .exists()
        );
        assert!(!Store::new(directory.path()).load().find(ID).unwrap().added);
    }

    #[test]
    fn at_most_thirty_two_apps_are_added() {
        let directory = tempfile::tempdir().unwrap();
        let apps = open(directory.path());
        for n in 0..registry::MAX_ADDED {
            let mut app = calculator(&format!("Newport.Remote.a.{n:02}"), "s1");
            app.icon = None;
            apps.set_added(app, true).unwrap();
        }
        let one_more = calculator(ID, "s1");
        assert!(
            apps.set_added(one_more.clone(), true)
                .unwrap_err()
                .contains("32")
        );
        assert!(apps.find(ID).is_none());
        // One already added may be added again.
        let mut again = calculator("Newport.Remote.a.00", "s1");
        again.icon = None;
        apps.set_added(again, true).unwrap();
    }

    #[test]
    fn a_removed_server_takes_its_apps_and_icons() {
        let directory = tempfile::tempdir().unwrap();
        let apps = open(directory.path());
        let other = "Newport.Remote.6fe02d2fc8ee3025544aa1e7.000000000000000000000000";
        apps.set_added(calculator(ID, "s1"), true).unwrap();
        apps.record(recorded(other, "s1", App::Server, "lab apps"));
        let kept = "Newport.Remote.0c8a3e572a0d4d6ba7a53b4f.db207836d948804bf9711c8c";
        apps.set_added(calculator(kept, "s2"), true).unwrap();
        apps.settle();
        let store = Store::new(directory.path());
        assert!(store.icon_path(ID).unwrap().is_file());
        assert_eq!(
            start_menu(directory.path()),
            on_windows(&["Calculator (lab) (2).lnk", "Calculator (lab).lnk"])
        );
        apps.forget_server("s1");
        apps.settle();
        assert_eq!(
            start_menu(directory.path()),
            on_windows(&["Calculator.lnk"])
        );
        assert!(apps.find(ID).is_none());
        assert!(apps.find(other).is_none());
        assert!(!store.icon_path(ID).unwrap().exists());
        assert!(apps.is_added(kept));
        assert!(store.icon_path(kept).unwrap().is_file());
        let saved = store.load();
        assert_eq!(saved.entries.len(), 1);
        assert_eq!(saved.entries[0].id, kept);
        // A server with no apps changes nothing.
        apps.forget_server("s3");
    }

    #[test]
    fn a_renamed_server_renames_its_apps_start_shortcuts_at_once() {
        let directory = tempfile::tempdir().unwrap();
        let apps = open(directory.path());
        let other = "Newport.Remote.0c8a3e572a0d4d6ba7a53b4f.db207836d948804bf9711c8c";
        apps.set_added(calculator(ID, "s1"), true).unwrap();
        apps.set_added(calculator(other, "s2"), true).unwrap();
        apps.settle();
        assert_eq!(
            start_menu(directory.path()),
            on_windows(&["Calculator (lab) (2).lnk", "Calculator (lab).lnk"])
        );
        apps.rename_server("s1", "build box");
        apps.settle();
        assert_eq!(
            start_menu(directory.path()),
            on_windows(&["Calculator (build box).lnk", "Calculator (lab).lnk"])
        );
        assert_eq!(apps.find(ID).unwrap().server_name, "build box");
        assert_eq!(apps.find(other).unwrap().server_name, "lab");
        assert_eq!(
            Store::new(directory.path())
                .load()
                .find(ID)
                .unwrap()
                .server_name,
            "build box"
        );
    }

    #[test]
    fn the_uninstaller_removes_icons_and_keeps_the_registry() {
        let directory = tempfile::tempdir().unwrap();
        let apps = open(directory.path());
        apps.set_added(calculator(ID, "s1"), true).unwrap();
        apps.settle();
        drop(apps);
        let launcher = launcher(directory.path());
        let removed = usize::from(cfg!(windows));
        assert_eq!(remove_shortcuts(directory.path(), &launcher), (removed, 1));
        assert!(!directory.path().join(ICONS).exists());
        assert!(start_menu(directory.path()).is_empty());
        assert!(Store::new(directory.path()).load().find(ID).unwrap().added);
        assert_eq!(remove_shortcuts(directory.path(), &launcher), (0, 0));
        // The next start puts the added app back.
        let apps = open(directory.path());
        apps.settle();
        assert_eq!(
            start_menu(directory.path()),
            on_windows(&["Calculator.lnk"])
        );
    }

    /// Startup repair, which only Windows has shortcuts for.
    #[cfg(windows)]
    #[test]
    fn startup_repairs_newports_shortcuts_and_keeps_what_pins_use() {
        use newport_platform::app_shortcuts::{self, Shortcut};
        let directory = tempfile::tempdir().unwrap();
        let launcher = launcher(directory.path());
        let programs = launcher.programs_folder.clone().unwrap();
        let pins = launcher.pinned_folder.clone().unwrap().join("TaskBar");
        let installed = programs.join(NEWPORT_SHORTCUT);
        let newport = Shortcut {
            target: launcher.executable.clone(),
            ..Shortcut::default()
        };
        app_shortcuts::write(&installed, &newport).unwrap();
        // Another program's pin, and a pin of an app unused for a year.
        let other = pins.join("Other.lnk");
        app_shortcuts::write(
            &other,
            &Shortcut {
                target: r"C:\Other\other.exe".into(),
                ..Shortcut::default()
            },
        )
        .unwrap();
        // A pinned window's shortcut, which the shell made.
        let pinned = launcher
            .pinned_folder
            .clone()
            .unwrap()
            .join(r"ImplicitAppShortcuts\0123456789abcdef\Calculator.lnk");
        app_shortcuts::write(
            &pinned,
            &Shortcut {
                arguments: format!("--remote-app {ID}"),
                app_id: ID.into(),
                ..newport.clone()
            },
        )
        .unwrap();
        // A pin of this profile's app from before Newport moved, its icon
        // gone, and one of an app this profile doesn't know.
        let stale = pins.join("Calculator.lnk");
        let outdated = Shortcut {
            target: r"C:\Old\newport.exe".into(),
            arguments: format!(r"--data-dir C:\old --remote-app {ID}"),
            icon: Some((directory.path().join("gone.ico"), 0)),
            description: "Calculator on lab".into(),
            app_id: ID.into(),
        };
        app_shortcuts::write(&stale, &outdated).unwrap();
        let unknown = pins.join("Other app.lnk");
        let foreign_pin = Shortcut {
            arguments: "--remote-app Newport.Remote.c.d".into(),
            app_id: "Newport.Remote.c.d".into(),
            ..outdated.clone()
        };
        app_shortcuts::write(&unknown, &foreign_pin).unwrap();
        // A stray shortcut of this profile, and another profile's.
        let folder = programs.join(shortcuts::FOLDER);
        let stray = folder.join("Gone.lnk");
        app_shortcuts::write(&stray, &app_shortcuts::read(&pinned).unwrap()).unwrap();
        let foreign = folder.join("Calculator.lnk");
        app_shortcuts::write(
            &foreign,
            &Shortcut {
                arguments: format!(r"--data-dir C:\p --remote-app {ID}"),
                ..app_shortcuts::read(&pinned).unwrap()
            },
        )
        .unwrap();
        let store = Store::new(directory.path());
        let mut registry = Registry::default();
        let old = now() - 365 * 24 * 60 * 60;
        for (id, added) in [(ID, false), ("Newport.Remote.a.b", true)] {
            registry.record(Entry {
                id: id.into(),
                server: "s1".into(),
                server_name: "lab".into(),
                app: App::DesktopId("org.gnome.Calculator.desktop".into()),
                name: "Calculator".into(),
                added: false,
                last_used: old,
                actions: Vec::new(),
            });
            registry.entries.last_mut().unwrap().added = added;
        }
        store.save(&registry).unwrap();
        let apps = RemoteApps::open(directory.path(), launcher.clone());
        apps.settle();
        assert_eq!(
            app_shortcuts::read_app_id(&installed).unwrap(),
            "Newport.App"
        );
        assert_eq!(app_shortcuts::read_app_id(&other).unwrap(), "");
        assert_eq!(app_shortcuts::read_app_id(&pinned).unwrap(), ID);
        assert!(apps.find(ID).is_some(), "Its pin keeps it");
        assert_eq!(
            app_shortcuts::read(&stale).unwrap(),
            Shortcut {
                target: launcher.executable.clone(),
                arguments: format!("--remote-app {ID}"),
                icon: None,
                ..outdated
            }
        );
        assert_eq!(app_shortcuts::read(&unknown).unwrap(), foreign_pin);
        assert!(!stray.exists());
        assert!(foreign.is_file());
        // The added app's name is taken by the other profile's.
        let added = folder.join("Calculator (2).lnk");
        let link = app_shortcuts::read(&added).unwrap();
        assert_eq!(link.app_id, "Newport.Remote.a.b");
        assert_eq!(link.arguments, "--remote-app Newport.Remote.a.b");
        assert_eq!(link.description, "Calculator on lab");
        assert_eq!(link.target, launcher.executable);
        // Run again: nothing changes.
        let modified = |path: &Path| std::fs::metadata(path).unwrap().modified().unwrap();
        let written = [modified(&added), modified(&stale)];
        drop(apps);
        let apps = RemoteApps::open(directory.path(), launcher);
        apps.settle();
        assert_eq!([modified(&added), modified(&stale)], written);
    }

    /// Notification names, in a registry key of the test's own; jump lists
    /// stay off, since the shell keeps them outside any folder a test owns.
    #[cfg(windows)]
    #[test]
    fn added_apps_register_their_notification_name_until_removed() {
        use newport_platform::app_shortcuts;
        let directory = tempfile::tempdir().unwrap();
        let key = format!(r"Software\NewportTests\{}", uuid::Uuid::new_v4());
        let launcher = Launcher {
            app_ids_key: Some(key.clone()),
            ..launcher(directory.path())
        };
        let apps = RemoteApps::open(directory.path(), launcher.clone());
        apps.set_added(calculator(ID, "s1"), true).unwrap();
        apps.settle();
        let registered = || app_shortcuts::registered_app_ids(&key);
        assert_eq!(registered(), [ID]);
        let image = Store::new(directory.path()).image_path(ID).unwrap();
        assert!(image.is_file());
        apps.set_added(calculator(ID, "s1"), false).unwrap();
        apps.settle();
        assert!(registered().is_empty());
        apps.set_added(calculator(ID, "s1"), true).unwrap();
        apps.settle();
        assert_eq!(registered(), [ID]);
        drop(apps);
        remove_shortcuts(directory.path(), &launcher);
        assert!(registered().is_empty());
        assert!(!image.exists());
        // The test's key itself, which only the test made.
        app_shortcuts::unregister_app_id(
            r"Software\NewportTests",
            key.rsplit('\\').next().unwrap(),
        )
        .unwrap();
    }
}
