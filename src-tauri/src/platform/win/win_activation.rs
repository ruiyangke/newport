//! A profile-scoped wake signal; the file lock remains the instance authority.
use sha2::{Digest, Sha256};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::{
    Foundation::{WAIT_FAILED, WAIT_OBJECT_0},
    System::Threading::{CreateEventW, SetEvent, WaitForSingleObject},
};

pub struct Activation(OwnedHandle);
impl Activation {
    pub fn new(directory: &std::path::Path) -> std::io::Result<Self> {
        let path = directory.canonicalize()?;
        let hash = Sha256::digest(path.as_os_str().as_encoded_bytes());
        let suffix: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
        let name = super::wide(std::ffi::OsStr::new(&format!(
            "Local\\Porthop-activate-{suffix}"
        )));
        let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle) }))
    }
    pub fn request(&self) -> std::io::Result<()> {
        if unsafe { SetEvent(self.0.as_raw_handle()) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    fn take(&self) -> std::io::Result<bool> {
        match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_FAILED => Err(std::io::Error::last_os_error()),
            _ => Ok(false),
        }
    }
    pub async fn listen(self, app: tauri::AppHandle) {
        loop {
            match self.take() {
                Ok(true) => crate::app::show(&app),
                Ok(false) => {}
                Err(error) => {
                    log::warn!("Window activation failed: {error}");
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wake_is_shared_by_profile_and_retained_until_consumed() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let owner = Activation::new(a.path()).unwrap();
        let other = Activation::new(b.path()).unwrap();
        Activation::new(a.path()).unwrap().request().unwrap();
        assert!(!other.take().unwrap());
        assert!(owner.take().unwrap());
        assert!(!owner.take().unwrap());
    }
}
