use super::{account, config_file, import, seal, secrets::MasterKey};
use newport_domain::model::Config;
use std::{fs, path::PathBuf};
use uuid::Uuid;
use zeroize::Zeroizing;

#[cfg(test)]
type PreparationHook = dyn Fn(&Config) + Send + Sync;

/// `account.json`: the account sign-in, sealed whole with the master key.
pub(super) const ACCOUNT_FILE: &str = "account.json";
const ACCOUNT_SCHEMA: u64 = 1;
const MISSING_KEY: &str = "The Newport key is missing from your system credential storage, so saved passwords can't be read. Restore it; nothing has been changed.";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SealedAccount {
    schema_version: u64,
    sealed: String,
}

pub struct Store {
    pub directory: PathBuf,
    key: MasterKey,
    #[cfg(test)]
    pub(crate) preparation_hook: std::sync::Mutex<Option<std::sync::Arc<PreparationHook>>>,
}
impl Store {
    /// The application supplies the profile's master key storage. No UI framework is needed.
    pub fn open(
        directory: PathBuf,
        key: impl FnOnce(&std::path::Path) -> Result<MasterKey, String>,
    ) -> Result<Self, String> {
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        newport_platform::filesystem::protect_directory(&directory).map_err(|e| e.to_string())?;
        let key = key(&directory)?;
        Ok(Self {
            directory,
            key,
            #[cfg(test)]
            preparation_hook: Default::default(),
        })
    }
    /// A profile whose master key stays in memory for this process, as
    /// `[7; 32]`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test(directory: PathBuf) -> Self {
        use super::secrets::{Memory, Secrets};
        let storage = Memory::for_directory(&directory);
        storage.write("fixture", &[7; 32]).unwrap();
        Self::open(directory, |_| Ok(MasterKey::fixture(storage))).expect("Test profile directory")
    }
    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }
    fn saved_by_earlier_version(&self) -> bool {
        self.path(import::KEY_MARKER).exists() || self.path(import::PROFILES).exists()
    }
    fn record(&self) -> Result<config_file::Record, String> {
        Ok(config_file::read(&self.path(config_file::FILE))?.unwrap_or_default())
    }
    fn reading_key(&self) -> Result<Zeroizing<Vec<u8>>, String> {
        self.key.read()?.ok_or_else(|| MISSING_KEY.into())
    }
    /// The key to seal a new secret with, created for the first one. A key
    /// is never replaced while anything it sealed remains.
    fn sealing_key(&self) -> Result<Zeroizing<Vec<u8>>, String> {
        self.key.get_or_create(|| {
            let record = config_file::read(&self.path(config_file::FILE))?;
            // Earlier versions' profiles count until they are imported.
            let unimported = record.is_none() && self.path(import::PROFILES).exists();
            if record.is_some_and(|record| !record.passwords.is_empty())
                || unimported
                || self.path(ACCOUNT_FILE).exists()
                || self.path(import::ACCOUNT).exists()
            {
                return Err(MISSING_KEY.into());
            }
            Ok(())
        })
    }
    /// The saved configuration. A profile without `profiles.json` imports the
    /// one an earlier version saved, if any, and otherwise starts empty.
    pub fn load(&self) -> Result<Config, String> {
        self.key.ensure_id()?;
        if let Some(record) = config_file::read(&self.path(config_file::FILE))? {
            // A profile from an earlier version names its key by its ID from now on.
            if self.saved_by_earlier_version()
                && let Err(error) = self.key.read()
            {
                log::warn!("{error}");
            }
            return Ok(record.config);
        }
        let earlier = self.path(import::PROFILES);
        if !earlier.exists() {
            if self.saved_by_earlier_version() {
                self.key.read()?;
            }
            return Ok(Config::default());
        }
        let key = self.key.read()?;
        let imported = import::profiles(&earlier, key.as_deref().map(|key| key.as_slice()))?;
        let key = key.ok_or(MISSING_KEY)?;
        let mut passwords = config_file::Passwords::new();
        for (id, password) in &imported.passwords {
            passwords.insert(
                *id,
                seal::seal(&key, &seal::password_purpose(*id), password.as_bytes())?,
            );
        }
        config_file::stage(&self.path(config_file::FILE), &imported.config, &passwords)?
            .commit()?;
        log::info!(
            "Imported {} servers saved by an earlier version",
            imported.config.servers.len()
        );
        Ok(imported.config.clone())
    }
    pub fn save(&self, config: &Config) -> Result<(), String> {
        self.prepare(config, None)?.commit()
    }
    pub fn save_with_password(
        &self,
        config: &Config,
        password: Option<(Uuid, &str)>,
    ) -> Result<(), String> {
        self.prepare(config, password)?.commit()
    }
    pub fn password(&self, id: Uuid) -> Result<Zeroizing<String>, String> {
        let record = self.record()?;
        let sealed = record
            .passwords
            .get(&id)
            .ok_or("No saved SSH password. Save a password in Edit Server.")?;
        let password = seal::open(&self.reading_key()?, &seal::password_purpose(id), sealed)?;
        String::from_utf8(password.to_vec())
            .map(Zeroizing::new)
            .map_err(|_| "The saved SSH password is damaged. Save it again in Edit Server.".into())
    }
    /// The key that sealed this profile's secrets, if it has one: also the
    /// key earlier versions encrypted their files and metrics with.
    pub fn earlier_key(&self) -> Option<Zeroizing<Vec<u8>>> {
        self.key
            .read()
            .inspect_err(|error| log::warn!("{error}"))
            .ok()
            .flatten()
    }
    /// The saved Newport account sign-in, if this computer has one. A sign-in
    /// an earlier version saved moves to `account.json` on first read.
    pub fn account(&self) -> Result<Option<account::Record>, String> {
        let path = self.path(ACCOUNT_FILE);
        match fs::read(&path) {
            Ok(data) => {
                let saved: SealedAccount = serde_json::from_slice(&data)
                    .map_err(|_| "Invalid saved Newport sign-in.".to_string())?;
                if saved.schema_version != ACCOUNT_SCHEMA {
                    return Err("Unsupported saved sign-in. Update Newport to use it.".into());
                }
                let data = seal::open(&self.reading_key()?, seal::ACCOUNT_PURPOSE, &saved.sealed)?;
                return account::decode(&data).map(Some);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        let earlier = self.path(import::ACCOUNT);
        if !earlier.exists() {
            return Ok(None);
        }
        let key = self.key.read()?;
        let record = import::account(&earlier, key.as_deref().map(|key| key.as_slice()))?;
        if let Some(record) = &record {
            self.save_account(record)?;
        }
        remove_file(&earlier)?;
        Ok(record)
    }
    pub fn save_account(&self, record: &account::Record) -> Result<(), String> {
        let sealed = seal::seal(
            &self.sealing_key()?,
            seal::ACCOUNT_PURPOSE,
            &account::encode(record)?,
        )?;
        let data = serde_json::to_vec(&SealedAccount {
            schema_version: ACCOUNT_SCHEMA,
            sealed,
        })
        .map_err(|e| e.to_string())?;
        config_file::stage_file(&self.path(ACCOUNT_FILE), &data)?.commit()?;
        // The new sign-in replaces one an earlier version saved.
        remove_file(&self.path(import::ACCOUNT))
    }
    /// Signing out leaves no credential, including one an earlier version saved.
    pub fn remove_account(&self) -> Result<(), String> {
        remove_file(&self.path(import::ACCOUNT))?;
        remove_file(&self.path(ACCOUNT_FILE))
    }
    pub(crate) fn prepare(
        &self,
        config: &Config,
        password: Option<(Uuid, &str)>,
    ) -> Result<config_file::Prepared, String> {
        #[cfg(test)]
        {
            let hook = self
                .preparation_hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(hook) = hook {
                hook(config);
            }
        }
        let password = match password {
            Some((id, password)) => Some((
                id,
                seal::seal(
                    &self.sealing_key()?,
                    &seal::password_purpose(id),
                    password.as_bytes(),
                )?,
            )),
            None => None,
        };
        config_file::prepare(&self.path(config_file::FILE), config, password)
    }
}

fn remove_file(path: &std::path::Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => {
            if let Some(directory) = path.parent() {
                let _ = newport_platform::filesystem::sync_directory(directory);
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::secrets::{Memory, Secrets};

    fn config() -> serde_json::Value {
        serde_json::json!({"servers":[{"id":"8d43df10-73f7-4358-b818-c41dddc2006e","name":"Imported","sshHost":"imported.example","sshPort":22,"sshUser":"me","identityFile":null,"authMethod":"password"}],"tunnels":[]})
    }
    fn id() -> Uuid {
        "8d43df10-73f7-4358-b818-c41dddc2006e".parse().unwrap()
    }
    fn open_store(directory: &std::path::Path, key: Option<[u8; 32]>) -> (Store, Memory) {
        let storage = Memory::default();
        if let Some(key) = key {
            storage.write("fixture", &key).unwrap();
        }
        let key = MasterKey::fixture(storage.clone());
        (Store::open(directory.into(), |_| Ok(key)).unwrap(), storage)
    }
    fn record() -> account::Record {
        account::Record::new(
            "credential".into(),
            "device".into(),
            serde_json::json!({"state":"pro","token":"v4.public.x"}),
        )
    }

    #[test]
    fn a_profile_without_secrets_never_creates_a_key() {
        let directory = tempfile::tempdir().unwrap();
        let (store, storage) = open_store(directory.path(), None);
        assert_eq!(store.load().unwrap(), Config::default());
        assert!(!store.path(config_file::FILE).exists());
        assert!(store.account().unwrap().is_none());
        store
            .save(&serde_json::from_value(config()).unwrap())
            .unwrap();
        assert!(storage.names().is_empty());
    }

    #[test]
    fn the_first_password_creates_the_key_and_it_is_never_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let (store, storage) = open_store(directory.path(), None);
        let config: Config = serde_json::from_value(config()).unwrap();
        store
            .save_with_password(&config, Some((id(), "secret")))
            .unwrap();
        assert_eq!(storage.names(), ["profile"]);
        assert_eq!(store.password(id()).unwrap().as_str(), "secret");
        let text = fs::read_to_string(store.path(config_file::FILE)).unwrap();
        assert!(!text.contains("secret") && text.contains("v1."));

        // With the key gone, nothing is sealed with a new one.
        let (reopened, storage) = open_store(directory.path(), None);
        assert!(reopened.password(id()).unwrap_err().contains("missing"));
        let saved = fs::read(store.path(config_file::FILE)).unwrap();
        assert!(
            reopened
                .save_with_password(&config, Some((id(), "other")))
                .is_err()
        );
        assert!(reopened.save_account(&record()).is_err());
        assert!(storage.names().is_empty());
        assert_eq!(fs::read(store.path(config_file::FILE)).unwrap(), saved);
    }

    #[test]
    fn a_key_removed_while_running_is_never_sealed_with_again() {
        let directory = tempfile::tempdir().unwrap();
        let (store, storage) = open_store(directory.path(), None);
        let config: Config = serde_json::from_value(config()).unwrap();
        store
            .save_with_password(&config, Some((id(), "secret")))
            .unwrap();
        storage.delete("profile").unwrap();
        assert!(
            store
                .save_with_password(&config, Some((id(), "other")))
                .unwrap_err()
                .contains("missing")
        );
        assert!(storage.names().is_empty());
    }

    #[test]
    fn imported_profiles_from_an_earlier_version_never_block_a_new_key() {
        let directory = tempfile::tempdir().unwrap();
        import::write_profiles_fixture(
            &directory.path().join(import::PROFILES),
            &[9; 32],
            &serde_json::json!({"schemaVersion":3,"config":config()}),
        );
        let (store, storage) = open_store(directory.path(), Some([9; 32]));
        let config: Config = serde_json::from_value(config()).unwrap();
        assert_eq!(store.load().unwrap(), config);
        // The key is lost after the import, with nothing sealed yet.
        storage.delete("fixture").unwrap();
        let (store, storage) = open_store(directory.path(), None);
        store
            .save_with_password(&config, Some((id(), "new")))
            .unwrap();
        assert_eq!(storage.names(), ["profile"]);
        assert_eq!(store.password(id()).unwrap().as_str(), "new");
    }

    #[test]
    fn signing_in_replaces_a_sign_in_an_earlier_version_saved() {
        let directory = tempfile::tempdir().unwrap();
        let earlier = directory.path().join(import::ACCOUNT);
        import::write_account_fixture(&earlier, &[9; 32], &record());
        let (store, _) = open_store(directory.path(), Some([9; 32]));
        let mut newer = record();
        newer.credential = "newer".into();
        store.save_account(&newer).unwrap();
        assert!(!earlier.exists());
        assert_eq!(store.account().unwrap().unwrap().credential, "newer");
    }

    #[test]
    fn the_first_launch_gives_every_profile_its_id_and_moves_an_earlier_key() {
        use crate::profile::secrets::ID_FILE;
        // A new profile gets its ID without touching credential storage.
        let directory = tempfile::tempdir().unwrap();
        let storage = Memory::default();
        let key = MasterKey::fixture_in(directory.path(), "path", storage.clone());
        let store = Store::open(directory.path().into(), |_| Ok(key)).unwrap();
        store.load().unwrap();
        assert!(directory.path().join(ID_FILE).is_file());
        assert!(storage.names().is_empty());

        // A profile from an earlier version also has its key saved under the ID.
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(import::KEY_MARKER), "newport-v1\n").unwrap();
        import::write_profiles_fixture(
            &directory.path().join(import::PROFILES),
            &[9; 32],
            &serde_json::json!({"schemaVersion":3,"config":config()}),
        );
        let storage = Memory::default();
        storage.write("path", &[9; 32]).unwrap();
        let key = MasterKey::fixture_in(directory.path(), "path", storage.clone());
        let store = Store::open(directory.path().into(), |_| Ok(key)).unwrap();
        store.load().unwrap();
        let id = fs::read_to_string(directory.path().join(ID_FILE)).unwrap();
        assert_eq!(
            storage.read(id.trim()).unwrap().unwrap().as_slice(),
            [9; 32]
        );
        assert_eq!(storage.read("path").unwrap().unwrap().as_slice(), [9; 32]);
    }

    #[test]
    fn a_damaged_profile_is_never_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = open_store(directory.path(), None);
        let path = store.path(config_file::FILE);
        fs::write(&path, "damaged").unwrap();
        assert!(store.load().is_err());
        assert!(store.save(&Config::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"damaged");
    }

    #[test]
    fn a_sealed_password_belongs_to_its_server() {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = open_store(directory.path(), Some([9; 32]));
        let mut config: Config = serde_json::from_value(config()).unwrap();
        let mut other = config.servers[0].clone();
        other.id = Uuid::new_v4();
        other.name = "Other".into();
        config.servers.push(other.clone());
        store
            .save_with_password(&config, Some((id(), "first")))
            .unwrap();
        // Moving the sealed value to another server doesn't make it that server's password.
        let path = store.path(config_file::FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let sealed = value["passwords"][id().to_string()].clone();
        value["passwords"][other.id.to_string()] = sealed;
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(store.password(other.id).is_err());
        assert_eq!(store.password(id()).unwrap().as_str(), "first");
    }

    #[test]
    fn earlier_profiles_import_once_with_their_passwords() {
        let directory = tempfile::tempdir().unwrap();
        let earlier = directory.path().join(import::PROFILES);
        import::write_profiles_fixture(
            &earlier,
            &[9; 32],
            &serde_json::json!({"schemaVersion":3,"config":config(),"passwords":{id().to_string():"imported"}}),
        );
        let original = fs::read(&earlier).unwrap();

        // Without its key nothing changes, no key is made, and the next launch tries again.
        let (locked, storage) = open_store(directory.path(), None);
        assert!(locked.load().unwrap_err().contains("Cannot unlock"));
        assert!(!locked.path(config_file::FILE).exists());
        assert!(
            locked
                .save_with_password(
                    &serde_json::from_value(config()).unwrap(),
                    Some((id(), "x"))
                )
                .is_err()
        );
        assert!(storage.names().is_empty());

        let (store, _) = open_store(directory.path(), Some([9; 32]));
        let config: Config = serde_json::from_value(config()).unwrap();
        assert_eq!(store.load().unwrap(), config);
        assert_eq!(store.password(id()).unwrap().as_str(), "imported");
        // The earlier file stays as it was, for an earlier version to read.
        assert_eq!(fs::read(&earlier).unwrap(), original);

        // profiles.json now wins: a change is never undone by importing again.
        let mut edited = config.clone();
        edited.servers[0].name = "Edited".into();
        store.save(&edited).unwrap();
        assert_eq!(store.load().unwrap(), edited);
        assert_eq!(store.password(id()).unwrap().as_str(), "imported");
    }

    #[test]
    fn a_sign_in_is_sealed_moves_from_an_earlier_version_and_sign_out_removes_it() {
        let directory = tempfile::tempdir().unwrap();
        let earlier = directory.path().join(import::ACCOUNT);
        import::write_account_fixture(&earlier, &[9; 32], &record());

        let (locked, _) = open_store(directory.path(), None);
        assert!(locked.account().is_err());
        assert!(earlier.exists());

        let (store, _) = open_store(directory.path(), Some([9; 32]));
        assert_eq!(store.account().unwrap(), Some(record()));
        assert!(!earlier.exists());
        let text = fs::read_to_string(store.path(ACCOUNT_FILE)).unwrap();
        assert!(!text.contains("credential") && !text.contains("v4.public"));
        assert_eq!(store.account().unwrap(), Some(record()));

        import::write_account_fixture(&earlier, &[9; 32], &record());
        store.remove_account().unwrap();
        assert!(!earlier.exists() && !store.path(ACCOUNT_FILE).exists());
        assert!(store.account().unwrap().is_none());
    }
}
