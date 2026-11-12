//! Explicitly opted-in, read-only remote advertisement check through desktop IPC.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use newport_client::{
    backend::Backend,
    git_remote_refs::{Listing, PAGE_SIZE},
};
use newport_protocol::{common::Present, desktop, git as protocol};
use serde_json::Value;

fn request(backend: &Backend, server: &str, request: protocol::Request) -> Result<Value, String> {
    backend
        .request_git(
            server
                .parse::<uuid::Uuid>()
                .map_err(|error| error.to_string())?,
            request,
            false,
        )
        .map_err(|error| error.to_string())
}

struct Opened<'a> {
    backend: &'a Backend,
    server: &'a str,
    repo: Value,
}
impl Drop for Opened<'_> {
    fn drop(&mut self) {
        self.backend.release_repository(self.server, &self.repo);
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
        .execute_rendered(desktop::Operation::Configuration(Box::new(
            desktop::configuration::Operation::Snapshot(Box::new(Present {})),
        )))
        .map_err(|error| error.to_string())?;
    let servers: Vec<_> = snapshot["config"]["servers"]
        .as_array()
        .ok_or("Missing servers")?
        .iter()
        .filter(|s| s["sshHost"] == host && s["sshUser"] == user)
        .collect();
    if servers.len() != 1 {
        return Err("Expected exactly one matching configured server".into());
    }
    let server = servers[0]["id"].as_str().ok_or("Missing server ID")?;
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
    let remote = request(
        &backend,
        server,
        protocol::Request::Remote {
            repo_id: repo.into(),
            name: remote_name.clone(),
        },
    )?;
    let mut listing = Listing::default();
    let mut pages = 0;
    // Deliberately stop after three pages rather than draining a large remote.
    for _ in 0..3 {
        let cursor = listing.page()["nextCursor"].as_str().map(str::to_owned);
        let response = request(
            &backend,
            server,
            protocol::Request::RemoteRefs {
                repo_id: repo.into(),
                remote: remote_name.clone(),
                expected_token: remote["token"]
                    .as_str()
                    .ok_or("Missing remote token")?
                    .into(),
                for_push: true,
                filter: String::new(),
                page_size: PAGE_SIZE,
                cursor: cursor.clone(),
            },
        )?;
        listing.accept(response, &remote, cursor.as_deref())?;
        pages += 1;
        if listing.page()["nextCursor"].is_null() {
            break;
        }
    }
    // Counts only: no repository contents, URLs or credentials in the test log.
    eprintln!(
        "Validated {pages} remote pages, {} references; more={}",
        listing.page()["entries"]
            .as_array()
            .ok_or("Missing rows")?
            .len(),
        listing.page()["nextCursor"].is_string()
    );
    Ok(())
}
