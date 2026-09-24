//! Verified downloads stay in memory until the user explicitly chooses to restart.
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::Duration;
use tauri::{Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub current_version: String,
    pub phase: String,
    pub version: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
}
pub struct Updates {
    status: Mutex<Status>,
    pending: tokio::sync::Mutex<Option<(Update, Vec<u8>)>>,
    pub restart: AtomicBool,
    #[cfg(target_os = "windows")]
    services_stopped: AtomicBool,
}
impl Updates {
    pub fn new(version: String) -> Self {
        Self {
            status: Mutex::new(Status {
                enabled: !cfg!(debug_assertions)
                    && cfg!(any(target_os = "macos", target_os = "windows"))
                    && option_env!("PORTHOP_APP_STORE").is_none()
                    && std::env::var_os("PORTHOP_DATA_DIR").is_none(),
                current_version: version,
                phase: "idle".into(),
                version: None,
                downloaded: 0,
                total: None,
                error: None,
            }),
            pending: Default::default(),
            restart: AtomicBool::new(false),
            #[cfg(target_os = "windows")]
            services_stopped: AtomicBool::new(false),
        }
    }
    fn change(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap());
    }
}
#[tauri::command]
pub fn update_status(state: State<'_, Updates>) -> Status {
    state.status.lock().unwrap().clone()
}

