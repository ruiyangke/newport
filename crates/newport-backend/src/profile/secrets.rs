//! The profile's master key: 32 random bytes in OS credential storage that
//! encrypt its SSH passwords and account sign-in. It is created when the first
//! of them is saved, and never replaced while anything it encrypted remains.
//! Its entry is named by the profile ID in `profile-id`.
pub use newport_platform::secrets::Secrets;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};
use zeroize::Zeroizing;

/// Names the profile's key entry: a random ID kept in the profile, so the key
/// follows the folder wherever it moves.
pub(super) const ID_FILE: &str = "profile-id";

pub struct MasterKey {
    storage: Box<dyn Secrets>,
    profile: Profile,
    /// The account versions before 0.7.0 named the key by: the profile hash.
    /// Its entry is kept, so they still find the key.
    path_hash: String,
    cached: Mutex<Option<Zeroizing<Vec<u8>>>>,
}
enum Profile {
    Directory(PathBuf),
    #[cfg(any(test, feature = "test-support"))]
    Fixed(String),
}
impl MasterKey {
    /// The key of a profile directory that exists, in the OS credential
    /// storage. Unsupported platforms never substitute files for it.
    pub fn system(directory: &Path) -> Result<Self, String> {
        let canonical = directory
            .canonicalize()
            .map_err(|error| error.to_string())?;
        #[cfg(target_os = "macos")]
        let storage = system_storage("app.newport.profile-vault");
        #[cfg(not(target_os = "macos"))]
        let storage = system_storage("app.newport/vault");
        Ok(Self::new(
            storage,
            Profile::Directory(directory.to_owned()),
            profile_hash(&canonical),
        ))
    }
    fn new(storage: Box<dyn Secrets>, profile: Profile, path_hash: String) -> Self {
        Self {
            storage,
            profile,
            path_hash,
            cached: Mutex::new(None),
        }
    }
    /// Gives the profile its ID if it has none yet.
    pub(super) fn ensure_id(&self) -> Result<(), String> {
        self.id(true).map(drop)
    }
    /// The key, if this profile has one.
    pub(super) fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        self.load(&mut cached)?;
        Ok(cached.clone())
    }
    /// The key as stored now, created with a new random value if this
    /// profile has none and `may_create` allows it. Concurrent callers share
    /// one key; a key removed while Newport runs is never sealed with again.
    pub(super) fn get_or_create(
        &self,
        may_create: impl FnOnce() -> Result<(), String>,
    ) -> Result<Zeroizing<Vec<u8>>, String> {
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        *cached = None;
        self.load(&mut cached)?;
        if let Some(key) = cached.as_ref() {
            return Ok(key.clone());
        }
        may_create()?;
        let id = self.id(true)?.ok_or("Cannot name the Newport key")?;
        let mut key = Zeroizing::new(vec![0; 32]);
        getrandom::fill(&mut key)
            .map_err(|error| format!("Cannot create a Newport key: {error}"))?;
        self.save(&id, &key)
            .map_err(|_| "The new Newport key could not be verified; nothing was saved.")?;
        *cached = Some(key.clone());
        Ok(key)
    }
    /// The key under the profile ID, or else under the path hash, where
    /// versions before 0.7.0 saved it: then it is also saved under the ID,
    /// leaving the path-hash entry for them.
    fn load(&self, cached: &mut Option<Zeroizing<Vec<u8>>>) -> Result<(), String> {
        if cached.is_some() {
            return Ok(());
        }
        if let Some(id) = self.id(false)?
            && let Some(key) = self.entry(&id)?
        {
            *cached = Some(key);
            return Ok(());
        }
        let Some(key) = self.entry(&self.path_hash)? else {
            return Ok(());
        };
        match self.id(true).and_then(|id| {
            let id = id.ok_or("Cannot name the Newport key")?;
            self.save(&id, &key)
        }) {
            Ok(()) => log::info!("Saved the Newport key under this profile's ID"),
            Err(error) => {
                log::warn!("Cannot save the Newport key under this profile's ID: {error}")
            }
        }
        *cached = Some(key);
        Ok(())
    }
    fn entry(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        match self.storage.read(account)? {
            Some(key) if key.len() == 32 => Ok(Some(key)),
            Some(_) => Err(INVALID.into()),
            None => Ok(None),
        }
    }
    /// Writes `key` under `account` and reads it back.
    fn save(&self, account: &str, key: &[u8]) -> Result<(), String> {
        self.storage.write(account, key)?;
        if self
            .storage
            .read(account)?
            .as_deref()
            .map(|saved| saved.as_slice())
            != Some(key)
        {
            return Err("The Newport key could not be verified".into());
        }
        Ok(())
    }
    /// The profile ID, made with `create` if the profile has none yet. A
    /// damaged ID is an error, never replaced: the key is found by it.
    fn id(&self, create: bool) -> Result<Option<String>, String> {
        match &self.profile {
            Profile::Directory(directory) => directory_id(directory, create),
            #[cfg(any(test, feature = "test-support"))]
            Profile::Fixed(id) => Ok(Some(id.clone())),
        }
    }
}

