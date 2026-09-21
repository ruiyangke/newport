//! Official desktop integrations. Domain state remains behind Rust commands.
use std::path::Path;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

pub fn logging(profile_directory: &Path) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    // Isolated profiles must not write into the running user's diagnostic log.
    let destination = if std::env::var_os("PORTHOP_DATA_DIR").is_some() {
        TargetKind::Folder {
            path: profile_directory.join("logs"),
            file_name: Some("porthop".into()),
        }
    } else {
        TargetKind::LogDir {
            file_name: Some("porthop".into()),
        }
    };
    tauri_plugin_log::Builder::new()
        // Only intentional app diagnostics: third-party transport logs can contain
        // server data. Never log credentials, remote output or clipboard contents.
        .level(log::LevelFilter::Off)
        .level_for("porthop", log::LevelFilter::Info)
        .targets([Target::new(TargetKind::Stderr), Target::new(destination)])
        .max_file_size(1_000_000)
        .rotation_strategy(RotationStrategy::KeepOne)
        .build()
}
