//! Shell link objects and jump lists on a thread of their own, in a
//! single-threaded COM apartment that lives as long as the process; and the
//! registry's AppUserModelID entries.
use super::{Shortcut, Task};
use std::{
    mem::ManuallyDrop,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock, mpsc},
};
use windows::{
    Win32::{
        Storage::{
            EnhancedStorage::{PKEY_AppUserModel_ID, PKEY_Title},
            FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
        },
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile, STGM_READ,
                StructuredStorage::{
                    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
                    PropVariantToStringAlloc,
                },
            },
            Variant::{VT_EMPTY, VT_LPWSTR},
        },
        UI::Shell::{
            Common::{IObjectArray, IObjectCollection},
            DestinationList, EnumerableObjectCollection, FOLDERID_Programs, FOLDERID_UserPinned,
            ICustomDestinationList, IShellLinkW, KF_FLAG_DEFAULT,
            PropertiesSystem::IPropertyStore,
            SHGetKnownFolderPath, SHStrDupW, SetCurrentProcessExplicitAppUserModelID, ShellLink,
        },
    },
    core::{GUID, HSTRING, Interface, PWSTR, Result as WinResult},
};

/// Longest text a link field holds, NUL included.
const FIELD: usize = 32 * 1024;

type Job = Box<dyn FnOnce() + Send>;

/// Runs `work` on the apartment thread and waits for it.
fn run<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    static JOBS: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();
    let jobs = JOBS.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        let started = std::thread::Builder::new()
            .name("app-shortcuts".into())
            .spawn(move || {
                // SAFETY: initializes COM once for this thread, which never
                // leaves the apartment.
                let initialized = unsafe {
                    CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)
                };
                if let Err(error) = initialized.ok() {
                    log::warn!("Shortcuts unavailable: {error}");
                    return;
                }
                while let Ok(job) = receiver.recv() {
                    job();
                }
            });
        if let Err(error) = started {
            log::warn!("Shortcuts unavailable: {error}");
        }
        Mutex::new(sender)
    });
    let (reply, result) = mpsc::sync_channel(1);
    jobs.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .send(Box::new(move || {
            let _ = reply.send(work());
        }))
        .map_err(|_| "Shortcuts are unavailable".to_owned())?;
    result
        .recv()
        .map_err(|_| "Shortcuts are unavailable".to_owned())?
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    // SAFETY: on success the returned string is NUL-terminated, valid until the
    // CoTaskMemFree below, and freed exactly once.
    let path = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.ok()?;
    // SAFETY: `path` is a valid NUL-terminated string.
    let text = unsafe { path.to_string() }.ok();
    // SAFETY: `path` came from SHGetKnownFolderPath, which allocates with CoTaskMemAlloc.
    unsafe { CoTaskMemFree(Some(path.0.cast())) };
    text.map(PathBuf::from)
}

pub fn programs_folder() -> Option<PathBuf> {
    known_folder(&FOLDERID_Programs)
}

pub fn pinned_folder() -> Option<PathBuf> {
    known_folder(&FOLDERID_UserPinned)
}

pub fn set_process_app_id(app_id: &str) -> Result<(), String> {
    // SAFETY: a plain call with a NUL-terminated string.
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(app_id)) }
        .map_err(|error| format!("Cannot set Newport's taskbar ID: {error}"))
}

pub fn write(path: &Path, shortcut: &Shortcut) -> Result<(), String> {
    let (path, shortcut) = (path.to_owned(), shortcut.clone());
    run(move || {
        let folder = path.parent().ok_or("A shortcut needs a folder")?;
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("Cannot create {}: {error}", folder.display()))?;
        // Beside the shortcut, so the move replaces it in one step; not a
        // `.lnk`, so the Start menu never lists it.
        let temporary = folder.join(format!(
            ".{}.{}.tmp",
            path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        let written = save(&temporary, &shortcut).and_then(|()| {
            // SAFETY: both paths are NUL-terminated strings.
            unsafe {
                MoveFileExW(
                    &HSTRING::from(temporary.as_path()),
                    &HSTRING::from(path.as_path()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }
        });
        written.map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            format!("Cannot write {}: {error}", path.display())
        })
    })
}

fn link() -> WinResult<IShellLinkW> {
    // SAFETY: creates an in-process object on this apartment thread.
    unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
}

fn save(path: &Path, shortcut: &Shortcut) -> WinResult<()> {
    let link = link()?;
    // SAFETY: COM calls on an object this thread created, with
    // NUL-terminated strings that outlive each call.
    unsafe {
        link.SetPath(&HSTRING::from(shortcut.target.as_path()))?;
        link.SetArguments(&HSTRING::from(shortcut.arguments.as_str()))?;
        link.SetDescription(&HSTRING::from(shortcut.description.as_str()))?;
        if let Some((icon, index)) = &shortcut.icon {
            link.SetIconLocation(&HSTRING::from(icon.as_path()), *index)?;
        }
        let store: IPropertyStore = link.cast()?;
        let value = if shortcut.app_id.is_empty() {
            PROPVARIANT::default()
        } else {
            string_value(&shortcut.app_id)?
        };
        store.SetValue(&PKEY_AppUserModel_ID, &value)?;
        store.Commit()?;
        link.cast::<IPersistFile>()?
            .Save(&HSTRING::from(path), true)
    }
}

pub fn read(path: &Path) -> Result<Shortcut, String> {
    let path = path.to_owned();
    run(move || load(&path).map_err(|error| format!("Cannot read {}: {error}", path.display())))
}

fn load(path: &Path) -> WinResult<Shortcut> {
    let link = link()?;
    // SAFETY: as in `save`; each buffer outlives the call that fills it.
    unsafe {
        link.cast::<IPersistFile>()?
            .Load(&HSTRING::from(path), STGM_READ)?;
        let mut buffer = vec![0u16; FIELD];
        link.GetPath(&mut buffer, std::ptr::null_mut(), 0)?;
        let target = PathBuf::from(text(&buffer));
        link.GetArguments(&mut buffer)?;
        let arguments = text(&buffer);
        link.GetDescription(&mut buffer)?;
        let description = text(&buffer);
        let mut index = 0;
        link.GetIconLocation(&mut buffer, &mut index)?;
        let icon = text(&buffer);
        let icon = (!icon.is_empty()).then(|| (PathBuf::from(icon), index));
        let store: IPropertyStore = link.cast()?;
        let value = store.GetValue(&PKEY_AppUserModel_ID)?;
        let app_id = if value.vt() == VT_EMPTY {
            String::new()
        } else {
            let text = PropVariantToStringAlloc(&value)?;
            let app_id = text.to_string().unwrap_or_default();
            CoTaskMemFree(Some(text.0.cast()));
            app_id
        };
        Ok(Shortcut {
            target,
            arguments,
            icon,
            description,
            app_id,
        })
    }
}

pub fn write_jump_list(app_id: &str, tasks: &[Task]) -> Result<(), String> {
    let (app_id, tasks) = (app_id.to_owned(), tasks.to_vec());
    run(move || {
        jump_list(&app_id, &tasks)
            .map_err(|error| format!("Cannot write the jump list of {app_id}: {error}"))
    })
}

fn destinations() -> WinResult<ICustomDestinationList> {
    // SAFETY: creates an in-process object on this apartment thread.
    unsafe { CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER) }
}

