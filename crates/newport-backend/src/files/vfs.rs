//! Server-incarnation VFS: shared single-flight directory queries plus bounded
//! change observers and bounded clean content snapshots. Unsaved text belongs
//! exclusively to editor buffers; cached revisions never authorize writes.
//! Native notifications are hints; atomic saves still verify the content hash.
use super::{Listing, Operations};
use crate::{manager::Shared, query::Registry, remote::sftp, server_session::ServerSession};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

pub(crate) struct Vfs {
    pub documents: super::vfs_documents::Documents,
    pub listings: Registry<String, Arc<Listing>>,
    remote: std::sync::OnceLock<super::Remote>,
    fallback: tokio::sync::Mutex<Fallback>,
}
struct Snapshot {
    root: String,
    cursor: String,
    entries: BTreeMap<String, Value>,
    directories: BTreeSet<String>,
    touched: Instant,
}
#[derive(Default)]
struct Fallback {
    /// Held while metadata watches exist, so the shared session stays open
    /// between polls.
    session: Option<sftp::Shared>,
    snapshots: HashMap<Uuid, Snapshot>,
    native_retry: Option<Instant>,
}
impl Default for Vfs {
    fn default() -> Self {
        Self {
            listings: Registry::new(64),
            documents: Default::default(),
            remote: Default::default(),
            fallback: Default::default(),
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Request {
    id: Uuid,
    operation: Uuid,
    expected_connection: Vec<Value>,
    root: String,
    watch: Uuid,
    directories: BTreeSet<String>,
    cursor: Option<String>,
}
fn within(root: &str, path: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| root == "/" || rest.starts_with('/'))
}
fn validate(request: &Request) -> Result<(), String> {
    if request.directories.len() > 64
        || request.directories.iter().map(String::len).sum::<usize>() > 48 * 1024
    {
        return Err("Too many watched folders.".into());
    }
    for path in std::iter::once(&request.root).chain(request.directories.iter()) {
        super::path_valid(path).map_err(|e| e.to_string())?;
        if !path.starts_with('/')
            || !within(&request.root, path)
            || path
                .split('/')
                .skip(1)
                .any(|p| matches!(p, "." | "..") || p.is_empty() && path != "/")
        {
            return Err("Invalid project watch path.".into());
        }
    }
    Ok(())
}
impl Vfs {
    /// The server's SFTP, made on first use from the server's link.
    pub(crate) fn remote(&self, owner: &ServerSession) -> super::Remote {
        self.remote
            .get_or_init(|| super::Remote::new(owner.server().clone(), owner.link().clone()))
            .clone()
    }

    async fn poll(&self, owner: &Arc<ServerSession>, request: &Request) -> Result<Value, String> {
        let native = self
            .fallback
            .lock()
            .await
            .native_retry
            .is_none_or(|at| Instant::now() >= at);
        if native {
            let result = crate::monitor::ask(
                owner,
                &json!({"op":"files_watch", "watch":request.watch,
                "root":request.root,"directories":request.directories,"cursor":request.cursor}),
                Duration::from_secs(8),
            )
            .await;
            if let Ok(reply) = result
                && reply["mode"] == "native"
            {
                let paths: Vec<String> = serde_json::from_value(reply["paths"].clone())
                    .map_err(|_| "Invalid watch paths.")?;
                if paths.len() > 2048
                    || paths.iter().any(|p| !within(&request.root, p))
                    || !reply["reset"].is_boolean()
                    || (!request.directories.is_empty() && !reply["cursor"].is_string())
                {
                    return Err("Invalid file watch reply.".into());
                }
                self.invalidate(&request.root, &paths, reply["reset"] == true);
                return Ok(reply);
            }
            // Without a native watch, poll metadata over SFTP. Probe the agent
            // again later without hammering it.
            self.fallback.lock().await.native_retry =
                Some(Instant::now() + Duration::from_secs(300));
        }
        let mut state = self.fallback.lock().await;
        state
            .snapshots
            .retain(|_, s| s.touched.elapsed() < Duration::from_secs(60));
        if request.directories.is_empty() {
            state.snapshots.remove(&request.watch);
            if state.snapshots.is_empty() {
                state.session = None;
            }
            return Ok(json!({"mode":"poll","cursor":null,"paths":[],"reset":false}));
        }
        if state.snapshots.len() >= 8 && !state.snapshots.contains_key(&request.watch) {
            return Err("Too many project watches.".into());
        }
        // After a lost channel, watches start over so clients reload what they show.
        if state.session.as_ref().is_some_and(|s| s.is_closed()) {
            state.session = None;
            state.snapshots.clear();
        }
        if state.session.is_none() {
            let session = self.remote(owner).sftp.shared().await;
            state.session = Some(session.map_err(|e| e.to_string())?);
        }
        if state.snapshots.is_empty() {
            let weak = Arc::downgrade(owner);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    let Some(owner) = weak.upgrade() else {
                        break;
                    };
                    let mut state = owner.files.fallback.lock().await;
                    state.snapshots.retain(|_, s| {
                        !owner.is_revoked() && s.touched.elapsed() < Duration::from_secs(60)
                    });
                    if state.snapshots.is_empty() {
                        state.session = None;
                        break;
                    }
                }
            });
        }
        // Metadata only: no large-document content is transferred on a quiet poll.
        let session = state.session.as_deref().expect("opened above");
        let result = async {
            let root = match super::canonical(session, &request.root).await {
                Ok(root) => root,
                Err(error) if missing(&error) => {
                    return Ok(BTreeMap::from([(request.root.clone(), Value::Null)]));
                }
                Err(error) => return Err(error),
            };
            anyhow::ensure!(root == request.root, "Project root changed.");
            let mut entries = BTreeMap::new();
            for directory in &request.directories {
                let listing = match super::list(session, directory).await {
                    Ok(listing) => listing,
                    Err(e) => {
                        if missing(&e) {
                            entries.insert(directory.clone(), Value::Null);
                            continue;
                        }
                        return Err(e);
                    }
                };
                anyhow::ensure!(
                    listing.path == *directory,
                    "Watched folder became a symbolic link."
                );
                for entry in listing.entries {
                    entries.insert(entry.path.clone(), serde_json::to_value(entry)?);
                    anyhow::ensure!(entries.len() <= 50_000, "Too many watched file entries.");
                }
            }
            Ok::<_, anyhow::Error>(entries)
        }
        .await;
        let entries = match result {
            Ok(entries) => entries,
            Err(e) => {
                state.session = None;
                return Err(e.to_string());
            }
        };
        let previous = state.snapshots.get(&request.watch);
        let reset = previous.is_none_or(|s| {
            s.root != request.root
                || s.directories != request.directories
                || request.cursor.as_deref() != Some(&s.cursor)
        });
        let paths = previous
            .filter(|s| s.root == request.root)
            .map(|s| changed(&s.entries, &entries))
            .unwrap_or_default();
        let cursor = Uuid::new_v4().to_string();
        state.snapshots.insert(
            request.watch,
            Snapshot {
                root: request.root.clone(),
                cursor: cursor.clone(),
                directories: request.directories.clone(),
                entries,
                touched: Instant::now(),
            },
        );
        self.invalidate(&request.root, &paths, reset);
        Ok(json!({"mode":"poll","cursor":cursor,"paths":paths,"reset":reset}))
    }
    fn invalidate(&self, root: &str, paths: &[String], reset: bool) {
        self.documents.invalidate(|scope, path| {
            scope == root
                && (reset
                    || paths
                        .iter()
                        .any(|changed| path == changed || within(changed, path)))
        });
        self.listings.invalidate(|directory| {
            (reset && within(root, directory))
                || paths.iter().any(|path| {
                    path == directory
                        || std::path::Path::new(path).parent().and_then(|p| p.to_str())
                            == Some(directory.as_str())
                })
        });
    }
}
fn missing(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<russh_sftp::client::error::Error>(),
        Some(russh_sftp::client::error::Error::Status(status)) if status.status_code==russh_sftp::protocol::StatusCode::NoSuchFile)
}
fn changed(before: &BTreeMap<String, Value>, after: &BTreeMap<String, Value>) -> Vec<String> {
    before
        .keys()
        .chain(after.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| before.get(*key) != after.get(*key))
        .cloned()
        .collect()
}
pub(crate) async fn dispatch(
    state: &Shared,
    operations: &Operations,
    request: Request,
) -> Result<Value, String> {
    validate(&request)?;
    operations
        .run_with_connection(
            state,
            request.id,
            request.operation,
            Some(&Value::Array(request.expected_connection.clone())),
            async |_, owner| {
                tokio::time::timeout(Duration::from_secs(30), owner.files.poll(&owner, &request))
                    .await
                    .map_err(|_| anyhow::anyhow!("File watch timed out."))?
                    .map_err(anyhow::Error::msg)
            },
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn invalidation_is_shared_but_scoped_to_the_changed_directory() {
        let vfs = Vfs::default();
        let policy = crate::query::Policy {
            stale_after: Duration::from_secs(60),
            retry_after: Duration::from_secs(2),
            timeout: Duration::from_secs(2),
            observation_lease: Duration::from_millis(1),
        };
        let query = vfs
            .listings
            .get_or_insert("/repo/src".into(), policy, || async {
                Ok(Arc::new(Listing {
                    path: "/repo/src".into(),
                    entries: vec![],
                }))
            })
            .unwrap();
        let other = vfs
            .listings
            .get_or_insert("/other".into(), policy, || async {
                Ok(Arc::new(Listing {
                    path: "/other".into(),
                    entries: vec![],
                }))
            })
            .unwrap();
        query.read().await.unwrap();
        other.read().await.unwrap();
        vfs.invalidate("/repo", &["/repo/src/main.rs".into()], false);
        assert!(query.snapshot().stale);
        assert!(!other.snapshot().stale);
        let second = vfs
            .listings
            .get_or_insert("/repo/src".into(), policy, || async {
                panic!("must reuse query")
            })
            .unwrap();
        assert!(Arc::ptr_eq(&query, &second));
        vfs.listings.close();
        assert!(query.read().await.is_err());
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- files::vfs"]
    async fn metadata_poll_fallback_detects_create_replace_delete_and_releases() {
        let server:crate::model::Server=serde_json::from_value(json!({"id":Uuid::new_v4(),"name":"VFS fixture",
            "sshUser":"fixture","sshHost":"127.0.0.1","sshPort":std::env::var("NEWPORT_TEST_SSH_PORT").unwrap().parse::<u16>().unwrap(),
            "identityFile":std::env::var("NEWPORT_TEST_IDENTITY").unwrap(),"agentInstallApproved":false})).unwrap();
        let sessions = crate::server_session::Registry::default();
        let owner = sessions.session(&server, 0);
        owner.files.fallback.lock().await.native_retry =
            Some(Instant::now() + Duration::from_secs(300));
        let mut request: Request = serde_json::from_value(
            json!({"id":server.id,"operation":Uuid::new_v4(),"expectedConnection":[],
            "root":"/","watch":Uuid::new_v4(),"directories":["/"],"cursor":null}),
        )
        .unwrap();
        let first = owner.files.poll(&owner, &request).await.unwrap();
        assert_eq!(first["mode"], "poll");
        assert_eq!(first["reset"], true);
        request.cursor = first["cursor"].as_str().map(str::to_owned);
        let quiet = owner.files.poll(&owner, &request).await.unwrap();
        assert_eq!(quiet["reset"], false);
        assert_eq!(quiet["paths"], json!([]));
        request.cursor = quiet["cursor"].as_str().map(str::to_owned);
        let sftp = owner.files.remote(&owner).sftp.shared().await.unwrap();
        let path = format!("/vfs-{}.txt", Uuid::new_v4());
        use russh_sftp::protocol::{FileAttributes, OpenFlags};
        let file = sftp
            .open(
                &path,
                OpenFlags::CREATE | OpenFlags::WRITE,
                FileAttributes::default(),
            )
            .await
            .unwrap();
        sftp.write(&file.handle, 0, b"hello".to_vec())
            .await
            .unwrap();
        sftp.close(file.handle).await.unwrap();
        let created = owner.files.poll(&owner, &request).await.unwrap();
        assert!(created["paths"].as_array().unwrap().contains(&json!(path)));
        request.cursor = created["cursor"].as_str().map(str::to_owned);
        sftp.remove(&path).await.unwrap();
        let deleted = owner.files.poll(&owner, &request).await.unwrap();
        assert!(deleted["paths"].as_array().unwrap().contains(&json!(path)));
        assert!(
            owner
                .files
                .fallback
                .lock()
                .await
                .session
                .as_ref()
                .unwrap()
                .downgrade()
                .ptr_eq(&sftp.downgrade())
        );
        drop(sftp);
        request.directories.clear();
        owner.files.poll(&owner, &request).await.unwrap();
        let state = owner.files.fallback.lock().await;
        assert!(state.session.is_none());
        assert!(state.snapshots.is_empty());
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- files::vfs"]
    async fn the_shared_sftp_session_reopens_after_its_channel_is_lost() {
        use crate::remote::command;
        let (_directory, state, owner, server) = super::super::integration_tests::app();
        owner.files.fallback.lock().await.native_retry =
            Some(Instant::now() + Duration::from_secs(300));
        let mut request: Request = serde_json::from_value(
            json!({"id":server.id,"operation":Uuid::new_v4(),"expectedConnection":[],
            "root":"/","watch":Uuid::new_v4(),"directories":["/"],"cursor":null}),
        )
        .unwrap();
        let watched = owner.files.poll(&owner, &request).await.unwrap();
        request.cursor = watched["cursor"].as_str().map(str::to_owned);
        let lost = owner.files.remote(&owner).sftp.shared().await.unwrap();
        let link = owner.link();
        let connection = command::stdout(link, "fixture-connection-id", None)
            .await
            .unwrap();
        // The command runs on the connection it drops, so its own result is lost.
        let _ = command::run(link, &format!("drop-connection {connection}"), None).await;
        tokio::time::timeout(Duration::from_secs(10), async {
            while !lost.is_closed() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the session to see its channel lost");
        let lost = lost.downgrade();

        let preview = super::super::files_preview(
            &state,
            &Operations::default(),
            server.id,
            Uuid::new_v4(),
            "/Documents/hello 世界.txt".into(),
            None,
        )
        .await
        .unwrap();
        assert!(preview.content.contains("<script>inert text</script>"));
        // The watch starts over on the new session.
        let resumed = owner.files.poll(&owner, &request).await.unwrap();
        assert_eq!(resumed["reset"], true);
        {
            let fallback = owner.files.fallback.lock().await;
            let session = fallback.session.as_ref().unwrap();
            assert!(!session.is_closed());
            assert!(!session.downgrade().ptr_eq(&lost));
        }
        assert_ne!(
            command::stdout(link, "fixture-connection-id", None)
                .await
                .unwrap(),
            connection
        );
        crate::manager::shutdown(&state).await;
    }
    #[test]
    fn metadata_diff_is_targeted_and_includes_rename_and_deletion() {
        let before = BTreeMap::from([
            ("/repo/a".into(), json!({"size":3})),
            ("/repo/keep".into(), json!(1)),
        ]);
        let after = BTreeMap::from([
            ("/repo/b".into(), json!({"size":3})),
            ("/repo/keep".into(), json!(1)),
        ]);
        assert_eq!(changed(&before, &after), ["/repo/a", "/repo/b"]);
        assert!(changed(&before, &before).is_empty());
        assert!(!within("/repo", "/repository/a"));
    }
    #[test]
    fn watch_paths_cannot_escape_project_or_exceed_budget() {
        let mut r:Request=serde_json::from_value(json!({"id":Uuid::new_v4(),"operation":Uuid::new_v4(),
            "expectedConnection":[],"root":"/repo","watch":Uuid::new_v4(),"directories":["/repo","/repo/src"]})).unwrap();
        assert!(validate(&r).is_ok());
        for bad in ["/repo/../etc", "/repository", "/repo//src", "relative"] {
            r.directories = BTreeSet::from([bad.into()]);
            assert!(validate(&r).is_err());
        }
        r.directories = (0..65).map(|i| format!("/repo/{i}")).collect();
        assert!(validate(&r).is_err());
    }
}