async fn check(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<Updates>();
    let mut pending = state
        .pending
        .try_lock()
        .map_err(|_| "An update is already in progress")?;
    if !state.status.lock().unwrap().enabled {
        return Err("Updates are available in the installed app.".into());
    }
    if pending.is_some() {
        return Ok(());
    }
    state.change(|s| {
        s.phase = "checking".into();
        s.error = None;
        s.version = None;
        s.downloaded = 0;
        s.total = None;
    });
    let result = async {
        let builder = app.updater_builder();
        #[cfg(target_os = "windows")]
        let builder = {
            let app = app.clone();
            builder.on_before_exit(move || {
                app.state::<Updates>()
                    .services_stopped
                    .store(true, Ordering::SeqCst);
                tauri::async_runtime::block_on(crate::app::stop_services(&app));
            })
        };
        let updater = builder
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;
        match updater.check().await.map_err(|e| e.to_string())? {
            None => state.change(|s| s.phase = "current".into()),
            Some(mut update) => {
                // Download deadlines are longer than the small manifest request.
                update.timeout = Some(Duration::from_secs(600));
                state.change(|s| {
                    s.phase = "downloading".into();
                    s.version = Some(update.version.clone());
                });
                let bytes = update
                    .download(
                        |size, total| {
                            state.change(|s| {
                                s.downloaded += size as u64;
                                s.total = total;
                            })
                        },
                        || {},
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                *pending = Some((update, bytes));
                state.change(|s| s.phase = "ready".into());
            }
        }
        Ok::<(), String>(())
    }
    .await;
    if let Err(ref error) = result {
        log::warn!("Update check/download failed: {error}");
        state.change(|s| {
            s.phase = "error".into();
            s.error = Some("Could not check or download the update. Try again.".into());
        });
    }
    result
}
#[tauri::command]
pub async fn check_for_updates(app: tauri::AppHandle) -> Result<(), String> {
    check(&app)
        .await
        .map_err(|_| "Could not check or download the update. Try again.".into())
}
#[tauri::command]
pub async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<Updates>();
    let mut pending = state
        .pending
        .try_lock()
        .map_err(|_| "An update is already in progress")?;
    let (update, bytes) = pending.as_ref().ok_or("No downloaded update is ready")?;
    state.change(|s| {
        s.phase = "installing".into();
        s.error = None;
    });
    let update = update.clone();
    let bytes = bytes.clone();
    let result = tokio::task::spawn_blocking(move || update.install(bytes)).await;
    if !matches!(result, Ok(Ok(()))) {
        state.change(|s| {
            s.phase = "ready".into();
            s.error = Some("Could not install the update. Try again.".into());
        });
        #[cfg(target_os = "windows")]
        if state.services_stopped.load(Ordering::SeqCst) {
            // A failed installer launch may follow the shutdown hook. Restart
            // the existing app so the user is not left with stopped workers.
            state.restart.store(true, Ordering::SeqCst);
            app.exit(0);
        }
        return Err("Could not install the update. Try again.".into());
    }
    *pending = None;
    state.restart.store(true, Ordering::SeqCst);
    // A normal exit is preventable, unlike Tauri's immediate restart request.
    // app.rs first closes workers, terminals, transfers and SSH connections.
    app.exit(0);
    Ok(())
}
pub async fn background(app: tauri::AppHandle) {
    if !app.state::<Updates>().status.lock().unwrap().enabled {
        return;
    }
    tokio::time::sleep(Duration::from_secs(30)).await;
    loop {
        let _ = check(&app).await;
        tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::test::{mock_builder, mock_context, noop_assets};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn download_fixture(
        target: &str,
        version: &str,
        tamper: bool,
    ) -> Result<Vec<u8>, String> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let endpoint = format!("http://{address}/latest.json");
        let manifest = serde_json::json!({
            "version": version,
            "platforms": {(target): {
                "url": format!("http://{address}/payload"),
                "signature": include_str!("../tests/fixtures/updater/payload.txt.sig").trim(),
            }},
        })
        .to_string();
        let payload = if tamper {
            b"modified".to_vec()
        } else {
            include_bytes!("../tests/fixtures/updater/payload.txt").to_vec()
        };
        let server = tokio::spawn(async move {
            for body in [manifest.into_bytes(), payload] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0; 4096];
                let mut received = Vec::new();
                while !received.windows(4).any(|part| part == b"\r\n\r\n") {
                    let count = socket.read(&mut request).await.unwrap();
                    assert!(count > 0 && received.len() < 65536);
                    received.extend_from_slice(&request[..count]);
                }
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(&body).await.unwrap();
            }
        });
        let mut context = mock_context(noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({
                "pubkey": include_str!("../tests/fixtures/updater/public.key").trim(),
                "requireSignedVersion": true,
                // Local synthetic HTTP server only; production endpoints remain HTTPS.
                "dangerousInsecureTransportProtocol": true,
                "endpoints": [endpoint],
            }),
        );
        let app = mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        let updater = app
            .updater_builder()
            .executable_path(std::path::PathBuf::from(
                "/tmp/Porthop.app/Contents/MacOS/porthop",
            ))
            .target(target)
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let update = updater.check().await.unwrap().unwrap();
        let result = update
            .download(|_, _| {}, || {})
            .await
            .map_err(|e| e.to_string());
        server.await.unwrap();
        result
    }
    #[tokio::test]
    async fn accepts_signed_update_bytes() {
        assert_eq!(
            download_fixture("darwin-aarch64", "2.0.0", false)
                .await
                .unwrap(),
            include_bytes!("../tests/fixtures/updater/payload.txt")
        );
    }
    #[tokio::test]
    async fn rejects_modified_update_bytes() {
        assert!(download_fixture("darwin-aarch64", "2.0.0", true)
            .await
            .is_err());
    }
    #[tokio::test]
    async fn rejects_manifest_version_not_bound_to_signature() {
        assert!(download_fixture("darwin-aarch64", "3.0.0", false)
            .await
            .is_err());
    }
    #[tokio::test]
    async fn windows_architectures_verify_signed_updates() {
        for target in ["windows-x86_64", "windows-aarch64"] {
            assert!(download_fixture(target, "2.0.0", false).await.is_ok());
            assert!(download_fixture(target, "2.0.0", true).await.is_err());
            assert!(download_fixture(target, "3.0.0", false).await.is_err());
        }
    }
    #[test]
    fn production_requires_https_and_version_bound_signatures() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let updater = &config["plugins"]["updater"];
        assert_eq!(updater["requireSignedVersion"], true);
        for endpoint in updater["endpoints"].as_array().unwrap() {
            assert!(endpoint.as_str().unwrap().starts_with("https://"));
        }
        assert!(updater.get("dangerousInsecureTransportProtocol").is_none());
    }
}
