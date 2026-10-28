//! A server's connection identity: every field whose change requires a new SSH
//! session, plus the connection revision and backend instance.
use serde_json::{Value, json};

pub fn connection(server: &Value, snapshot: &Value) -> Value {
    let id = server["id"].as_str().unwrap_or_default();
    json!([
        server["id"],
        server["sshHost"],
        server["sshUser"],
        server["sshPort"],
        server["authMethod"],
        server["identityFile"],
        server["agentSource"],
        server["agentKeyFingerprint"],
        snapshot["runtime"]["connectionRevisions"][id]
            .as_u64()
            .unwrap_or(0),
        snapshot["instanceId"].as_str().unwrap_or_default(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
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
