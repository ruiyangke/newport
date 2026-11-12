//! Stage and hand off the shipping raw NSIS format. The host owns service drain
//! and exit; this module never exits the process or claims installation success.
use super::{
    Target, VerifiedUpdate,
    stale::{STAGING_PREFIX, remove_stale_staging},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

pub struct PreparedInstaller {
    directory: tempfile::TempDir,
    digest: [u8; 32],
    size: u64,
}
impl PreparedInstaller {
    pub fn prepare(update: &VerifiedUpdate) -> Result<Self, String> {
        let target = if cfg!(target_arch = "aarch64") {
            Target::WindowsArm64
        } else {
            Target::WindowsX64
        };
        if update.target() != target {
            return Err("The installer does not match this application's architecture.".into());
        }
        validate_package(update)?;
        let directory = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir()
            .map_err(|error| error.to_string())?;
        crate::filesystem::protect_directory(directory.path())
            .map_err(|error| error.to_string())?;
        let path = directory.path().join("newport-setup.exe");
        let mut file = crate::filesystem::private_file_options()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| error.to_string())?;
        file.write_all(update.bytes())
            .and_then(|_| file.sync_all())
            .map_err(|error| error.to_string())?;
        Ok(Self {
            directory,
            digest: Sha256::digest(update.bytes()).into(),
            size: update.bytes().len() as u64,
        })
    }

    pub fn path(&self) -> PathBuf {
        self.directory.path().join("newport-setup.exe")
    }

    /// Recheck immediately before handing the staged executable to the OS.
    /// The caller must retain this object until the installer has consumed it.
    pub fn revalidate(&self) -> Result<(), String> {
        verified_file(&self.path(), self.size, &self.digest).map(|_| ())
    }

    /// Call only after backend/profile shutdown. On successful OS handoff the
    /// private staging directory is retained: the installer still needs it after
    /// the app exits. A successful handoff is NOT proof that installation passed.
    /// On rejection/cancellation, Drop removes staging so the host can restart.
    pub fn launch(self) -> Result<PathBuf, String> {
        // A dedicated thread guarantees a clean STA for Shell extensions, rather
        // than inheriting a Tokio worker or GPUI thread's COM apartment.
        std::thread::Builder::new()
            .name("newport-installer".into())
            .spawn(move || self.launch_with(shell_execute))
            .map_err(|error| format!("Could not start installer handoff: {error}"))?
            .join()
            .map_err(|_| "Installer handoff panicked.".to_owned())?
    }

    fn launch_with(
        self,
        launch: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<PathBuf, String> {
        let directory = locked_directory(self.directory.path())?;
        let file = verified_file(&self.path(), self.size, &self.digest)?;
        launch(&self.path())?;
        // Keep the verified inode locked against modification/replacement until
        // ShellExecuteEx finishes creating the installer process.
        drop(file);
        drop(directory);
        Ok(self.directory.keep())
    }
}

/// Remove staging directories earlier handoffs left in this user's temporary
/// folder. Best effort: a directory that cannot go now goes on a later start.
pub fn remove_stale_installers() {
    let _ = remove_stale_staging(&std::env::temp_dir(), std::time::SystemTime::now());
}

fn locked_directory(path: &Path) -> Result<fs::File, String> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let directory = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| error.to_string())?;
    let metadata = directory.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("The installer staging directory changed.".into());
    }
    Ok(directory)
}

fn shell_execute(path: &Path) -> Result<(), String> {
    use windows_sys::{
        Win32::{
            Foundation::CloseHandle,
            System::Com::{
                COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
            },
            UI::{
                Shell::{
                    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS,
                    SHELLEXECUTEINFOW, ShellExecuteExW,
                },
                WindowsAndMessaging::SW_SHOW,
            },
        },
        w,
    };
    if !path.is_absolute() {
        return Err("Installer path must be absolute.".into());
    }
    let mut executable: Vec<u16> = path.as_os_str().encode_wide().collect();
    if executable.contains(&0) {
        return Err("Invalid installer path.".into());
    }
    executable.push(0);
    // SAFETY: called on a dedicated fresh thread, balanced by Apartment below.
    let initialized = unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    if initialized < 0 {
        return Err(format!(
            "Cannot initialize installer handoff (HRESULT {initialized:#x})."
        ));
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            // SAFETY: this guard stays on the thread that initialized COM.
            unsafe {
                CoUninitialize();
            }
        }
    }
    let _apartment = Apartment;
    let mut request = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("open"),
        lpFile: executable.as_ptr(),
        // Same passive-update/restart contract as the shipping NSIS updater.
        // No shell, inherited app arguments, or untrusted argument interpolation.
        lpParameters: w!("/P /UPDATE /R"),
        nShow: SW_SHOW,
        ..Default::default()
    };
    // SAFETY: structure and NUL-terminated strings remain alive for this
    // synchronous call. NOASYNC is required because the app exits after handoff.
    let launched = unsafe { ShellExecuteExW(&mut request) };
    let error = (launched == 0).then(std::io::Error::last_os_error);
    if !request.hProcess.is_null() {
        // SAFETY: NOCLOSEPROCESS transfers the returned handle to this caller.
        // Closing it does not terminate or wait for the installer.
        unsafe {
            CloseHandle(request.hProcess);
        }
    }
    match error {
        Some(error) => Err(format!(
            "Windows could not launch the update installer: {error}"
        )),
        None => Ok(()),
    }
}

