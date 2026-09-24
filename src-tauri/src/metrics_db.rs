//! Metric history on the SQL plugin's pool. Sampling and transactions stay in Rust.
mod rollup;
use crate::model::Server;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{pool::PoolConnection, Sqlite, SqlitePool};
use std::{
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::Manager;

const RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const SCHEMA: &str = include_str!("../migrations/001_metrics.sql");

#[derive(Clone)]
pub struct MetricsDb {
    pool: Result<SqlitePool, String>,
    maintenance: Arc<tokio::sync::RwLock<()>>,
    key: Arc<Result<zeroize::Zeroizing<Vec<u8>>, String>>,
}
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MetricsCache {
    pub bytes: u64,
    pub samples: u64,
}
#[derive(Serialize, Deserialize, Debug)]
pub struct SavedSample {
    pub at: i64,
    pub data: Value,
}
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn endpoint(server: &Server) -> String {
    serde_json::to_string(&(&server.ssh_host, server.ssh_port, &server.ssh_user))
        .expect("Endpoint contains only JSON-serializable strings and a port")
}

pub fn migrations() -> Vec<tauri_plugin_sql::Migration> {
    vec![
        tauri_plugin_sql::Migration {
            version: 1,
            description: "metric history",
            sql: SCHEMA,
            kind: tauri_plugin_sql::MigrationKind::Up,
        },
        tauri_plugin_sql::Migration {
            version: 2,
            description: "minute metric summaries",
            sql: include_str!("../migrations/002_metrics_resolution.sql"),
            kind: tauri_plugin_sql::MigrationKind::Up,
        },
    ]
}

/// Use the existing profile directory (including isolated profiles), not the
/// plugin's default app-config directory. Pre-create with private permissions.
pub fn database_url(path: &Path) -> Result<String, String> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| e.to_string())?;
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let url = url::Url::from_file_path(path).map_err(|_| "Invalid metrics database path")?;
    Ok(format!("sqlite:{}", &url.as_str()["file://".len()..]))
}

