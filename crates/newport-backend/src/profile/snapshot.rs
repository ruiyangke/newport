//! Direct Stronghold ownership. The vault key is used as a raw 32-byte key
//! provider; no password KDF is applied.
use iota_stronghold::{Client, KeyProvider, SnapshotPath, Stronghold};
use std::path::Path;
use zeroize::Zeroizing;

pub(super) struct Snapshot {
    inner: Stronghold,
    key: KeyProvider,
    path: SnapshotPath,
}
impl Snapshot {
    pub fn new(path: impl AsRef<Path>, key: Vec<u8>) -> Result<Self, String> {
        let key = KeyProvider::try_from(Zeroizing::new(key)).map_err(|e| e.to_string())?;
        let path = SnapshotPath::from_path(path);
        let inner = Stronghold::default();
        if path.exists() {
            inner
                .load_snapshot(&key, &path)
                .map_err(|e| e.to_string())?;
        }
        Ok(Self { inner, key, path })
    }
    pub fn load_client(&self, name: &[u8]) -> Result<Client, String> {
        self.inner.load_client(name).map_err(|e| e.to_string())
    }
    pub fn create_client(&self, name: &[u8]) -> Result<Client, String> {
        self.inner.create_client(name).map_err(|e| e.to_string())
    }
    pub fn save(&self) -> Result<(), String> {
        self.inner
            .commit_with_keyprovider(&self.path, &self.key)
            .map_err(|e| e.to_string())
    }
}