fn validate_package(update: &VerifiedUpdate) -> Result<(), String> {
    let arch = match update.target() {
        Target::WindowsX64 => "x86_64",
        Target::WindowsArm64 => "aarch64",
        _ => return Err("Expected a Windows update package.".into()),
    };
    let expected = format!("Newport-{}-windows-{arch}-setup.exe", update.version());
    // The NSIS launcher itself may be x86 even for an ARM64 application. Its
    // machine field cannot identify the payload architecture. Bind the target
    // to the filename inside the already-authenticated Minisign comment instead.
    if update.artifact_name() != Some(expected.as_str()) {
        return Err("The signed artifact name does not match this Windows release target.".into());
    }
    let bytes = update.bytes();
    if bytes.len() < 64 || &bytes[..2] != b"MZ" {
        return Err("The update is not a Windows executable installer.".into());
    }
    let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
    let header = pe
        .checked_add(24)
        .filter(|end| *end <= bytes.len())
        .and_then(|end| bytes.get(pe..end))
        .ok_or("Invalid installer PE header.")?;
    if &header[..4] != b"PE\0\0"
        || !matches!(
            u16::from_le_bytes(header[4..6].try_into().unwrap()),
            0x014c | 0x8664 | 0xaa64
        )
    {
        return Err("Invalid installer executable format.".into());
    }
    Ok(())
}

fn verified_file(path: &Path, size: u64, digest: &[u8; 32]) -> Result<fs::File, String> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    };
    let mut file = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file()
        || metadata.len() != size
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err("The staged installer changed.".into());
    }
    let mut hash = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    let mut remaining = size;
    while remaining > 0 {
        let count = file
            .read(&mut bytes[..(remaining as usize).min(64 * 1024)])
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("The staged installer was truncated.".into());
        }
        hash.update(&bytes[..count]);
        remaining -= count as u64;
    }
    let actual: [u8; 32] = hash.finalize().into();
    if &actual != digest
        || file
            .read(&mut bytes[..1])
            .map_err(|error| error.to_string())?
            != 0
    {
        return Err("The staged installer changed after verification.".into());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn native_package() -> VerifiedUpdate {
        package(if cfg!(target_arch = "aarch64") {
            Target::WindowsArm64
        } else {
            Target::WindowsX64
        })
    }
    fn package(target: Target) -> VerifiedUpdate {
        let mut bytes = vec![0; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        bytes[68..70].copy_from_slice(&0x014cu16.to_le_bytes());
        let arch = if target == Target::WindowsArm64 {
            "aarch64"
        } else {
            "x86_64"
        };
        VerifiedUpdate {
            version: "2.0.0".parse().unwrap(),
            target,
            bytes,
            artifact_name: Some(format!("Newport-2.0.0-windows-{arch}-setup.exe")),
        }
    }
    #[test]
    fn x86_nsis_launchers_can_carry_both_architectures_but_signed_names_must_match() {
        for target in [Target::WindowsX64, Target::WindowsArm64] {
            let mut update = package(target);
            validate_package(&update).unwrap();
            update.artifact_name = None;
            assert!(validate_package(&update).is_err());
            update.artifact_name = Some("Newport-3.0.0-windows-x86_64-setup.exe".into());
            assert!(validate_package(&update).is_err());
        }
    }
    #[test]
    fn truncated_and_invalid_pe_headers_are_rejected() {
        let mut update = package(Target::WindowsX64);
        update.bytes[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(validate_package(&update).is_err());
        update.bytes.truncate(2);
        assert!(validate_package(&update).is_err());
    }
    #[test]
    fn staged_installer_is_private_revalidated_and_cleaned_on_drop() {
        let target = if cfg!(target_arch = "aarch64") {
            Target::WindowsArm64
        } else {
            Target::WindowsX64
        };
        let update = package(target);
        let prepared = PreparedInstaller::prepare(&update).unwrap();
        prepared.revalidate().unwrap();
        let path = prepared.path();
        let mut modified = update.bytes().to_vec();
        modified[120] = 1;
        fs::write(&path, modified).unwrap();
        assert!(prepared.revalidate().is_err());
        drop(prepared);
        assert!(!path.exists());
    }

    #[test]
    fn rejected_launch_cleans_staging_and_modified_payload_never_launches() {
        let prepared = PreparedInstaller::prepare(&native_package()).unwrap();
        let path = prepared.path();
        let error = prepared
            .launch_with(|_| Err("User cancelled".into()))
            .unwrap_err();
        assert_eq!(error, "User cancelled");
        assert!(!path.parent().unwrap().exists());

        let prepared = PreparedInstaller::prepare(&native_package()).unwrap();
        let path = prepared.path();
        fs::write(&path, "modified").unwrap();
        assert!(
            prepared
                .launch_with(|_| panic!("Changed installer was launched"))
                .is_err()
        );
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn handoff_locks_verified_bytes_and_retains_them_after_success() {
        let prepared = PreparedInstaller::prepare(&native_package()).unwrap();
        let path = prepared.path();
        let retained = prepared
            .launch_with(|path| {
                assert!(fs::OpenOptions::new().write(true).open(path).is_err());
                assert!(fs::rename(path, path.with_extension("moved")).is_err());
                assert!(fs::remove_file(path).is_err());
                let parent = path.parent().unwrap();
                assert!(fs::rename(parent, parent.with_extension("moved")).is_err());
                Ok(())
            })
            .unwrap();
        assert!(path.exists());
        assert_eq!(retained, path.parent().unwrap());
        assert_eq!(fs::read(path).unwrap(), native_package().bytes());
        // The fake launcher has no child; this fixture can safely clean up.
        fs::remove_dir_all(retained).unwrap();
    }
}
