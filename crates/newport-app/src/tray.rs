//! Status polling belongs to application composition, never to the GUI thread.
use crate::desktop::Integration;
use newport_client::backend::Backend;
use serde_json::{Value, json};
use std::time::Duration;

fn status(snapshot: &Value) -> Option<(usize, bool)> {
    let tunnels = snapshot.get("runtime")?.get("tunnels")?.as_object()?;
    Some((
        tunnels
            .values()
            .filter(|tunnel| tunnel["status"] == "connected")
            .count(),
        tunnels.values().any(|tunnel| tunnel["status"] == "error"),
    ))
}

pub async fn monitor(backend: Backend, desktop: Integration) {
    let mut published = None;
    loop {
        let client = backend.clone();
        let next = tokio::task::spawn_blocking(move || {
            client
                .call_with_timeout("snapshot", json!({}), Duration::from_secs(2))
                .ok()
                .and_then(|value| status(&value))
        })
        .await
        .ok()
        .flatten();
        if published != Some(next) {
            if !desktop.tray_status(next).await {
                return;
            }
            published = Some(next);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_matches_original_connected_count_and_error_precedence() {
        assert_eq!(status(&json!({"runtime":{"tunnels":{}}})), Some((0, false)));
        assert_eq!(
            status(&json!({"runtime":{"tunnels":{
                "a":{"status":"connected"},"b":{"status":"connecting"},
                "c":{"status":"error"},"d":{"status":"connected"}
            }}})),
            Some((2, true))
        );
        assert_eq!(
            status(&json!({"runtime":{"tunnels":{"a":{"status":"error"}}}})),
            Some((0, true))
        );
        assert_eq!(status(&Value::Null), None);
    }
}
