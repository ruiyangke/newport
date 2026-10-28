//! History writes and UUID reuse serialize per server. Only an explicit clear
//! of the entire database needs exclusive application-wide coordination.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{
    Mutex as AsyncMutex, OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock,
};
use uuid::Uuid;

#[derive(Default)]
pub struct HistoryGates {
    clear: Arc<RwLock<()>>,
    servers: Mutex<HashMap<Uuid, Weak<AsyncMutex<()>>>>,
}

pub struct ServerHistoryGuard {
    _server: OwnedMutexGuard<()>,
    _clear: OwnedRwLockReadGuard<()>,
}

impl HistoryGates {
    pub async fn server(&self, id: Uuid) -> ServerHistoryGuard {
        let clear = self.clear.clone().read_owned().await;
        let gate = {
            let mut servers = self.servers.lock().expect("history gates");
            servers.retain(|_, entry| entry.strong_count() > 0);
            if let Some(gate) = servers.get(&id).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(AsyncMutex::new(()));
                servers.insert(id, Arc::downgrade(&gate));
                gate
            }
        };
        ServerHistoryGuard {
            _server: gate.lock_owned().await,
            _clear: clear,
        }
    }

    pub async fn clear(&self) -> OwnedRwLockWriteGuard<()> {
        self.clear.clone().write_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn servers_progress_independently_and_global_clear_waits_for_readers() {
        let gates = HistoryGates::default();
        let a = Uuid::new_v4();
        let held = gates.server(a).await;
        let other = tokio::time::timeout(Duration::from_millis(100), gates.server(Uuid::new_v4()))
            .await
            .expect("history for another server must remain independent");
        assert!(
            tokio::time::timeout(Duration::from_millis(10), gates.server(a))
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), gates.clear())
                .await
                .is_err()
        );
        drop((held, other));
        let clear = gates.clear().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), gates.server(a))
                .await
                .is_err()
        );
        drop(clear);
        let _resumed = tokio::time::timeout(Duration::from_millis(100), gates.server(a))
            .await
            .unwrap();
        assert_eq!(gates.servers.lock().unwrap().len(), 1);
    }
}
