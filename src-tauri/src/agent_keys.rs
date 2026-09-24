use serde::Serialize;
use std::time::Duration;

pub use crate::platform::ssh_agent::connect;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentKey {
    pub source: String,
    pub comment: String,
    pub fingerprint: String,
    pub algorithm: String,
}
#[derive(Serialize)]
pub struct KeyList {
    pub keys: Vec<AgentKey>,
    pub warnings: Vec<String>,
}
pub async fn list() -> KeyList {
    let mut result = KeyList {
        keys: vec![],
        warnings: vec![],
    };
    for &(source, name) in crate::platform::ssh_agent::SOURCES {
        let read = tokio::time::timeout(Duration::from_secs(3), async {
            let mut agent = connect(Some(source)).await?;
            agent.request_identities().await.map_err(|e| e.to_string())
        })
        .await;
        match read {
            Ok(Ok(keys)) => {
                for key in keys {
                    result.keys.push(AgentKey {
                        source: source.into(),
                        comment: key.comment().into(),
                        fingerprint: key
                            .public_key()
                            .fingerprint(russh::keys::ssh_key::HashAlg::Sha256)
                            .to_string(),
                        algorithm: key.public_key().algorithm().to_string(),
                    });
                }
            }
            _ => result.warnings.push(format!(
                "{name} is unavailable. Check that it is running and SSH agent access is enabled."
            )),
        }
    }
    result
        .keys
        .sort_by(|a, b| (&a.source, &a.fingerprint).cmp(&(&b.source, &b.fingerprint)));
    result
        .keys
        .dedup_by(|a, b| a.source == b.source && a.fingerprint == b.fingerprint);
    result
}
