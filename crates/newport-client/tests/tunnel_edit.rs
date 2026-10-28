//! Opt-in check against an isolated, running desktop backend and loopback fixture.
//! NEWPORT_DATA_DIR=/tmp/... NEWPORT_TEST_SSH_PORT=... cargo test --test tunnel_edit -- --ignored
use newport_client::backend::Backend;
use newport_domain::{forwarding::discovered_tunnel, model::Tunnel};
use serde_json::{Value, json};
use std::path::PathBuf;

fn tunnels(backend: &Backend) -> Result<Vec<Tunnel>, String> {
    let snapshot = backend.call("snapshot", json!({}))?;
    serde_json::from_value(snapshot["config"]["tunnels"].clone()).map_err(|e| e.to_string())
}

fn require(condition: bool, message: &str) -> Result<(), String> {
    condition.then_some(()).ok_or_else(|| message.to_owned())
}

fn expect_failure(result: Result<Value, String>, reason: &str) -> Result<(), String> {
    match result {
        Err(error) if error.contains(reason) => Ok(()),
        outcome => Err(format!("Expected {reason:?} rejection, got {outcome:?}")),
    }
}

#[test]
#[ignore = "requires isolated desktop backend and disposable loopback SSH fixture"]
fn editor_updates_reject_deleted_and_changed_forwards_without_side_effects() -> Result<(), String> {
    let profile = PathBuf::from(std::env::var("NEWPORT_DATA_DIR").map_err(|e| e.to_string())?);
    let temp = std::fs::canonicalize("/tmp").map_err(|e| e.to_string())?;
    require(
        std::fs::canonicalize(profile)
            .map_err(|e| e.to_string())?
            .starts_with(temp),
        "Test requires an isolated profile under /tmp",
    )?;
    let port: u16 = std::env::var("NEWPORT_TEST_SSH_PORT")
        .map_err(|e| e.to_string())?
        .parse::<u16>()
        .map_err(|e| e.to_string())?;
    let backend = Backend::discover();
    let snapshot = backend.call("snapshot", json!({}))?;
    let servers: Vec<_> = snapshot["config"]["servers"]
        .as_array()
        .ok_or("Missing servers")?
        .iter()
        .filter(|s| s["sshHost"] == "127.0.0.1" && s["sshPort"] == port)
        .collect();
    require(
        servers.len() == 1,
        "Expected exactly one loopback fixture server",
    )?;
    let server = serde_json::from_value(servers[0]["id"].clone()).map_err(|e| e.to_string())?;
    let before = tunnels(&backend)?;
    let mut original = (65000..=65535)
        .filter_map(|port| discovered_tunnel(&before, server, "127.0.0.1", port).ok())
        .find(|candidate| !before.iter().any(|t| t.id == candidate.id))
        .ok_or("No unused fixture destination")?;
    original.name = "Disposable editor conflict check".into();
    original.auto_reconnect = false;
    // Neither save nor update connects this forward. No SSH or repository writes.
    backend.call("save_tunnel", json!({"tunnel":original}))?;
    let outcome = (|| -> Result<(), String> {
        let mut edited = original.clone();
        edited.name = "Edited disposable forward".into();
        backend.call(
            "update_tunnel",
            json!({"tunnel":edited,"expected":original}),
        )?;
        require(
            tunnels(&backend)?.iter().find(|t| t.id == original.id) == Some(&edited),
            "Fresh edit was not saved",
        )?;

        let mut stale = original.clone();
        stale.auto_reconnect = true;
        expect_failure(
            backend.call("update_tunnel", json!({"tunnel":stale,"expected":original})),
            "changed",
        )?;
        require(
            tunnels(&backend)?.iter().find(|t| t.id == original.id) == Some(&edited),
            "Stale editor overwrote another edit",
        )?;

        let mut wrong_server = edited.clone();
        wrong_server.server_id = uuid::Uuid::new_v4();
        expect_failure(
            backend.call(
                "update_tunnel",
                json!({"tunnel":wrong_server,"expected":edited}),
            ),
            "identity",
        )?;
        backend.call("delete_tunnel", json!({"id":original.id}))?;
        expect_failure(
            backend.call("update_tunnel", json!({"tunnel":stale,"expected":edited})),
            "deleted",
        )?;
        require(
            !tunnels(&backend)?.iter().any(|t| t.id == original.id),
            "Stale editor recreated a deleted forward",
        )?;
        Ok(())
    })();
    // Always attempt cleanup, even if the backend rejected an assertion above.
    backend.call("delete_tunnel", json!({"id":original.id}))?;
    require(
        tunnels(&backend)? == before,
        "Unrelated forwarding configuration changed",
    )?;
    outcome
}
