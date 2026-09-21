use crate::{manager::Shared, model::Server};
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
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
