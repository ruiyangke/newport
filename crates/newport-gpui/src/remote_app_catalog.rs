//! The apps the Remote apps page last listed, by server and desktop id, so a
//! remote window presented as its own app gets the app's name and icon.
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub icon_png: Option<Arc<[u8]>>,
    /// Its desktop actions, such as "New Window", from the desktop file's
    /// `Actions=`.
    pub actions: Vec<Action>,
}

pub use newport_domain::remote_app_registry::Action;

pub(crate) fn actions(actions: &[crate::remote_apps_api::AppAction]) -> Vec<Action> {
    actions
        .iter()
        .map(|action| Action {
            id: action.id.clone(),
            name: action.name.clone(),
        })
        .collect()
}

/// More than any set of servers lists; past it the cache starts over.
const MAX_ENTRIES: usize = 4096;

type Entries = HashMap<(String, String), Entry>;

static ENTRIES: Mutex<Option<Entries>> = Mutex::new(None);

/// Called with each catalogue read: the server's id and name, and every app
/// it lists, by catalogue id.
pub type Observer = Box<dyn Fn(&str, &str, Vec<(String, Entry)>) + Send + Sync>;

static OBSERVER: std::sync::OnceLock<Observer> = std::sync::OnceLock::new();

/// Have `observer` called after each catalogue read, as the Mac's app
/// launchers follow the apps a server lists. Set once.
pub fn observe(observer: Observer) {
    let _ = OBSERVER.set(observer);
}

pub(crate) fn remember(server: &str, name: &str, apps: &[crate::remote_apps_api::RemoteApp]) {
    record(server, apps);
    // A read that listed nothing is never taken to mean the apps are gone:
    // it may be one that failed or hasn't finished.
    if !apps.is_empty()
        && !name.is_empty()
        && let Some(observer) = OBSERVER.get()
    {
        let listed = apps
            .iter()
            .take(MAX_ENTRIES)
            .map(|app| {
                (
                    app.id.clone(),
                    Entry {
                        name: app.name.clone(),
                        icon_png: app.icon_png.clone(),
                        actions: actions(&app.actions),
                    },
                )
            })
            .collect();
        observer(server, name, listed);
    }
}

fn record(server: &str, apps: &[crate::remote_apps_api::RemoteApp]) {
    let mut entries = ENTRIES.lock().unwrap();
    let entries = entries.get_or_insert_with(HashMap::new);
    entries.retain(|(known, _), _| known != server);
    if entries.len() + apps.len() > MAX_ENTRIES {
        entries.clear();
    }
    for app in apps.iter().take(MAX_ENTRIES) {
        entries.insert(
            (server.to_owned(), app.id.clone()),
            Entry {
                name: app.name.clone(),
                icon_png: app.icon_png.clone(),
                actions: actions(&app.actions),
            },
        );
    }
}

/// How long a server's apps wait for its catalogue before they open with
/// their desktop ids and letter tiles.
const WAIT: Duration = Duration::from_secs(3);

/// Called once the catalogue being read has arrived, or after [`WAIT`].
type Waiter = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Reads {
    /// Servers whose catalogue is being read, since when, and who waits.
    pending: HashMap<String, (Instant, Vec<Waiter>)>,
    /// Servers whose catalogue was read once, whether or not it listed apps.
    settled: HashSet<String>,
}

static READS: Mutex<Option<Reads>> = Mutex::new(None);

fn known(server: &str) -> bool {
    ENTRIES
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|entries| entries.keys().any(|(known, _)| known == server))
}

/// Read the server's catalogue in the background unless it is known, for
/// apps that open before the Remote apps page was shown.
pub fn load(server: &str, name: &str) {
    let _ = ready(server, name, || {});
}

/// Whether apps of `server` can be named now. If not, the catalogue is being
/// read, and `then` runs once it arrives or [`WAIT`] has passed.
pub fn ready(server: &str, name: &str, then: impl FnOnce() + Send + 'static) -> bool {
    let waits = {
        let mut reads = READS.lock().unwrap();
        let reads = reads.get_or_insert_with(Reads::default);
        let known = known(server);
        // Only a server never read holds its apps back; one read before is
        // read again while its apps open.
        let waits = !known && !reads.settled.contains(server);
        if let Some((since, waiters)) = reads.pending.get_mut(server) {
            if !waits || since.elapsed() >= WAIT {
                return true;
            }
            waiters.push(Box::new(then));
            return false;
        }
        if known {
            return true;
        }
        let waiters: Vec<Waiter> = if waits {
            vec![Box::new(then)]
        } else {
            Vec::new()
        };
        reads
            .pending
            .insert(server.to_owned(), (Instant::now(), waiters));
        waits
    };
    read(server.to_owned(), name.to_owned());
    if waits {
        let server = server.to_owned();
        std::thread::spawn(move || {
            std::thread::sleep(WAIT);
            wake(&server, false);
        });
    }
    !waits
}

