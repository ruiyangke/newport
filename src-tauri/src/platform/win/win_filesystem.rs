//! Private ACLs inherit from the protected profile directory. Replacements are durable.
use std::{
    fs::{self, OpenOptions},
    io,
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree},
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

pub fn application_data_directory() -> Option<std::path::PathBuf> {
    dirs::data_local_dir()
}

pub fn private_file_options() -> OpenOptions {
    OpenOptions::new()
}
pub fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    protect_directory(path)
}
pub fn protect_directory(path: &Path) -> io::Result<()> {
    protect(path, "OICI")
}
pub fn protect_file(path: &Path) -> io::Result<()> {
    protect(path, "")
}

fn protect(path: &Path, inherit: &str) -> io::Result<()> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut needed = 0;
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed);
        // A usize buffer supplies TOKEN_USER's pointer alignment.
        let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        );
        let error = io::Error::last_os_error();
        CloseHandle(token);
        if ok == 0 {
            return Err(error);
        }
        let user = &*(buffer.as_ptr().cast::<TOKEN_USER>());
        let mut sid = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut length = 0;
        while *sid.add(length) != 0 {
            length += 1;
        }
        let sid_text = String::from_utf16_lossy(std::slice::from_raw_parts(sid, length));
        LocalFree(sid.cast());
        let sddl = super::wide(std::ffi::OsStr::new(&format!(
            "D:P(A;{inherit};FA;;;{sid_text})(A;{inherit};FA;;;SY)"
        )));
        let mut descriptor = ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let result = SetFileSecurityW(
            super::wide(path.as_os_str()).as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        );
        let error = io::Error::last_os_error();
        LocalFree(descriptor);
        if result == 0 {
            return Err(error);
        }
    }
    Ok(())
}

pub fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    let result = unsafe {
        MoveFileExW(
            super::wide(source.as_os_str()).as_ptr(),
            super::wide(destination.as_os_str()).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
// Windows replacement is flushed by MOVEFILE_WRITE_THROUGH; directory handles
// cannot be synced with the Unix File::open(directory) approach.
pub fn sync_directory(_: &Path) -> io::Result<()> {
    Ok(())
}

pub fn database_url(path: &Path) -> Result<String, String> {
    let url = url::Url::from_file_path(path).map_err(|_| "Invalid metrics database path")?;
    // SQLx expects C:/... rather than /C:/...; retain UNC server names.
    let filename = match url.host_str() {
        Some(host) => format!(r"\\{host}{}", url.path().replace('/', r"\")),
        None => url.path().trim_start_matches('/').to_owned(),
    };
    Ok(format!("sqlite:{filename}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drive_and_unc_database_urls_preserve_absolute_paths() {
        assert_eq!(
            database_url(Path::new(r"C:\Users\test\metrics #1.db")).unwrap(),
            "sqlite:C:/Users/test/metrics%20%231.db"
        );
        assert_eq!(
            database_url(Path::new(r"\\server\share\metrics.db")).unwrap(),
            r"sqlite:\\server\share\metrics.db"
        );
    }
    #[test]
    fn sqlx_receives_the_original_network_share_path() {
        let path = Path::new(r"\\server\share\metrics #1.db");
        let options: sqlx::sqlite::SqliteConnectOptions =
            database_url(path).unwrap().parse().unwrap();
        assert_eq!(options.get_filename(), path);
    }
    #[test]
    fn protected_files_can_be_replaced_and_read() {
        let directory = tempfile::tempdir().unwrap();
        protect_directory(directory.path()).unwrap();
        let target = directory.path().join("vault");
        let staged = directory.path().join("staged");
        fs::write(&target, b"old").unwrap();
        fs::write(&staged, b"new").unwrap();
        protect_file(&staged).unwrap();
        replace_file(&staged, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!staged.exists());
    }
}