/// The ID in `directory`'s `profile-id`, made with `create` if it has none.
fn directory_id(directory: &Path, create: bool) -> Result<Option<String>, String> {
    let path = directory.join(ID_FILE);
    let parse = |text: &str| {
        uuid::Uuid::parse_str(text.trim())
            .map(|id| Some(id.to_string()))
            .map_err(|_| {
                format!(
                    "{} is damaged; saved passwords were not changed.",
                    path.display()
                )
            })
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => return parse(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    if !create {
        return Ok(None);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let written = newport_platform::filesystem::private_file_options()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut file| {
            std::io::Write::write_all(&mut file, format!("{id}\n").as_bytes())?;
            file.sync_all()
        });
    match written {
        Ok(()) => {
            let _ = newport_platform::filesystem::sync_directory(directory);
            Ok(Some(id))
        }
        // Another save made it first: use that one.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            parse(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
        }
        Err(error) => Err(error.to_string()),
    }
}

const INVALID: &str = "The Newport key in your system credential storage is invalid; saved passwords were not changed.";

/// Stable across versions: the key is found by it.
pub fn profile_hash(directory: &Path) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(directory.as_os_str().as_encoded_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(any(target_os = "macos", windows))]
fn system_storage(service: &str) -> Box<dyn Secrets> {
    Box::new(newport_platform::secrets::SystemStorage::new(service))
}
#[cfg(not(any(target_os = "macos", windows)))]
fn system_storage(_service: &str) -> Box<dyn Secrets> {
    Box::new(Unavailable)
}
#[cfg(not(any(target_os = "macos", windows)))]
struct Unavailable;
#[cfg(not(any(target_os = "macos", windows)))]
impl Secrets for Unavailable {
    fn read(&self, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        Err(UNAVAILABLE.into())
    }
    fn write(&self, _: &str, _: &[u8]) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
    fn delete(&self, _: &str) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }
}
#[cfg(not(any(target_os = "macos", windows)))]
const UNAVAILABLE: &str = "System credential storage is unavailable on this platform.";

