use crate::{manager::Shared, model::Server};
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::Mutex,
};
use tauri::State;
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub name: String,
    pub completed: u64,
    pub total: Option<u64>,
}
struct Operation {
    server: Uuid,
    cancel: watch::Sender<bool>,
    progress: Progress,
}
#[derive(Default)]
pub struct Operations(Mutex<Registry>);
#[derive(Default)]
struct Registry {
    entries: HashMap<Uuid, Operation>,
    cancelled: VecDeque<Uuid>,
    closing: bool,
    cleanup: Vec<tokio::task::JoinHandle<()>>,
}
struct Guard<'a>(&'a Operations, Uuid);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0 .0.lock().unwrap().entries.remove(&self.1);
    }
}
impl Operations {
    pub async fn run<T, F>(
        &self,
        state: &Shared,
        server: Uuid,
        id: Uuid,
        work: F,
    ) -> Result<T, String>
    where
        F: AsyncFnOnce(Server) -> Result<T>,
    {
        let (server, mut cancel, _guard) = {
            // Register under the manager lock so an edit/delete cannot miss new work.
            let manager = state.lock().await;
            let profile = manager.server(server)?;
            let mut registry = self.0.lock().unwrap();
            if registry.closing {
                return Err("Porthop is shutting down".into());
            }
            if let Some(index) = registry
                .cancelled
                .iter()
                .position(|cancelled| *cancelled == id)
            {
                registry.cancelled.remove(index);
                return Err("File operation cancelled".into());
            }
            let entries = &mut registry.entries;
            if entries.len() >= 8 {
                return Err("Too many file operations. Wait for one to finish.".into());
            }
            if entries.contains_key(&id) {
                return Err("File operation already exists".into());
            }
            let (sender, receiver) = watch::channel(false);
            entries.insert(
                id,
                Operation {
                    server,
                    cancel: sender,
                    progress: Progress::default(),
                },
            );
            (profile, receiver, Guard(self, id))
        };
        let result = tokio::select! {
            biased;
            _ = cancel.changed() => Err(anyhow::anyhow!("File operation cancelled")),
            result = work(server) => result,
        };
        result.map_err(|error| format!("{error:#}"))
    }
    pub fn progress(&self, id: Uuid, name: &str, completed: u64, total: Option<u64>) {
        if let Some(entry) = self.0.lock().unwrap().entries.get_mut(&id) {
            entry.progress = Progress {
                name: name.into(),
                completed,
                total,
            };
        }
    }
    pub fn cancel_server(&self, server: Option<Uuid>) {
        for entry in self.0.lock().unwrap().entries.values() {
            if server.is_none_or(|id| id == entry.server) {
                let _ = entry.cancel.send(true);
            }
        }
    }

    pub(super) fn cleanup(&self, work: impl Future<Output = ()> + Send + 'static) {
        let mut registry = self.0.lock().unwrap();
        registry.cleanup.retain(|task| !task.is_finished());
        registry.cleanup.push(tokio::spawn(work));
    }

    pub async fn shutdown(&self) {
        {
            let mut registry = self.0.lock().unwrap();
            registry.closing = true;
            for entry in registry.entries.values() {
                let _ = entry.cancel.send(true);
            }
        }
        // Dropping each operation registers staging-file cleanup before its
        // guard removes the registry entry. Reject new work throughout shutdown.
        let drain = async {
            while !self.0.lock().unwrap().entries.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            let tasks = std::mem::take(&mut self.0.lock().unwrap().cleanup);
            for task in tasks {
                let _ = task.await;
            }
        };
        if tokio::time::timeout(std::time::Duration::from_secs(5), drain)
            .await
            .is_err()
        {
            log::warn!("File cleanup timed out during shutdown; remote staging files may remain");
        }
    }
}
#[tauri::command]
pub fn files_cancel(operations: State<'_, Operations>, operation: Uuid) {
    let mut registry = operations.0.lock().unwrap();
    if let Some(entry) = registry.entries.get(&operation) {
        let _ = entry.cancel.send(true);
    } else {
        // A React cleanup can arrive while registration waits for the manager.
        if registry.cancelled.len() == 128 {
            registry.cancelled.pop_front();
        }
        registry.cancelled.push_back(operation);
    }
}
#[tauri::command]
pub fn files_progress(operations: State<'_, Operations>, operation: Uuid) -> Option<Progress> {
    operations
        .0
        .lock()
        .unwrap()
        .entries
        .get(&operation)
        .map(|entry| entry.progress.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn shutdown_waits_for_cleanup_registered_by_cancelled_operation() {
        let operations = Arc::new(Operations::default());
        let id = Uuid::new_v4();
        let (cancel, mut cancelled) = watch::channel(false);
        operations.0.lock().unwrap().entries.insert(
            id,
            Operation {
                server: Uuid::new_v4(),
                cancel,
                progress: Progress::default(),
            },
        );
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let work = operations.clone();
        let operation = tokio::spawn(async move {
            let _guard = Guard(&work, id);
            cancelled.changed().await.unwrap();
            work.cleanup(async move {
                started.send(()).unwrap();
                released.await.unwrap();
            });
        });
        let work = operations.clone();
        let shutdown = tokio::spawn(async move { work.shutdown().await });
        tokio::time::timeout(std::time::Duration::from_secs(1), ready)
            .await
            .unwrap()
            .unwrap();
        assert!(operations.0.lock().unwrap().closing);
        assert!(!shutdown.is_finished());
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), shutdown)
            .await
            .unwrap()
            .unwrap();
        operation.await.unwrap();
        assert!(operations.0.lock().unwrap().entries.is_empty());
        operations.shutdown().await;
    }
}
