//! Explicitly opted-in, read-only remote advertisement check through desktop IPC.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use newport_client::{
    backend::Backend,
    git_remote_refs::{Listing, PAGE_SIZE},
};
use newport_protocol::git as protocol;
use serde_json::Value;

/// Sends a Git read and returns its checked reply.
fn reply(
    backend: &Backend,
    server: &str,
    request: protocol::Request,
) -> Result<protocol::Reply, String> {
    backend
        .git(
            server
                .parse::<uuid::Uuid>()
                .map_err(|error| error.to_string())?,
            request,
            false,
        )
        .map_err(|error| error.to_string())
}
/// [`reply`] as JSON, for assertions on its fields.
fn request(backend: &Backend, server: &str, request: protocol::Request) -> Result<Value, String> {
    serde_json::to_value(reply(backend, server, request)?).map_err(|error| error.to_string())
}

struct Opened<'a> {
    backend: &'a Backend,
    server: &'a str,
    repo: Value,
}
impl Drop for Opened<'_> {
    fn drop(&mut self) {
        if let Some(repo_id) = self.repo["repoId"].as_str() {
            self.backend.release_repository(self.server, repo_id);
        }
    }
}

#[test]
#[ignore = "requires running backend and explicit read-only SSH target environment"]
fn configured_remote_advertisement_accepts_bounded_pages() -> Result<(), String> {
    let env = |name: &str| std::env::var(name).map_err(|_| format!("Set {name} explicitly"));
    let host = env("NEWPORT_TEST_READONLY_HOST")?;
    let user = env("NEWPORT_TEST_READONLY_USER")?;
    let path = env("NEWPORT_TEST_READONLY_PATH")?;
    let remote_name = env("NEWPORT_TEST_READONLY_REMOTE")?;
    if !path.starts_with('/') || host.is_empty() || user.is_empty() || remote_name.is_empty() {
        return Err("Specify an absolute repository path and an exact SSH host/user/remote".into());
    }
    let backend = Backend::discover();
    let snapshot = backend
        .configuration_snapshot()
        .map_err(|error| error.to_string())?;
    let servers: Vec<_> = snapshot
        .config
        .servers
        .iter()
        .filter(|s| s.ssh_host == host && s.ssh_user == user)
        .collect();
    if servers.len() != 1 {
        return Err("Expected exactly one matching configured server".into());
    }
    let server = servers[0].id.to_string();
    let server = server.as_str();
    let opened = Opened {
        backend: &backend,
        server,
        repo: request(
            &backend,
            server,
            protocol::Request::Open {
                path: protocol::Path::new(path.as_bytes()),
            },
        )?,
    };
    if opened.repo["root"]["bytesB64"] != STANDARD.encode(&path) {
        return Err("The opened repository differs from the requested path".into());
    }
    let repo = opened.repo["repoId"]
        .as_str()
        .ok_or("Missing repository ID")?;
    let protocol::Reply::Remote(remote) = reply(
        &backend,
        server,
        protocol::Request::Remote {
            repo_id: repo.into(),
            name: remote_name.clone(),
        },
    )?
    else {
        return Err("Invalid remote".into());
    };
    let mut listing = Listing::default();
    let mut pages = 0;
    // Deliberately stop after three pages rather than draining a large remote.
    for _ in 0..3 {
        let cursor = listing.page().and_then(|page| page.next_cursor.clone());
        let protocol::Reply::RemoteRefs(response) = reply(
            &backend,
            server,
            protocol::Request::RemoteRefs {
                repo_id: repo.into(),
                remote: remote_name.clone(),
                expected_token: remote.token.clone(),
                for_push: true,
                filter: String::new(),
                page_size: PAGE_SIZE,
                cursor: cursor.clone(),
            },
        )?
        else {
            return Err("Invalid remote references".into());
        };
        listing.accept(*response, &remote, cursor.as_deref())?;
        pages += 1;
        if listing.page().is_none_or(|page| page.next_cursor.is_none()) {
            break;
        }
    }
    // Counts only: no repository contents, URLs or credentials in the test log.
    eprintln!(
        "Validated {pages} remote pages, {} references; more={}",
        listing.page().ok_or("Missing rows")?.entries.len(),
        listing
            .page()
            .is_some_and(|page| page.next_cursor.is_some())
    );
    Ok(())
}