impl MetricsDb {
    pub async fn from_plugin<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        url: &str,
        key: Result<Vec<u8>, String>,
    ) -> Result<Self, String> {
        let pool = if key.is_ok() {
            let instances = app.state::<tauri_plugin_sql::DbInstances>();
            let pools = instances.0.read().await;
            let Some(tauri_plugin_sql::DbPool::Sqlite(pool)) = pools.get(url) else {
                return Err("SQL plugin did not load metric history".into());
            };
            Ok(pool.clone())
        } else {
            Err("Metric history is unavailable while the profile vault is locked".into())
        };
        let db = Self {
            pool,
            maintenance: Default::default(),
            key: Arc::new(key.map(zeroize::Zeroizing::new)),
        };
        if db.pool.is_ok() {
            db.initialize().await?;
        }
        Ok(db)
    }
    fn pool(&self) -> Result<&SqlitePool, String> {
        self.pool.as_ref().map_err(Clone::clone)
    }
    /// These PRAGMAs are connection-local: apply them on every checked-out
    /// connection, including connections the plugin replaces after an idle period.
    async fn connection(&self) -> Result<PoolConnection<Sqlite>, String> {
        let mut connection = self.pool()?.acquire().await.map_err(|e| e.to_string())?;
        for pragma in [
            "PRAGMA busy_timeout=3000",
            "PRAGMA synchronous=NORMAL",
            "PRAGMA secure_delete=ON",
        ] {
            sqlx::query(pragma)
                .execute(&mut *connection)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(connection)
    }
    fn fingerprint(&self, endpoint: &str) -> Result<String, String> {
        use hmac::{Hmac, KeyInit, Mac};
        let key = self.key.as_ref().as_ref().map_err(Clone::clone)?;
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key).map_err(|e| e.to_string())?;
        mac.update(b"porthop-metrics-endpoint-v2\0");
        mac.update(endpoint.as_bytes());
        Ok(mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
    async fn initialize(&self) -> Result<(), String> {
        let mut connection = self.connection().await?;
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&mut *connection)
            .await
            .map_err(|e| e.to_string())?;
        if version > 2 {
            return Err("Metrics database was created by a newer Porthop version".into());
        }
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&mut *connection)
            .await
            .map_err(|e| e.to_string())?;
        if version < 2 {
            use sqlx::Connection;
            let mut tx = connection.begin().await.map_err(|e| e.to_string())?;
            let endpoints: Vec<String> =
                sqlx::query_scalar("SELECT DISTINCT endpoint FROM samples")
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(|e| e.to_string())?;
            for value in endpoints {
                sqlx::query("UPDATE samples SET endpoint=?1 WHERE endpoint=?2")
                    .bind(self.fingerprint(&value)?)
                    .bind(value)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            sqlx::query("PRAGMA user_version=2")
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())?;
        }
        // Retry physical cleanup after an interrupted legacy endpoint migration.
        sqlx::raw_sql("PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")
            .execute(&mut *connection)
            .await
            .map_err(|e| format!("Cannot compact metric history: {e}"))?;
        Ok(())
    }
    pub async fn record(&self, server: &Server, at: i64, overview: &Value) -> Result<(), String> {
        let _guard = self.maintenance.read().await;
        // Store measurements only; never process lists, log output, command output or credentials.
        let mut data = serde_json::Map::new();
        for key in [
            "cpu",
            "memoryUsed",
            "memoryTotal",
            "swapUsed",
            "swapTotal",
            "load",
            "uptime",
            "network",
            "disks",
        ] {
            if let Some(value) = overview.get(key) {
                data.insert(key.into(), value.clone());
            }
        }
        if !data.get("cpu").is_some_and(Value::is_number)
            || !data.get("network").is_some_and(Value::is_array)
        {
            return Err("Incomplete metric sample".into());
        }
        let encoded = serde_json::to_string(&data).map_err(|e| e.to_string())?;
        if encoded.len() > 128 * 1024 {
            return Err("Metric sample exceeds storage limit".into());
        }
        use sqlx::Connection;
        let fingerprint = self.fingerprint(&endpoint(server))?;
        let mut connection = self.connection().await?;
        let mut tx = connection.begin().await.map_err(|e| e.to_string())?;
        sqlx::query(
            "INSERT OR REPLACE INTO samples(server_id,endpoint,at,data) SELECT ?1,?2,?3,?4 WHERE NOT EXISTS (SELECT 1 FROM samples WHERE server_id=?1 AND endpoint=?2 AND resolution=60000 AND at=(?3/60000)*60000)",
        )
        .bind(server.id.to_string())
        .bind(fingerprint)
        .bind(at)
        .bind(encoded)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        sqlx::query("DELETE FROM samples WHERE at < ?1")
            .bind(at - RETENTION_MS)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())
    }
    #[cfg(test)]
    pub async fn history(&self, server: &Server, now: i64) -> Result<Vec<SavedSample>, String> {
        self.history_range(server, now, 15).await
    }
    pub async fn history_range(
        &self,
        server: &Server,
        now: i64,
        minutes: u32,
    ) -> Result<Vec<SavedSample>, String> {
        let minutes = minutes.clamp(1, 7 * 24 * 60);
        let _guard = self.maintenance.read().await;
        let fingerprint = self.fingerprint(&endpoint(server))?;
        let mut connection = self.connection().await?;
        sqlx::query("DELETE FROM samples WHERE at < ?1")
            .bind(now - RETENTION_MS)
            .execute(&mut *connection)
            .await
            .map_err(|e| format!("Cannot prune metric history: {e}"))?;
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT at, data FROM samples WHERE server_id=?1 AND endpoint=?2 AND at<=?3 AND at>=?4 ORDER BY at DESC LIMIT ?5")
            .bind(server.id.to_string()).bind(fingerprint).bind(now).bind(now - i64::from(minutes) * 60_000).bind(i64::from(minutes) * 12)
            .fetch_all(&mut *connection).await.map_err(|e| format!("Cannot query metric history: {e}"))?;
        rows.into_iter()
            .rev()
            .map(|(at, encoded)| {
                let data = serde_json::from_str(&encoded)
                    .map_err(|e| format!("Invalid stored sample at {at}: {e}"))?;
                Ok(SavedSample { at, data })
            })
            .collect()
    }
    pub async fn cache_info(&self) -> Result<MetricsCache, String> {
        let _guard = self.maintenance.read().await;
        self.cache_info_inner().await
    }
    async fn cache_info_inner(&self) -> Result<MetricsCache, String> {
        let pool = self.pool()?;
        let samples: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM samples")
            .fetch_one(pool)
            .await
            .map_err(|e| e.to_string())?;
        let path = pool.connect_options().get_filename().to_path_buf();
        let bytes = tokio::task::spawn_blocking(move || {
            let mut bytes = 0;
            for suffix in ["", "-wal", "-shm"] {
                let mut name = path.as_os_str().to_os_string();
                name.push(suffix);
                match std::fs::metadata(&name) {
                    Ok(metadata) => bytes += metadata.len(),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            Ok(bytes)
        })
        .await
        .map_err(|e| e.to_string())??;
        Ok(MetricsCache {
            bytes,
            samples: samples as u64,
        })
    }
    pub async fn clear_cache(&self) -> Result<MetricsCache, String> {
        let _guard = self.maintenance.write().await;
        let mut connection = self.connection().await?;
        sqlx::query("DELETE FROM samples")
            .execute(&mut *connection)
            .await
            .map_err(|e| format!("Could not clear metric history: {e}"))?;
        sqlx::query("VACUUM")
            .execute(&mut *connection)
            .await
            .map_err(|e| format!("History cleared, but storage could not be compacted: {e}"))?;
        let (busy, _, _): (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&mut *connection)
            .await
            .map_err(|e| e.to_string())?;
        if busy != 0 {
            return Err("History cleared, but storage is busy. Try clearing again.".into());
        }
        drop(connection);
        self.cache_info_inner().await
    }
    pub async fn delete_server(&self, id: uuid::Uuid) -> Result<(), String> {
        let _guard = self.maintenance.read().await;
        let mut connection = self.connection().await?;
        sqlx::query("DELETE FROM samples WHERE server_id=?1")
            .bind(id.to_string())
            .execute(&mut *connection)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::test::{mock_builder, mock_context, noop_assets};

    fn server() -> Server {
        serde_json::from_value(serde_json::json!({"id":uuid::Uuid::new_v4(),"name":"Test","sshHost":"host","sshPort":22,"sshUser":"user","identityFile":null,"authMethod":"publicKey"})).unwrap()
    }
    fn sample() -> Value {
        serde_json::json!({"cpu":12.0,"memoryUsed":4,"memoryTotal":8,"load":[1,2,3],"uptime":120,"network":[],"processes":["secret-command"],"password":"secret"})
    }
    // Exercise the real plugin preload and migration path without a webview.
    async fn open(path: &Path, key: u8) -> Result<MetricsDb, String> {
        let url = database_url(path)?;
        let mut context = mock_context(noop_assets());
        context.config_mut().identifier = "ke.ry.porthop.metrics-tests".into();
        context
            .config_mut()
            .plugins
            .0
            .insert("sql".into(), serde_json::json!({"preload":[url]}));
        let app = mock_builder()
            .plugin(
                tauri_plugin_sql::Builder::default()
                    .add_migrations(&url, migrations())
                    .build(),
            )
            .build(context)
            .map_err(|e| e.to_string())?;
        MetricsDb::from_plugin(app.handle(), &url, Ok(vec![key; 32])).await
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rollup_preserves_recent_readings_and_is_atomic_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir.path().join("metrics.sqlite3"), 7).await.unwrap();
        let one = server();
        let two = server();
        let day = 24 * 60 * 60 * 1000;
        let now = day * 10 + 25_000;
        let boundary = day * 9;
        let minute = boundary - 60_000;
        for server in [&one, &two] {
            for i in 0..6 {
                let mut value = sample();
                value["cpu"] = serde_json::json!(i * 10);
                value["uptime"] = serde_json::json!(100 + i * 10);
                value["network"] =
                    serde_json::json!([{"name":"eth0","received":i*1000,"sent":i*500}]);
                db.record(server, minute + i * 10_000, &value)
                    .await
                    .unwrap();
                db.record(server, boundary + i * 10_000, &value)
                    .await
                    .unwrap();
            }
        }
        db.compact_history(now).await.unwrap();
        assert_eq!(db.cache_info().await.unwrap().samples, 14);
        let history = db.history_range(&one, now, 10080).await.unwrap();
        assert_eq!(history.len(), 7);
        assert_eq!(history[0].at, minute);
        assert_eq!(history[0].data["cpu"], 25.0);
        assert_eq!(history[0].data["peaks"]["cpu"], 50.0);
        assert_eq!(history[0].data["resolutionMs"], 60000);
        assert_eq!(history[0].data["sampleCount"], 6);
        assert_eq!(history[0].data["network"][0]["rx"], 100.0);
        assert_eq!(history[0].data["network"][0]["tx"], 50.0);
        assert_eq!(history[0].data["network"][0]["received"], 5000);
        assert_eq!(history[0].data["gap"], false);
        assert!(history[1].data.get("resolutionMs").is_none());
        let before = serde_json::to_value(&history).unwrap();
        db.compact_history(now).await.unwrap();
        // A stale collection cannot replace or reintroduce raw data in a compacted minute.
        db.record(&one, minute + 10_000, &sample()).await.unwrap();
        assert_eq!(
            serde_json::to_value(db.history_range(&one, now, 10080).await.unwrap()).unwrap(),
            before
        );
        assert_eq!(db.cache_info().await.unwrap().samples, 14);
        db.compact_history(now + RETENTION_MS + 60_000)
            .await
            .unwrap();
        assert_eq!(db.cache_info().await.unwrap().samples, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rollup_failure_keeps_raw_samples_and_endpoint_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir.path().join("metrics.sqlite3"), 7).await.unwrap();
        let s = server();
        let mut changed = s.clone();
        changed.ssh_host = "different-host".into();
        let minute = 2 * 24 * 60 * 60 * 1000;
        let now = minute + 2 * 24 * 60 * 60 * 1000;
        for i in 0..6 {
            db.record(&s, minute + i * 10_000, &sample()).await.unwrap();
            db.record(&changed, minute + i * 10_000, &sample())
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO samples(server_id,endpoint,at,data) VALUES('broken','broken',?1,'invalid')")
            .bind(minute+60_000).execute(db.pool().unwrap()).await.unwrap();
        assert!(db.compact_history(now).await.is_err());
        assert_eq!(db.cache_info().await.unwrap().samples, 13);
        sqlx::query("DELETE FROM samples WHERE server_id='broken'")
            .execute(db.pool().unwrap())
            .await
            .unwrap();
        db.compact_history(now).await.unwrap();
        assert_eq!(
            db.history_range(&s, now, 10080).await.unwrap()[0].data["sampleCount"],
            6
        );
        assert_eq!(
            db.history_range(&changed, now, 10080).await.unwrap()[0].data["sampleCount"],
            6
        );
        assert_eq!(db.cache_info().await.unwrap().samples, 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rollup_drains_multiple_batches_and_reclaims_disk_space() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir.path().join("metrics.sqlite3"), 7).await.unwrap();
        let s = server();
        let start = 2 * 24 * 60 * 60 * 1000;
        let mut value = sample();
        value["network"] = serde_json::json!([{"name":"x".repeat(16000),"received":0,"sent":0}]);
        for i in 0..300 {
            db.record(&s, start + i * 10_000, &value).await.unwrap();
        }
        let before = db.cache_info().await.unwrap();
        db.compact_history(start + 2 * 24 * 60 * 60 * 1000)
            .await
            .unwrap();
        let after = db.cache_info().await.unwrap();
        assert_eq!(after.samples, 50);
        assert!(after.bytes < before.bytes);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clearing_history_reclaims_space_and_allows_new_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.sqlite3");
        let db = open(&path, 7).await.unwrap();
        let one = server();
        let two = server();
        let at = now_ms();
        std::fs::write(dir.path().join("profiles.stronghold"), b"profile sentinel").unwrap();
        for server in [&one, &two] {
            let mut value = sample();
            value["network"] = serde_json::json!([{"name": "x".repeat(16000)}]);
            sqlx::query("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<128) INSERT INTO samples(server_id,endpoint,at,data) SELECT ?1,?2,?3-x,?4 FROM n")
                .bind(server.id.to_string()).bind(db.fingerprint(&endpoint(server)).unwrap())
                .bind(at).bind(serde_json::to_string(&value).unwrap())
                .execute(db.pool().unwrap()).await.unwrap();
        }
        let before = db.cache_info().await.unwrap();
        assert_eq!(before.samples, 256);
        let disk_bytes: u64 = [
            "metrics.sqlite3",
            "metrics.sqlite3-wal",
            "metrics.sqlite3-shm",
        ]
        .iter()
        .filter_map(|name| std::fs::metadata(dir.path().join(name)).ok())
        .map(|metadata| metadata.len())
        .sum();
        assert_eq!(before.bytes, disk_bytes);
        let cleared = db.clear_cache().await.unwrap();
        assert_eq!(cleared.samples, 0);
        assert!(cleared.bytes < before.bytes);
        assert!(db.history(&one, at).await.unwrap().is_empty());
        assert!(db.history(&two, at).await.unwrap().is_empty());
        assert_eq!(
            std::fs::read(dir.path().join("profiles.stronghold")).unwrap(),
            b"profile sentinel"
        );
        db.record(&one, at + 1, &sample()).await.unwrap();
        assert_eq!(db.history(&one, at + 1).await.unwrap().len(), 1);
        assert_eq!(db.cache_info().await.unwrap().samples, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plugin_reopen_retention_isolation_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics with # and %?.sqlite3");
        let s = server();
        let other = server();
        let now = RETENTION_MS + 100;
        let db = open(&path, 7).await.unwrap();
        db.record(&s, 99, &sample()).await.unwrap();
        db.record(&s, now - 10, &sample()).await.unwrap();
        db.record(&s, now, &sample()).await.unwrap();
        db.record(&other, now, &sample()).await.unwrap();
        db.pool().unwrap().close().await;
        let db = open(&path, 7).await.unwrap();
        let saved = db.history(&s, now).await.unwrap();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].at, now - 10);
        assert!(saved[0].data.get("processes").is_none());
        assert!(saved[0].data.get("password").is_none());
        let mut changed = s.clone();
        changed.ssh_host = "new-host".into();
        assert!(db.history(&changed, now).await.unwrap().is_empty());
        db.delete_server(s.id).await.unwrap();
        assert!(db.history(&s, now).await.unwrap().is_empty());
        assert_eq!(db.history(&other, now).await.unwrap().len(), 1);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let applied: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success=1")
                .fetch_one(db.pool().unwrap())
                .await
                .unwrap();
        assert_eq!(applied, 2);
        db.pool().unwrap().close().await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn legacy_migration_preserves_history_and_removes_readable_endpoints() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.sqlite3");
        let s = server();
        let now = now_ms();
        let old = SqlitePool::connect(&database_url(&path).unwrap())
            .await
            .unwrap();
        sqlx::raw_sql(SCHEMA).execute(&old).await.unwrap();
        sqlx::query("PRAGMA user_version=1")
            .execute(&old)
            .await
            .unwrap();
        sqlx::query("INSERT INTO samples VALUES(?1,?2,?3,?4)")
            .bind(s.id.to_string())
            .bind(endpoint(&s))
            .bind(now)
            .bind(sample().to_string())
            .execute(&old)
            .await
            .unwrap();
        old.close().await;
        let db = open(&path, 7).await.unwrap();
        assert_eq!(db.history(&s, now).await.unwrap().len(), 1);
        db.pool().unwrap().close().await;
        let other_key = open(&path, 8).await.unwrap();
        assert!(other_key.history(&s, now).await.unwrap().is_empty());
        other_key.pool().unwrap().close().await;
        let bytes = std::fs::read(path).unwrap();
        let plain = endpoint(&s);
        assert!(!bytes.windows(plain.len()).any(|w| w == plain.as_bytes()));
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn existing_v2_fingerprints_are_not_migrated_twice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.sqlite3");
        let s = server();
        let now = now_ms();
        let old = SqlitePool::connect(&database_url(&path).unwrap())
            .await
            .unwrap();
        let fingerprint = MetricsDb {
            pool: Ok(old.clone()),
            maintenance: Default::default(),
            key: Arc::new(Ok(zeroize::Zeroizing::new(vec![7; 32]))),
        }
        .fingerprint(&endpoint(&s))
        .unwrap();
        // Persisted endpoint IDs must survive crypto dependency upgrades.
        assert_eq!(
            fingerprint,
            "f8336c2ae6b02b95ce913b372236f9ad38d5530cb7eb5aa281e2b60e3e4254c2"
        );
        sqlx::raw_sql(SCHEMA).execute(&old).await.unwrap();
        sqlx::query("PRAGMA user_version=2")
            .execute(&old)
            .await
            .unwrap();
        sqlx::query("INSERT INTO samples VALUES(?1,?2,?3,?4)")
            .bind(s.id.to_string())
            .bind(&fingerprint)
            .bind(now)
            .bind(sample().to_string())
            .execute(&old)
            .await
            .unwrap();
        old.close().await;
        let db = open(&path, 7).await.unwrap();
        assert_eq!(db.history(&s, now).await.unwrap().len(), 1);
        let stored: String = sqlx::query_scalar("SELECT endpoint FROM samples")
            .fetch_one(db.pool().unwrap())
            .await
            .unwrap();
        assert_eq!(stored, fingerprint);
        db.pool().unwrap().close().await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unavailable_vault_does_not_require_a_database() {
        let app = mock_builder().build(mock_context(noop_assets())).unwrap();
        let db = MetricsDb::from_plugin(app.handle(), "", Err("Vault locked".into()))
            .await
            .unwrap();
        assert_eq!(
            db.history(&server(), now_ms()).await.unwrap_err(),
            "Vault locked"
        );
        assert!(db.delete_server(uuid::Uuid::new_v4()).await.is_err());
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn corrupt_database_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.sqlite3");
        std::fs::write(&path, b"broken data").unwrap();
        assert!(open(&path, 7).await.is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"broken data");
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn history_ranges_include_older_readings_only_when_requested() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir.path().join("metrics.sqlite3"), 7).await.unwrap();
        let s = server();
        let now = RETENTION_MS;
        for age in [6 * 86400000, 12 * 3600000, 30 * 60000, 60000] {
            db.record(&s, now - age, &sample()).await.unwrap();
        }
        for (minutes, count) in [(5, 1), (15, 1), (60, 2), (1440, 3), (10080, 4)] {
            assert_eq!(
                db.history_range(&s, now, minutes).await.unwrap().len(),
                count
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn history_is_bounded_ordered_and_pruned_on_reads() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir.path().join("metrics.sqlite3"), 7).await.unwrap();
        let s = server();
        for at in 0..200 {
            db.record(&s, at, &sample()).await.unwrap();
        }
        db.record(&s, 199, &serde_json::json!({"cpu":55,"network":[]}))
            .await
            .unwrap();
        let history = db.history(&s, 199).await.unwrap();
        assert_eq!(history.len(), 180);
        assert_eq!(history.first().unwrap().at, 20);
        assert_eq!(history.last().unwrap().data["cpu"], 55);
        assert_eq!(db.history(&s, 10).await.unwrap().len(), 11);
        assert!(db.history(&s, RETENTION_MS + 200).await.unwrap().is_empty());
        assert!(db
            .record(&s, 201, &serde_json::json!({"cpu":10}))
            .await
            .is_err());
        db.pool().unwrap().close().await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_samples_and_reads_use_the_same_pool() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir.path().join("metrics.sqlite3"), 7).await.unwrap();
        let s = server();
        let mut tasks = tokio::task::JoinSet::new();
        for at in 0..20 {
            let (db, s) = (db.clone(), s.clone());
            tasks.spawn(async move {
                db.record(&s, at, &sample()).await.unwrap();
                db.history(&s, 20).await.unwrap();
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
        assert_eq!(db.history(&s, 20).await.unwrap().len(), 20);
        db.pool().unwrap().close().await;
    }
}
