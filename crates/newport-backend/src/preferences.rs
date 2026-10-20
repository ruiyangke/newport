//! Non-secret preferences, compatible with the original Store plugin's JSON object.
//! The application owns the profile instance lock and shares one Preferences clone
//! family. Transactions reload the file, preserve unknown keys, and publish by rename.
use serde_json::{Map, Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

pub mod projects;
pub mod remote_apps;
#[cfg(test)]
mod tests;

const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct Preferences {
    path: Arc<PathBuf>,
    access: Arc<Mutex<()>>,
    git_identity: Arc<OnceLock<String>>,
}
impl Preferences {
    /// Opening never writes or silently replaces malformed or unreadable preferences.
    pub fn open(directory: &Path) -> Result<Self, String> {
        let state = Self {
            path: Arc::new(directory.join("preferences.json")),
            access: Arc::default(),
            git_identity: Arc::default(),
        };
        state.read()?;
        Ok(state)
    }
    fn read(&self) -> Result<Map<String, Value>, String> {
        let file = match fs::File::open(self.path.as_ref()) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
            Err(error) => return Err(error.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Preferences exceed the storage limit.".into());
        }
        serde_json::from_slice(&bytes).map_err(|e| format!("Preferences could not be read: {e}"))
    }
    fn get(&self, key: &str) -> Result<Option<Value>, String> {
        let _guard = self
            .access
            .lock()
            .map_err(|_| "Preference storage is unavailable.")?;
        Ok(self.read()?.remove(key))
    }
    fn update<T>(
        &self,
        edit: impl FnOnce(&mut Map<String, Value>) -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self
            .access
            .lock()
            .map_err(|_| "Preference storage is unavailable.")?;
        let mut values = self.read()?;
        let original = values.clone();
        let result = edit(&mut values)?;
        // Existing values already came from disk. In particular, opening a Git
        // session must not rewrite preferences just to read its persisted ID.
        if values == original {
            return Ok(result);
        }
        let bytes = serde_json::to_vec_pretty(&values).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Preferences exceed the storage limit.".into());
        }
        let directory = self.path.parent().ok_or("Invalid preferences path")?;
        fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.as_file().sync_all().map_err(|e| e.to_string())?;
        file.persist(self.path.as_ref())
            .map_err(|e| e.to_string())?;
        #[cfg(unix)]
        fs::File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(result)
    }
}

pub fn get_sidebar_width(state: &Preferences) -> Result<Option<f64>, String> {
    Ok(state
        .get("sidebarWidth")?
        .and_then(|v| v.as_f64())
        .filter(|v| (176.0..=300.0).contains(v)))
}
pub async fn set_sidebar_width(state: &Preferences, width: f64) -> Result<(), String> {
    if !width.is_finite() || !(176.0..=300.0).contains(&width) {
        return Err("Sidebar width must be between 176 and 300".into());
    }
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        state.update(|values| {
            values.insert("sidebarWidth".into(), json!(width));
            Ok(())
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

impl Preferences {
    // Runs on a blocking worker. A failed transaction must never populate the
    // cache: the identity may reach an agent only after durable publication.
    fn load_git_client_id(&self) -> Result<String, String> {
        if let Some(id) = self.git_identity.get() {
            return Ok(id.clone());
        }
        let id = self.update(|values| {
            let id = match values.get("gitClientId") {
                Some(value) => value
                    .as_str()
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                    .ok_or("Stored Git client identity is invalid.")?,
                None => uuid::Uuid::new_v4(),
            };
            values.insert("gitClientId".into(), json!(id.to_string()));
            Ok(id.to_string())
        })?;
        // Concurrent cold readers use the same atomic preference transaction,
        // so every successful reader observes the same persisted identity.
        let _ = self.git_identity.set(id.clone());
        Ok(id)
    }
}

impl crate::git::ClientIdentity for Preferences {
    fn git_client_id(&self) -> crate::git::IdentityFuture {
        if let Some(id) = self.git_identity.get().cloned() {
            return Box::pin(std::future::ready(Ok(id)));
        }
        let state = self.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || state.load_git_client_id())
                .await
                .map_err(|error| error.to_string())?
        })
    }
}