fn jump_list(app_id: &str, tasks: &[Task]) -> WinResult<()> {
    let list = destinations()?;
    // SAFETY: COM calls on objects this thread created, with NUL-terminated
    // strings that outlive each call.
    unsafe {
        list.SetAppID(&HSTRING::from(app_id))?;
        let mut slots = 0;
        // What the person removed from the list; tasks can't be removed.
        let _removed: IObjectArray = list.BeginList(&mut slots)?;
        let written = (|| {
            let collection: IObjectCollection =
                CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
            for task in tasks {
                let link = link()?;
                link.SetPath(&HSTRING::from(task.target.as_path()))?;
                link.SetArguments(&HSTRING::from(task.arguments.as_str()))?;
                link.SetDescription(&HSTRING::from(task.title.as_str()))?;
                if let Some((icon, index)) = &task.icon {
                    link.SetIconLocation(&HSTRING::from(icon.as_path()), *index)?;
                }
                // A task shows its title, not its description.
                let store: IPropertyStore = link.cast()?;
                store.SetValue(&PKEY_Title, &string_value(&task.title)?)?;
                store.Commit()?;
                collection.AddObject(&link)?;
            }
            list.AddUserTasks(&collection.cast::<IObjectArray>()?)?;
            list.CommitList()
        })();
        if written.is_err() {
            let _ = list.AbortList();
        }
        written
    }
}

pub fn remove_jump_list(app_id: &str) -> Result<(), String> {
    let app_id = app_id.to_owned();
    run(move || {
        // SAFETY: as in `jump_list`.
        destinations()
            .and_then(|list| unsafe { list.DeleteList(&HSTRING::from(app_id.as_str())) })
            .map_err(|error| format!("Cannot remove the jump list of {app_id}: {error}"))
    })
}

/// `key\app_id`, refusing an ID that would name another key.
fn app_id_key(key: &str, app_id: &str) -> Result<String, String> {
    if app_id.is_empty() || app_id.contains(['\\', '/']) {
        return Err(format!("Not an AppUserModelID: {app_id}"));
    }
    Ok(format!(r"{key}\{app_id}"))
}

pub fn register_app_id(
    key: &str,
    app_id: &str,
    display_name: &str,
    icon: Option<&Path>,
) -> Result<(), String> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let path = app_id_key(key, app_id)?;
    let failed = |error: std::io::Error| format!("Cannot register {app_id}: {error}");
    let (entry, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(&path)
        .map_err(failed)?;
    entry
        .set_value("DisplayName", &display_name)
        .map_err(failed)?;
    match icon {
        Some(icon) => entry
            .set_value("IconUri", &icon.as_os_str())
            .map_err(failed),
        None => match entry.delete_value("IconUri") {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(failed(error)),
            _ => Ok(()),
        },
    }
}

pub fn unregister_app_id(key: &str, app_id: &str) -> Result<(), String> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let path = app_id_key(key, app_id)?;
    match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("Cannot unregister {app_id}: {error}"))
        }
        _ => Ok(()),
    }
}

pub fn registered_app_ids(key: &str) -> Vec<String> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(key)
        .map(|key| key.enum_keys().filter_map(Result::ok).collect())
        .unwrap_or_default()
}

fn text(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// A `VT_LPWSTR` value, the type the shell's AppUserModel properties take.
pub(crate) fn string_value(value: &str) -> WinResult<PROPVARIANT> {
    // SAFETY: the copy is owned by the PROPVARIANT, which frees it with
    // `PropVariantClear` when dropped.
    let text: PWSTR = unsafe { SHStrDupW(&HSTRING::from(value)) }?;
    Ok(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_LPWSTR,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 { pwszVal: text },
            }),
        },
    })
}
