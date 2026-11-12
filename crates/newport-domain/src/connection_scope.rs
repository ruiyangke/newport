//! A server's connection identity: every field whose change requires a new SSH
//! session, plus the connection revision and backend instance.
use serde_json::{Value, json};

/// The settings part of a server's connection identity: which server, and
/// every field whose change requires a new SSH session.
pub fn settings(server: &Value) -> Value {
    json!([
        server["id"],
        server["sshHost"],
        server["sshUser"],
        server["sshPort"],
        server["authMethod"],
        server["identityFile"],
        server["agentSource"],
        server["agentKeyFingerprint"],
        server["proxy"],
    ])
}

/// The connection identity: [`settings`], then the connection revision and
/// the backend instance.
pub fn connection(server: &Value, snapshot: &Value) -> Value {
    let id = server["id"].as_str().unwrap_or_default();
    let mut parts = match settings(server) {
        Value::Array(parts) => parts,
        _ => unreachable!("settings are a list"),
    };
    parts.push(json!(
        snapshot["runtime"]["connectionRevisions"][id]
            .as_u64()
            .unwrap_or(0)
    ));
    parts.push(json!(snapshot["instanceId"].as_str().unwrap_or_default()));
    Value::Array(parts)
}

/// Whether `identity` is a connection identity of server `id`, shaped as
/// [`connection`] makes them, from a running backend.
pub fn is_connection_of(identity: &Value, id: &str) -> bool {
    let parts = connection(&Value::Null, &Value::Null)
        .as_array()
        .map_or(0, Vec::len);
    identity
        .as_array()
        .is_some_and(|parts_of| parts_of.len() == parts)
        && identity[0].as_str() == Some(id)
        && instance(identity).is_some_and(|instance| !instance.is_empty())
}

/// The backend instance that made `identity`: its last part.
pub fn instance(identity: &Value) -> Option<&str> {
    identity.as_array()?.last()?.as_str()
}

/// The connection revision `identity` was made at: the part before the
/// instance.
pub fn revision(identity: &Value) -> Option<u64> {
    let parts = identity.as_array()?;
    parts.get(parts.len().checked_sub(2)?)?.as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_identity_is_one_of_its_server_from_a_running_backend() {
        let server =
            json!({"id":"s","sshHost":"h","sshUser":"u","sshPort":22,"proxy":{"jump":"j"}});
        let snapshot = json!({"instanceId":"backend"});
        let identity = connection(&server, &snapshot);
        assert!(is_connection_of(&identity, "s"));
        assert_eq!(instance(&identity), Some("backend"));
        assert_eq!(
            revision(&connection(
                &server,
                &json!({"instanceId":"backend","runtime":{"connectionRevisions":{"s":7}}})
            )),
            Some(7)
        );
        assert!(!is_connection_of(&identity, "other"));
        assert!(!is_connection_of(&connection(&server, &json!({})), "s"));
        let mut short = identity.as_array().unwrap().clone();
        short.remove(1);
        assert!(!is_connection_of(&Value::Array(short), "s"));
        assert!(!is_connection_of(&Value::Null, "s"));
    }
    #[test]
    fn the_settings_change_exactly_when_the_server_needs_a_new_connection() {
        let server: crate::model::Server = serde_json::from_value(json!({
            "id":"7a9c1d4e-0000-4000-8000-000000000001","name":"Box","sshHost":"host",
            "sshUser":"user","sshPort":22,"identityFile":null
        }))
        .unwrap();
        let original = serde_json::to_value(&server).unwrap();
        // Every field the model saves, so a new one is checked too.
        for (field, value) in original.as_object().unwrap() {
            if field == "id" {
                continue;
            }
            let changed = match (field.as_str(), value) {
                ("authMethod", _) => json!("password"),
                ("proxy", _) => json!({"command":"nc %h %p"}),
                (_, Value::Bool(flag)) => json!(!flag),
                (_, Value::Number(number)) => json!(number.as_u64().unwrap() + 1),
                _ => json!("changed"),
            };
            let mut updated = original.clone();
            updated[field] = changed;
            let updated: crate::model::Server = serde_json::from_value(updated).unwrap();
            assert_eq!(
                !server.same_connection(&updated),
                settings(&original) != settings(&serde_json::to_value(&updated).unwrap()),
                "{field}"
            );
        }
    }
    #[test]
    fn presentation_and_integration_updates_preserve_sessions() {
        let server = json!({"id":"one","sshHost":"host","sshUser":"user","sshPort":22,"authMethod":"publicKey"});
        let before = connection(&server, &Value::Null);
        let mut updated = server;
        updated["name"] = json!("Renamed");
        updated["clipboardEnabled"] = json!(true);
        updated["browserEnabled"] = json!(true);
        updated["agentSource"] = Value::Null;
        updated["agentKeyFingerprint"] = Value::Null;
        assert_eq!(connection(&updated, &json!({"revision":42})), before);
    }
    #[test]
    fn every_connection_field_and_runtime_epoch_invalidate_sessions() {
        let server = json!({"id":"one","sshHost":"host","sshUser":"user","sshPort":22,"authMethod":"publicKey"});
        let snapshot =
            json!({"instanceId":"backend-one","runtime":{"connectionRevisions":{"one":3}}});
        let before = connection(&server, &snapshot);
        for field in [
            "id",
            "sshHost",
            "sshUser",
            "sshPort",
            "authMethod",
            "identityFile",
            "agentSource",
            "agentKeyFingerprint",
            "proxy",
        ] {
            let mut updated = server.clone();
            updated[field] = json!("changed");
            assert_ne!(connection(&updated, &snapshot), before, "{field}");
        }
        let mut updated = snapshot.clone();
        updated["runtime"]["connectionRevisions"]["one"] = json!(4);
        assert_ne!(connection(&server, &updated), before);
        updated = snapshot;
        updated["instanceId"] = json!("backend-two");
        assert_ne!(connection(&server, &updated), before);
    }
}