/// In-memory credential storage for tests, shared by every clone.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Default)]
pub struct Memory(
    std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, Zeroizing<Vec<u8>>>>>,
);
#[cfg(any(test, feature = "test-support"))]
impl Memory {
    /// The storage of a test profile directory, kept for the whole process
    /// like the OS storage, so a reopened profile finds the same key.
    pub fn for_directory(directory: &Path) -> Self {
        static PROFILES: std::sync::Mutex<std::collections::BTreeMap<std::path::PathBuf, Memory>> =
            std::sync::Mutex::new(std::collections::BTreeMap::new());
        PROFILES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(directory.to_owned())
            .or_default()
            .clone()
    }
    pub fn names(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }
}
#[cfg(any(test, feature = "test-support"))]
impl Secrets for Memory {
    fn read(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(account)
            .cloned())
    }
    fn write(&self, account: &str, value: &[u8]) -> Result<(), String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(account.into(), Zeroizing::new(value.to_vec()));
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(account);
        Ok(())
    }
}
#[cfg(any(test, feature = "test-support"))]
impl MasterKey {
    /// A key kept in `storage` under the account `profile`, or found under
    /// `fixture`, the path-hash account versions before 0.7.0 used.
    pub fn fixture(storage: impl Secrets + 'static) -> Self {
        Self::new(
            Box::new(storage),
            Profile::Fixed("profile".into()),
            "fixture".into(),
        )
    }
    /// A key for `directory`, named by its `profile-id`, with `path_hash`
    /// standing for its path.
    pub fn fixture_in(directory: &Path, path_hash: &str, storage: impl Secrets + 'static) -> Self {
        Self::new(
            Box::new(storage),
            Profile::Directory(directory.to_owned()),
            path_hash.into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_hash_finds_keys_saved_by_earlier_versions() {
        assert_eq!(
            profile_hash(Path::new("/Users/test/Library/Application Support/Newport")),
            "c70d210acb98917454d7b7e9f149173f8f167d2dc7d18cceb9d125adb22e93bf"
        );
    }

    #[test]
    fn a_created_key_is_random_saved_and_cached() {
        let storage = Memory::default();
        let key = MasterKey::fixture(storage.clone());
        assert!(key.read().unwrap().is_none());
        assert!(key.get_or_create(|| Err("refused".into())).is_err());
        assert!(storage.names().is_empty());
        let created = key.get_or_create(|| Ok(())).unwrap();
        assert_eq!(created.len(), 32);
        assert_ne!(created.as_slice(), [0; 32]);
        assert_eq!(storage.names(), ["profile"]);
        assert_eq!(key.read().unwrap().unwrap().as_slice(), created.as_slice());
        // A key is never replaced, even when creating is allowed.
        assert_eq!(
            key.get_or_create(|| Ok(())).unwrap().as_slice(),
            created.as_slice()
        );
        let other = MasterKey::fixture(Memory::default())
            .get_or_create(|| Ok(()))
            .unwrap();
        assert_ne!(other.as_slice(), created.as_slice());
    }

    #[test]
    fn concurrent_first_saves_share_one_key() {
        let storage = Memory::default();
        let key = std::sync::Arc::new(MasterKey::fixture(storage.clone()));
        let keys: Vec<_> = (0..8)
            .map(|_| {
                let key = key.clone();
                std::thread::spawn(move || key.get_or_create(|| Ok(())).unwrap())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(
            keys.iter()
                .all(|other| other.as_slice() == keys[0].as_slice())
        );
        assert_eq!(
            storage.read("profile").unwrap().unwrap().as_slice(),
            keys[0].as_slice()
        );
    }

    #[test]
    fn an_earlier_versions_key_is_saved_under_the_profile_id_and_kept() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Memory::default();
        storage.write("path-a", &[7; 32]).unwrap();
        let key = MasterKey::fixture_in(directory.path(), "path-a", storage.clone());
        assert_eq!(key.read().unwrap().unwrap().as_slice(), [7; 32]);
        let id = std::fs::read_to_string(directory.path().join(ID_FILE)).unwrap();
        let id = uuid::Uuid::parse_str(id.trim()).unwrap().to_string();
        assert_eq!(storage.read(&id).unwrap().unwrap().as_slice(), [7; 32]);
        // The earlier entry stays for earlier versions.
        assert_eq!(storage.read("path-a").unwrap().unwrap().as_slice(), [7; 32]);
    }

    #[test]
    fn the_key_follows_its_folder_when_it_moves() {
        let parent = tempfile::tempdir().unwrap();
        let (before, after) = (parent.path().join("before"), parent.path().join("after"));
        std::fs::create_dir(&before).unwrap();
        let storage = Memory::default();
        let created = MasterKey::fixture_in(&before, "path-before", storage.clone())
            .get_or_create(|| Ok(()))
            .unwrap();
        assert!(storage.read("path-before").unwrap().is_none());
        std::fs::rename(&before, &after).unwrap();
        let moved = MasterKey::fixture_in(&after, "path-after", storage.clone());
        assert_eq!(
            moved.read().unwrap().unwrap().as_slice(),
            created.as_slice()
        );
        // Another profile has another ID and so another key.
        let other = parent.path().join("other");
        std::fs::create_dir(&other).unwrap();
        let other = MasterKey::fixture_in(&other, "path-other", storage);
        assert!(other.read().unwrap().is_none());
        assert_ne!(
            other.get_or_create(|| Ok(())).unwrap().as_slice(),
            created.as_slice()
        );
    }

    #[test]
    fn a_damaged_profile_id_is_never_replaced() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(ID_FILE), "damaged").unwrap();
        let key = MasterKey::fixture_in(directory.path(), "path", Memory::default());
        assert!(key.read().is_err());
        assert!(key.get_or_create(|| Ok(())).is_err());
        assert_eq!(
            std::fs::read(directory.path().join(ID_FILE)).unwrap(),
            b"damaged"
        );
    }

    #[test]
    fn only_a_32_byte_key_is_used() {
        let storage = Memory::default();
        storage.write("fixture", &[7; 31]).unwrap();
        assert!(MasterKey::fixture(storage.clone()).read().is_err());
        assert!(
            MasterKey::fixture(storage.clone())
                .get_or_create(|| Ok(()))
                .is_err()
        );
        storage.write("fixture", &[7; 32]).unwrap();
        assert_eq!(
            MasterKey::fixture(storage)
                .read()
                .unwrap()
                .unwrap()
                .as_slice(),
            [7; 32]
        );
    }
}
