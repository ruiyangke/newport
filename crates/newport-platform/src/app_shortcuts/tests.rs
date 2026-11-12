use super::*;

struct Folder(PathBuf);
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn folder() -> Folder {
    let path = std::env::temp_dir().join(format!("newport-shortcuts-{}", uuid::Uuid::new_v4()));
    Folder(path)
}

#[test]
fn a_shortcut_round_trips_and_rewrites_in_place() {
    let folder = folder();
    let path = folder.0.join("Newport Apps").join("Calculator (lab).lnk");
    let executable = std::env::current_exe().unwrap();
    let shortcut = Shortcut {
        target: executable.clone(),
        arguments: r#"--data-dir "C:\test profile\\" --remote-app Newport.Remote.a.b"#.into(),
        icon: Some((executable.clone(), 0)),
        description: "Calculator on lab".into(),
        app_id: "Newport.Remote.6fe02d2fc8ee3025544aa1e7.db207836d948804bf9711c8c".into(),
    };
    write(&path, &shortcut).unwrap();
    assert_eq!(read(&path).unwrap(), shortcut);
    assert_eq!(read_app_id(&path).unwrap(), shortcut.app_id);
    // Writing it again changes nothing and leaves no temporary file.
    write(&path, &shortcut).unwrap();
    assert_eq!(read(&path).unwrap(), shortcut);
    assert_eq!(list(path.parent().unwrap()), std::slice::from_ref(&path));
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
    set_app_id(&path, "Newport.App").unwrap();
    assert_eq!(
        read(&path).unwrap(),
        Shortcut {
            app_id: "Newport.App".into(),
            ..shortcut.clone()
        }
    );
    // No icon and no ID read back as none.
    let plain = Shortcut {
        icon: None,
        app_id: String::new(),
        ..shortcut
    };
    write(&path, &plain).unwrap();
    assert_eq!(read(&path).unwrap(), plain);
    remove(&path).unwrap();
    assert!(!path.exists());
    remove(&path).unwrap();
    assert!(read(&path).is_err());
}

#[test]
fn the_shell_folders_are_known() {
    let programs = programs_folder().unwrap();
    assert!(
        programs.ends_with(r"Start Menu\Programs"),
        "{}",
        programs.display()
    );
    let pinned = pinned_folder().unwrap();
    // Its TaskBar folder exists only once the account's taskbar was set up,
    // which a CI account's may not be; listing a missing one finds no pins.
    assert!(pinned.ends_with("User Pinned"), "{}", pinned.display());
}

/// The files the shell keeps custom jump lists in that hold `text`.
fn jump_lists_holding(text: &str) -> Vec<PathBuf> {
    let folder = dirs::data_dir()
        .unwrap()
        .join(r"Microsoft\Windows\Recent\CustomDestinations");
    let needle: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            std::fs::read(path).is_ok_and(|bytes| {
                bytes
                    .windows(needle.len())
                    .any(|window| window == needle.as_slice())
            })
        })
        .collect()
}

#[test]
fn a_jump_list_is_written_replaced_and_removed() {
    // An ID nothing shows, so the test touches no real app's list.
    let app_id = format!("Newport.Test.{}", uuid::Uuid::new_v4().simple());
    let marker = format!("--test-{}", uuid::Uuid::new_v4().simple());
    let executable = std::env::current_exe().unwrap();
    let task = |n: usize| Task {
        title: format!("Task {n}"),
        target: executable.clone(),
        arguments: format!("{marker} --action action-{n}"),
        icon: Some((executable.clone(), 0)),
    };
    write_jump_list(&app_id, &[task(1), task(2)]).unwrap();
    let written = jump_lists_holding(&format!("{marker} --action action-2"));
    assert_eq!(written.len(), 1, "{written:?}");
    write_jump_list(&app_id, &[task(3)]).unwrap();
    assert!(jump_lists_holding(&format!("{marker} --action action-2")).is_empty());
    assert_eq!(
        jump_lists_holding(&format!("{marker} --action action-3")),
        written
    );
    remove_jump_list(&app_id).unwrap();
    assert!(jump_lists_holding(&marker).is_empty());
    remove_jump_list(&app_id).unwrap();
    write_jump_list(&app_id, &[]).unwrap();
}

#[test]
fn an_app_id_registers_its_name_and_icon_in_the_key_given() {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    // Below an inert test subtree, never the user's own registrations.
    let key = format!(r"Software\NewportTests\{}", uuid::Uuid::new_v4());
    let app_id = "Newport.Remote.6fe02d2fc8ee3025544aa1e7.db207836d948804bf9711c8c";
    let icon = std::env::temp_dir().join("Calculator.png");
    assert!(registered_app_ids(&key).is_empty());
    register_app_id(&key, app_id, "Calculator", Some(&icon)).unwrap();
    let entry = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(format!(r"{key}\{app_id}"))
        .unwrap();
    assert_eq!(
        entry.get_value::<String, _>("DisplayName").unwrap(),
        "Calculator"
    );
    assert_eq!(
        entry.get_value::<String, _>("IconUri").unwrap(),
        icon.to_string_lossy()
    );
    register_app_id(&key, app_id, "Calculator (lab)", None).unwrap();
    assert!(entry.get_value::<String, _>("IconUri").is_err());
    assert_eq!(registered_app_ids(&key), [app_id]);
    assert!(register_app_id(&key, r"..\Other", "x", None).is_err());
    unregister_app_id(&key, app_id).unwrap();
    unregister_app_id(&key, app_id).unwrap();
    assert!(registered_app_ids(&key).is_empty());
    RegKey::predef(HKEY_CURRENT_USER)
        .delete_subkey_all(&key)
        .unwrap();
}
