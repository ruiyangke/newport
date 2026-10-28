use newport_domain::model::Config;
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

type KeySource = dyn Fn(&Path) -> Result<zeroize::Zeroizing<Vec<u8>>, String> + Send + Sync;
#[cfg(test)]
type PreparationHook = dyn Fn(&Config) + Send + Sync;

pub struct Store {
    key_source: Box<KeySource>,
    pub directory: PathBuf,
    key: std::sync::OnceLock<Result<zeroize::Zeroizing<Vec<u8>>, String>>,
    #[cfg(test)]
    pub(crate) preparation_hook: std::sync::Mutex<Option<std::sync::Arc<PreparationHook>>>,
}
impl Store {
    /// The application supplies its OS credential adapter. No UI framework is needed.
    pub fn open(
        directory: PathBuf,
        key_source: impl Fn(&Path) -> Result<zeroize::Zeroizing<Vec<u8>>, String>
        + Send
        + Sync
        + 'static,
    ) -> Result<Self, String> {
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        newport_platform::filesystem::protect_directory(&directory).map_err(|e| e.to_string())?;
        Ok(Self {
            directory,
            key_source: Box::new(key_source),
            key: Default::default(),
            #[cfg(test)]
            preparation_hook: Default::default(),
        })
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(directory: PathBuf) -> Self {
        let key = std::sync::OnceLock::new();
        key.set(Ok(zeroize::Zeroizing::new(vec![7; 32]))).unwrap();
        Self {
            directory,
            key,
            key_source: Box::new(|_| Err("Fixture key already initialized".into())),
            #[cfg(test)]
            preparation_hook: Default::default(),
        }
    }
    pub fn vault_key(&self) -> Result<&[u8], String> {
        self.key
            .get_or_init(|| (self.key_source)(&self.directory).and_then(super::credentials::valid))
            .as_ref()
            .map(|key| key.as_slice())
            .map_err(Clone::clone)
    }
    pub fn load(&self) -> Result<Config, String> {
        let vault = self.directory.join("profiles.stronghold");
        if vault.exists() {
            return super::vault::read(&vault, self.vault_key()?);
        }
        // A new profile starts empty. Saving it creates and verifies the vault
        // key now, so credential storage failures surface at first launch.
        let config = Config::default();
        self.save(&config)?;
        Ok(config)
    }
    pub fn save(&self, config: &Config) -> Result<(), String> {
        super::vault::write(
            &self.directory.join("profiles.stronghold"),
            self.vault_key()?,
            config,
        )
    }
    pub fn save_with_password(
        &self,
        config: &Config,
        password: Option<(Uuid, &str)>,
    ) -> Result<(), String> {
        super::vault::write_with_password(
            &self.directory.join("profiles.stronghold"),
            self.vault_key()?,
            config,
            password,
        )
    }
    pub fn password(&self, id: Uuid) -> Result<zeroize::Zeroizing<String>, String> {
        super::vault::password(
            &self.directory.join("profiles.stronghold"),
            self.vault_key()?,
            id,
        )
    }
    /// The saved Newport account sign-in, if this computer has one.
    pub fn account(&self) -> Result<Option<super::account::Record>, String> {
        super::account::read(
            &self.directory.join(super::account::FILE),
            self.vault_key()?,
        )
    }
    pub fn save_account(&self, record: &super::account::Record) -> Result<(), String> {
        super::account::write(
            &self.directory.join(super::account::FILE),
            self.vault_key()?,
            record,
        )
    }
    pub fn remove_account(&self) -> Result<(), String> {
        super::account::remove(&self.directory.join(super::account::FILE))
    }
    pub(crate) fn prepare(
        &self,
        config: &Config,
        password: Option<(Uuid, &str)>,
    ) -> Result<super::vault::Prepared, String> {
        #[cfg(test)]
        {
            let hook = self.preparation_hook.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook(config);
            }
        }
        super::vault::prepare(
            &self.directory.join("profiles.stronghold"),
            self.vault_key()?,
            config,
            password,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_key_source_is_cached_and_failures_cannot_replace_a_vault() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let store = Store::open(directory.path().into(), move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(zeroize::Zeroizing::new(vec![7; 32]))
        })
        .unwrap();
        store.save(&Config::default()).unwrap();
        store.load().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let path = directory.path().join("profiles.stronghold");
        let original = fs::read(&path).unwrap();
        let denied = Store::open(directory.path().into(), |_| {
            Err("Credential access denied".into())
        })
        .unwrap();
        assert_eq!(denied.load().unwrap_err(), "Credential access denied");
        assert!(denied.save(&Config::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        let invalid = Store::open(directory.path().into(), |_| {
            Ok(zeroize::Zeroizing::new(vec![7; 31]))
        })
        .unwrap();
        assert!(invalid.vault_key().is_err());
        assert!(invalid.save(&Config::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    #[test]
    fn new_profile_starts_empty_and_never_replaces_a_damaged_vault() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::for_test(dir.path().into());
        assert_eq!(store.load().unwrap(), Config::default());
        let vault = dir.path().join("profiles.stronghold");
        assert!(vault.exists());
        assert_eq!(store.load().unwrap(), Config::default());
        fs::write(&vault, "damaged").unwrap();
        assert!(store.load().is_err());
        assert_eq!(fs::read(&vault).unwrap(), b"damaged");
    }
}