/// Run the waiters of `server`, and when its read has ended, forget it.
fn wake(server: &str, ended: bool) {
    let waiters = {
        let mut reads = READS.lock().unwrap();
        let reads = reads.get_or_insert_with(Reads::default);
        if ended {
            reads.settled.insert(server.to_owned());
            reads
                .pending
                .remove(server)
                .map(|(_, waiters)| waiters)
                .unwrap_or_default()
        } else {
            reads
                .pending
                .get_mut(server)
                .map(|(_, waiters)| std::mem::take(waiters))
                .unwrap_or_default()
        }
    };
    for waiter in waiters {
        waiter();
    }
}

fn read(server: String, name: String) {
    std::thread::spawn(move || {
        let read = uuid::Uuid::parse_str(&server)
            .map_err(|error| error.to_string())
            .and_then(|server| {
                newport_client::backend::Backend::discover()
                    .execute_with_timeout(
                        newport_protocol::desktop::Operation::Applications(Box::new(
                            newport_protocol::desktop::applications::Operation::Catalogue(
                                Box::new(newport_protocol::desktop::applications::CatalogueArgs {
                                    server,
                                    refresh: false,
                                }),
                            ),
                        )),
                        Duration::from_secs(30),
                    )
                    .map_err(|error| error.to_string())
                    .and_then(|reply| {
                        newport_client::desktop_result::project(reply)
                            .map_err(|error| error.to_string())
                    })
            });
        match read {
            Ok(value) => remember(
                &server,
                &name,
                &crate::remote_apps::decode_catalogue(&value),
            ),
            Err(error) => eprintln!("Remote app catalogue for {server}: {error}"),
        }
        wake(&server, true);
    });
}

/// Catalogue ids carry the `.desktop` suffix; window identities may not.
pub fn find(server: &str, desktop_id: &str) -> Option<Entry> {
    let entries = ENTRIES.lock().unwrap();
    let entries = entries.as_ref()?;
    let key = |id: String| (server.to_owned(), id);
    entries
        .get(&key(desktop_id.to_owned()))
        .or_else(|| entries.get(&key(format!("{desktop_id}.desktop"))))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_apps_api::RemoteApp;

    fn app(id: &str, name: &str) -> RemoteApp {
        RemoteApp {
            id: id.into(),
            name: name.into(),
            description: None,
            icon_png: None,
            actions: Vec::new(),
        }
    }

    #[test]
    fn an_app_keeps_its_desktop_actions() {
        let chromium = RemoteApp {
            actions: vec![crate::remote_apps_api::AppAction {
                id: "new-window".into(),
                name: "New Window".into(),
            }],
            ..app("chromium.desktop", "Chromium")
        };
        remember("catalog-test-actions", "Lab", &[chromium]);
        assert_eq!(
            find("catalog-test-actions", "chromium").unwrap().actions,
            [Action {
                id: "new-window".into(),
                name: "New Window".into(),
            }]
        );
    }

    #[test]
    fn servers_keep_their_own_apps() {
        remember(
            "catalog-test-a",
            "Lab",
            &[app("term.desktop", "Terminal A")],
        );
        remember(
            "catalog-test-b",
            "Lab",
            &[app("term.desktop", "Terminal B")],
        );
        assert_eq!(find("catalog-test-a", "term").unwrap().name, "Terminal A");
        assert_eq!(
            find("catalog-test-b", "term.desktop").unwrap().name,
            "Terminal B"
        );
        remember("catalog-test-a", "Lab", &[]);
        assert!(find("catalog-test-a", "term").is_none());
        assert!(find("catalog-test-b", "term").is_some());
    }

    #[test]
    fn an_unread_server_has_no_apps() {
        assert!(find("catalog-test-unread", "term").is_none());
    }

    #[test]
    fn apps_wait_for_a_first_read_only() {
        // Not a server id, so the read ends at once without apps.
        let server = "catalog-test-first-read";
        let (sender, receiver) = std::sync::mpsc::channel();
        assert!(!ready(server, "Lab", move || sender.send(()).unwrap()));
        receiver
            .recv_timeout(WAIT - Duration::from_millis(500))
            .expect("the waiter runs when the read ends, before the wait is up");
        assert!(ready(server, "Lab", || panic!(
            "a server read once never waits"
        )));
    }

    #[test]
    fn a_known_server_never_waits() {
        let server = "catalog-test-known";
        remember(server, "Lab", &[app("term.desktop", "Terminal")]);
        assert!(ready(server, "Lab", || panic!(
            "a known server never waits"
        )));
    }
}
