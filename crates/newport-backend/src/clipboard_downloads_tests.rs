use super::*;
use crate::clipboard_files::tests::{Disk, mtime, set_mtime};
use crate::clipboard_files::{Item, Stat};
use std::os::unix::fs::PermissionsExt;
use tokio::sync::Semaphore;

/// A server tree under the fake's home: a folder with a nested folder and an
/// in-tree link, and a Unicode-named executable file.
fn copied(disk: &Disk) -> Vec<String> {
    let base = disk.home.path().join("project");
    let docs = base.join("docs");
    std::fs::create_dir_all(docs.join("nested")).unwrap();
    std::fs::write(docs.join("nested/deep.txt"), b"deep").unwrap();
    std::os::unix::fs::symlink("nested/deep.txt", docs.join("link")).unwrap();
    let named = base.join("名前 1.txt");
    std::fs::write(&named, vec![7u8; 100_000]).unwrap();
    std::fs::set_permissions(&named, std::fs::Permissions::from_mode(0o775)).unwrap();
    // Folders last, children first, as their times change with their contents.
    for (path, mode, seconds) in [
        (docs.join("nested/deep.txt"), 0o644, 1_600_000_000),
        (named.clone(), 0o775, 1_500_000_000),
        (docs.join("nested"), 0o755, 1_400_000_000),
        (docs.clone(), 0o750, 1_300_000_000),
    ] {
        set_mtime(&path, seconds);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    vec![path(&docs), path(&named)]
}
fn path(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}
/// Names in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}
async fn until(condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("condition not reached");
}
fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}
fn plan_of(bytes: u64, count: u64, inside_home: bool) -> Plan {
    Plan {
        entries: Vec::new(),
        bytes,
        count,
        inside_home,
        hidden: false,
        executable: false,
        installer: false,
    }
}
async fn refused(disk: &Disk, paths: &[String]) -> String {
    format!("{:#}", plan(disk, paths).await.unwrap_err())
}

#[test]
fn the_path_list_is_strict() {
    let list = |value: serde_json::Value| serde_json::to_vec(&value).unwrap();
    assert_eq!(
        parse_paths(&list(
            serde_json::json!({"version": 1, "paths": ["/home/u/a b"]})
        ))
        .unwrap(),
        ["/home/u/a b"]
    );
    for invalid in [
        serde_json::json!({"version": 1, "paths": ["/a"], "extra": true}),
        serde_json::json!({"version": 2, "paths": ["/a"]}),
        serde_json::json!({"version": 1}),
        serde_json::json!({"version": 1, "paths": []}),
        serde_json::json!({"version": 1, "paths": ["relative/a"]}),
        serde_json::json!({"version": 1, "paths": ["/a\u{0}b"]}),
        serde_json::json!({"version": 1, "paths": vec!["/a"; 10_001]}),
    ] {
        assert!(parse_paths(&list(invalid.clone())).is_err(), "{invalid}");
    }
    assert!(
        parse_paths(&list(
            serde_json::json!({"version": 1, "paths": vec!["/a"; 10_000]})
        ))
        .is_ok()
    );
    let long = format!("/{}", "a".repeat(MAX_LIST));
    assert!(parse_paths(&list(serde_json::json!({"version": 1, "paths": [long]}))).is_err());
    assert!(parse_paths(b"not json").is_err());
}

#[test]
fn small_copies_inside_home_download_at_once_and_large_ones_ask_or_are_refused() {
    assert_eq!(decide(&plan_of(512 << 20, 10_000, true)), Decision::Auto);
    assert_eq!(decide(&plan_of((512 << 20) + 1, 1, true)), Decision::Ask);
    assert_eq!(decide(&plan_of(1, 1, false)), Decision::Ask);
    assert_eq!(decide(&plan_of(2 << 30, 10_000, false)), Decision::Ask);
    assert_eq!(decide(&plan_of((2 << 30) + 1, 1, true)), Decision::Refuse);
    assert_eq!(decide(&plan_of(0, 10_001, true)), Decision::Refuse);
    let mut sensitive = plan_of(1, 1, true);
    sensitive.hidden = true;
    assert_eq!(decide(&sensitive), Decision::Ask);
}

/// The decision for the items `make` creates under a fresh home.
async fn decision(make: impl FnOnce(&Path) -> Vec<PathBuf>) -> (Decision, bool) {
    let disk = Disk::new(false);
    let paths: Vec<String> = make(disk.home.path()).iter().map(|p| path(p)).collect();
    let plan = plan(&*disk, &paths).await.unwrap();
    (decide(&plan), plan.sensitive())
}

#[tokio::test]
async fn the_question_says_why_it_asks_and_names_the_first_items() {
    use std::fs::{create_dir_all, set_permissions, write};
    let disk = Disk::new(false);
    let home = disk.home.path();
    let long = "n".repeat(60);
    write(home.join(".env"), b"x").unwrap();
    write(home.join("tool"), b"x").unwrap();
    set_permissions(home.join("tool"), std::fs::Permissions::from_mode(0o700)).unwrap();
    create_dir_all(home.join("Editor.app")).unwrap();
    write(home.join(&long), b"x").unwrap();
    write(home.join("fifth"), b"x").unwrap();
    let paths: Vec<String> = [".env", "tool", "Editor.app", long.as_str(), "fifth"]
        .iter()
        .map(|name| path(&home.join(name)))
        .collect();
    let plan = plan(&*disk, &paths).await.unwrap();
    assert!(plan.hidden() && plan.executable() && plan.installer() && !plan.large());
    assert_eq!(plan.top_level_count(), 5);
    assert_eq!(plan.shown_names(), [".env", "tool", "Editor.app"]);
    let plan = super::plan(&*disk, &[path(&home.join(&long))])
        .await
        .unwrap();
    assert_eq!(plan.shown_names(), [format!("{}…", "n".repeat(40))]);
    assert!(!plan.hidden() && !plan.executable() && !plan.installer());
    assert!(plan_of((512 << 20) + 1, 1, true).large());
}

#[tokio::test]
async fn hidden_executable_and_installable_items_ask_instead_of_downloading_at_once() {
    use std::fs::{create_dir_all, set_permissions, write};
    let plain = |home: &Path| {
        write(home.join("notes.txt"), b"x").unwrap();
        vec![home.join("notes.txt")]
    };
    assert_eq!(decision(plain).await, (Decision::Auto, false));

    // A hidden top-level item.
    let (decision_, _) = decision(|home| {
        write(home.join(".env"), b"x").unwrap();
        vec![home.join(".env")]
    })
    .await;
    assert_eq!(decision_, Decision::Ask);
    // A hidden folder holding the item, and a hidden folder inside the copy.
    let (decision_, _) = decision(|home| {
        create_dir_all(home.join(".ssh")).unwrap();
        write(home.join(".ssh/id_ed25519.pub"), b"x").unwrap();
        vec![home.join(".ssh/id_ed25519.pub")]
    })
    .await;
    assert_eq!(decision_, Decision::Ask);
    let (decision_, _) = decision(|home| {
        create_dir_all(home.join("repo/.git")).unwrap();
        write(home.join("repo/.git/config"), b"x").unwrap();
        vec![home.join("repo")]
    })
    .await;
    assert_eq!(decision_, Decision::Ask);
    // A visible link to a hidden folder: the resolved path is what counts.
    let (decision_, _) = decision(|home| {
        create_dir_all(home.join(".aws")).unwrap();
        std::os::unix::fs::symlink(".aws", home.join("cloud")).unwrap();
        vec![home.join("cloud")]
    })
    .await;
    assert_eq!(decision_, Decision::Ask);
    // An executable file, top level or nested.
    let (decision_, _) = decision(|home| {
        write(home.join("tool"), b"x").unwrap();
        set_permissions(home.join("tool"), std::fs::Permissions::from_mode(0o700)).unwrap();
        vec![home.join("tool")]
    })
    .await;
    assert_eq!(decision_, Decision::Ask);
    let (decision_, _) = decision(|home| {
        create_dir_all(home.join("dir")).unwrap();
        write(home.join("dir/run"), b"x").unwrap();
        set_permissions(home.join("dir/run"), std::fs::Permissions::from_mode(0o601)).unwrap();
        vec![home.join("dir")]
    })
    .await;
    assert_eq!(decision_, Decision::Ask);
    // Applications, installers and scripts by name, whatever the case.
    for name in ["Tool.app", "a.pkg", "b.dmg", "c.command", "d.sh", "E.SH"] {
        let (decision_, _) = decision(|home| {
            write(home.join(name), b"x").unwrap();
            vec![home.join(name)]
        })
        .await;
        assert_eq!(decision_, Decision::Ask, "{name}");
    }
    let (decision_, _) = decision(|home| {
        create_dir_all(home.join("Tool.app/Contents")).unwrap();
        vec![home.join("Tool.app")]
    })
    .await;
    assert_eq!(decision_, Decision::Ask);
    // Names that only look similar are not.
    let (decision_, _) = decision(|home| {
        write(home.join("sh"), b"x").unwrap();
        write(home.join("app.txt"), b"x").unwrap();
        vec![home.join("sh"), home.join("app.txt")]
    })
    .await;
    assert_eq!(decision_, Decision::Auto);
}

#[tokio::test]
async fn a_tree_is_planned_parents_first_and_downloaded_into_place() {
    let disk = Disk::new(false);
    let paths = copied(&disk);
    let plan = plan(&*disk, &paths).await.unwrap();
    let summary: Vec<(&str, &EntryKind)> = plan
        .entries()
        .iter()
        .map(|entry| (entry.path.to_str().unwrap(), &entry.kind))
        .collect();
    let deep = EntryKind::File {
        size: 4,
        mtime: Some(
            std::fs::metadata(disk.home.path().join("project/docs/nested/deep.txt"))
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs() as u32,
        ),
    };
    assert_eq!(summary.len(), 5);
    assert_eq!(summary[0], ("docs", &EntryKind::Directory));
    assert_eq!(
        summary[1],
        (
            "docs/link",
            &EntryKind::Symlink {
                target: "nested/deep.txt".into()
            }
        )
    );
    assert_eq!(summary[2], ("docs/nested", &EntryKind::Directory));
    assert_eq!(summary[3], ("docs/nested/deep.txt", &deep));
    assert_eq!(summary[4].0, "名前 1.txt");
    assert_eq!(plan.top_level().collect::<Vec<_>>(), ["docs", "名前 1.txt"]);
    assert_eq!((plan.bytes(), plan.count()), (100_004, 5));
    assert!(plan.inside_home());
    // The executable file makes it ask.
    assert_eq!(decide(&plan), Decision::Ask);

    let cache = tempfile::tempdir().unwrap();
    let root = cache.path().join("Newport/Clipboard");
    let folder = fetch(&*disk, &plan, &root, || true).await.unwrap();
    let id = uuid::Uuid::try_parse(folder.file_name().unwrap().to_str().unwrap()).unwrap();
    assert_eq!(folder, root.join(id.to_string()));
    assert_eq!(names(&root), [id.to_string()]);
    assert_eq!(
        std::fs::read(folder.join("docs/nested/deep.txt")).unwrap(),
        b"deep"
    );
    assert_eq!(
        std::fs::read_link(folder.join("docs/link")).unwrap(),
        Path::new("nested/deep.txt")
    );
    assert_eq!(std::fs::read(folder.join("docs/link")).unwrap(), b"deep");
    let named = folder.join("名前 1.txt");
    assert_eq!(std::fs::read(&named).unwrap(), vec![7u8; 100_000]);
    // The server's group and other write bits are kept (the sticky and set-id
    // bits are not).
    assert_eq!(mode(&named), 0o775);
    // The cache folders are private; copied folders keep their own mode.
    for dir in [&root, &folder] {
        assert_eq!(mode(dir), 0o700, "{dir:?}");
    }
    assert_eq!(mode(&folder.join("docs")), 0o750);
    assert_eq!(mode(&folder.join("docs/nested")), 0o755);
    assert_eq!(mode(&folder.join("docs/nested/deep.txt")), 0o644);
    assert_eq!(mtime(&named), 1_500_000_000);
    assert_eq!(mtime(&folder.join("docs/nested/deep.txt")), 1_600_000_000);
    assert_eq!(mtime(&folder.join("docs/nested")), 1_400_000_000);
    assert_eq!(mtime(&folder.join("docs")), 1_300_000_000);
    // A link carries no times of its own to keep.
    assert!(folder.join("docs/link").is_symlink());
}

#[tokio::test]
async fn read_only_folders_keep_their_modes_and_are_still_removed() {
    let disk = Disk::new(false);
    let locked = disk.home.path().join("project/locked");
    std::fs::create_dir_all(locked.join("inner")).unwrap();
    std::fs::write(locked.join("inner/ro.txt"), b"ro").unwrap();
    for (path, mode, seconds) in [
        (locked.join("inner/ro.txt"), 0o444, 1_000_000_001),
        (locked.join("inner"), 0o555, 1_000_000_002),
        (locked.clone(), 0o500, 1_000_000_003),
    ] {
        set_mtime(&path, seconds);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    let planned = plan(&*disk, &[path(&locked)]).await;
    let cache = tempfile::tempdir().unwrap();
    let folder = match &planned {
        Ok(planned) => fetch(&*disk, planned, cache.path(), || true).await,
        Err(_) => unreachable!(),
    };
    // Restore the server tree first so a failure cannot leave it undeletable.
    for path in [&locked, &locked.join("inner")] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let folder = folder.unwrap();
    assert_eq!(mode(&folder.join("locked")), 0o500);
    assert_eq!(mode(&folder.join("locked/inner")), 0o555);
    assert_eq!(mode(&folder.join("locked/inner/ro.txt")), 0o444);
    assert_eq!(mtime(&folder.join("locked/inner/ro.txt")), 1_000_000_001);
    assert_eq!(mtime(&folder.join("locked/inner")), 1_000_000_002);
    assert_eq!(mtime(&folder.join("locked")), 1_000_000_003);
    remove(&folder);
    assert!(!folder.exists());
}

#[tokio::test]
async fn a_linked_top_level_folder_is_followed_and_outside_home_asks() {
    let disk = Disk::new(false);
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(outside.path().join("data")).unwrap();
    std::fs::write(outside.path().join("data/file"), b"x").unwrap();
    let link = disk.home.path().join("data-link");
    std::os::unix::fs::symlink(outside.path().join("data"), &link).unwrap();
    let plan = plan(&*disk, &[path(&link)]).await.unwrap();
    assert_eq!(plan.top_level().collect::<Vec<_>>(), ["data-link"]);
    assert_eq!(plan.entries()[1].path, Path::new("data-link/file"));
    assert!(!plan.inside_home());
    assert_eq!(decide(&plan), Decision::Ask);
    // The home folder itself is not inside it.
    let other = Disk::new(false);
    std::fs::write(other.home.path().join("file"), b"x").unwrap();
    let home = super::plan(&*other, &[path(other.home.path())])
        .await
        .unwrap();
    assert!(!home.inside_home());
}

#[tokio::test]
async fn links_leaving_the_tree_are_refused() {
    let disk = Disk::new(false);
    std::fs::write(disk.home.path().join("outside.txt"), b"x").unwrap();
    let cases = [
        ("absolute", vec![("link", "/etc/hosts")]),
        ("parent", vec![("link", "../outside.txt")]),
        ("dangling", vec![("link", "missing")]),
        // Lexically inside, but `here` resolves to the tree itself first.
        (
            "resolved",
            vec![("here", "."), ("link", "here/../outside.txt")],
        ),
    ];
    for (name, links) in cases {
        let tree = disk.home.path().join(name);
        std::fs::create_dir(&tree).unwrap();
        for (link, target) in links {
            std::os::unix::fs::symlink(target, tree.join(link)).unwrap();
        }
        let error = refused(&disk, &[path(&tree)]).await;
        assert!(
            error.contains(&format!("“{name}/link” links outside")),
            "{name}: {error}"
        );
    }
}

#[tokio::test]
async fn names_a_mac_folder_cannot_hold_side_by_side_are_refused_by_name() {
    let disk = Disk::new(false);
    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("Report.txt"), b"x").unwrap();
    std::fs::write(disk.home.path().join("report.TXT"), b"x").unwrap();
    std::fs::write(other.path().join("caf\u{e9}"), b"x").unwrap();
    std::fs::write(disk.home.path().join("cafe\u{301}"), b"x").unwrap();
    let cased = [
        path(&other.path().join("Report.txt")),
        path(&disk.home.path().join("report.TXT")),
    ];
    assert!(
        refused(&disk, &cased)
            .await
            .contains("“Report.txt” and “report.TXT” would have the same name on this Mac")
    );
    let composed = [
        path(&other.path().join("caf\u{e9}")),
        path(&disk.home.path().join("cafe\u{301}")),
    ];
    assert!(
        refused(&disk, &composed)
            .await
            .contains("would have the same name")
    );
    // A Mac volume cannot hold such a pair in one folder, so the folder check
    // is exercised directly; different folders may reuse a name.
    let mut folder = Folder::default();
    folder.add(Path::new("tree/sub/A.txt"), "A.txt").unwrap();
    folder.add(Path::new("tree/sub/b.txt"), "b.txt").unwrap();
    let error = folder
        .add(Path::new("tree/sub/a.TXT"), "a.TXT")
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "“tree/sub/A.txt” and “tree/sub/a.TXT” would have the same name on this Mac"
    );
    Folder::default()
        .add(Path::new("tree/a.txt"), "a.txt")
        .unwrap();
}

#[tokio::test]
async fn devices_sockets_and_pipes_bad_names_and_duplicates_are_refused_by_name() {
    let disk = Disk::new(false);
    let tree = disk.home.path().join("tree");
    std::fs::create_dir(&tree).unwrap();
    let fifo = std::ffi::CString::new(path(&tree.join("pipe"))).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(
        refused(&disk, &[path(&tree)])
            .await
            .contains("“tree/pipe” is a device, socket or pipe")
    );

    let bad = disk.home.path().join("bad");
    std::fs::create_dir(&bad).unwrap();
    std::fs::write(bad.join("line\nbreak"), b"x").unwrap();
    assert!(
        refused(&disk, &[path(&bad)])
            .await
            .contains("“bad/line\nbreak” has a name")
    );

    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("same"), b"x").unwrap();
    std::fs::write(disk.home.path().join("same"), b"x").unwrap();
    let both = [
        path(&other.path().join("same")),
        path(&disk.home.path().join("same")),
    ];
    assert!(
        refused(&disk, &both)
            .await
            .contains("Two copied items are named “same”")
    );
    assert!(
        refused(&disk, &[format!("{}/", path(&tree))])
            .await
            .contains("has a name")
    );
    assert!(
        refused(&disk, &[path(&disk.home.path().join("gone"))])
            .await
            .contains("not available")
    );
}

#[tokio::test]
async fn planning_stops_beyond_the_limits_and_such_copies_are_never_fetched() {
    let disk = Disk::new(false);
    let large = disk.home.path().join("large.bin");
    std::fs::File::create(&large)
        .unwrap()
        .set_len((2 << 30) + 1)
        .unwrap();
    let small = disk.home.path().join("small.txt");
    std::fs::write(&small, b"x").unwrap();
    let plan = plan(&*disk, &[path(&large), path(&small)]).await.unwrap();
    assert_eq!(plan.count(), 1);
    assert_eq!(decide(&plan), Decision::Refuse);
    let cache = tempfile::tempdir().unwrap();
    assert!(fetch(&*disk, &plan, cache.path(), || true).await.is_err());
    assert!(names(cache.path()).is_empty());

    let many = disk.home.path().join("many");
    std::fs::create_dir(&many).unwrap();
    for index in 0..10_000 {
        std::fs::write(many.join(index.to_string()), b"").unwrap();
    }
    let plan = super::plan(&*disk, &[path(&many)]).await.unwrap();
    assert_eq!(plan.count(), 10_001);
    assert_eq!(decide(&plan), Decision::Refuse);
    std::fs::remove_file(many.join("0")).unwrap();
    let plan = super::plan(&*disk, &[path(&many)]).await.unwrap();
    assert_eq!(plan.count(), 10_000);
    assert_eq!(decide(&plan), Decision::Auto);
}

#[tokio::test]
async fn a_file_changing_during_the_copy_fails_and_removes_the_partial() {
    let disk = Disk::new(false);
    let paths = copied(&disk);
    let plan = plan(&*disk, &paths).await.unwrap();
    let cache = tempfile::tempdir().unwrap();
    disk.hold_reads.store(true, Ordering::Release);
    let (fetched, ()) = tokio::join!(fetch(&*disk, &plan, cache.path(), || true), async {
        // The first read of deep.txt.
        until(|| disk.reading.load(Ordering::Acquire) > 0).await;
        let partial = names(cache.path());
        assert!(partial.len() == 1 && partial[0].starts_with(PARTIAL));
        std::fs::write(
            disk.home.path().join("project/docs/nested/deep.txt"),
            b"deeper",
        )
        .unwrap();
        disk.reads.add_permits(Semaphore::MAX_PERMITS);
    });
    assert_eq!(fetched.unwrap_err().to_string(), CHANGED);
    assert!(names(cache.path()).is_empty());
}

#[tokio::test]
async fn a_large_file_is_read_with_several_requests_outstanding_and_assembled_in_order() {
    let disk = Disk::new(false);
    let large = disk.home.path().join("large.bin");
    let bytes: Vec<u8> = (0..20 * 65_536 + 17).map(|i| (i % 251) as u8).collect();
    std::fs::write(&large, &bytes).unwrap();
    let plan = plan(&*disk, &[path(&large)]).await.unwrap();
    let cache = tempfile::tempdir().unwrap();
    disk.hold_reads.store(true, Ordering::Release);
    let (folder, ()) = tokio::join!(fetch(&*disk, &plan, cache.path(), || true), async {
        // The whole window is requested before any reply.
        until(|| disk.reading.load(Ordering::Acquire) >= 16).await;
        assert_eq!(disk.reading.load(Ordering::Acquire), 16);
        disk.reads.add_permits(Semaphore::MAX_PERMITS);
    });
    let folder = folder.unwrap();
    assert!(std::fs::read(folder.join("large.bin")).unwrap() == bytes);
}

/// A server whose `realpath` claims every path under its root is where it
/// says, as a dishonest or confused server would.
struct Liar {
    disk: Arc<Disk>,
    root: String,
}
impl RemoteFs for Liar {
    async fn home(&self) -> Result<String> {
        self.disk.home().await
    }
    async fn realpath(&self, path: &str) -> Result<String> {
        if path.starts_with(&format!("{}/", self.root)) {
            return Ok(path.to_owned());
        }
        self.disk.realpath(path).await
    }
    async fn kind(&self, path: &str) -> Result<Option<Kind>> {
        self.disk.kind(path).await
    }
    async fn lstat(&self, path: &str) -> Result<Option<Stat>> {
        self.disk.lstat(path).await
    }
    async fn readlink(&self, path: &str) -> Result<String> {
        self.disk.readlink(path).await
    }
    async fn list(&self, path: &str) -> Result<Option<Vec<Item>>> {
        self.disk.list(path).await
    }
    async fn open_read(&self, path: &str) -> Result<String> {
        self.disk.open_read(path).await
    }
    async fn read(&self, handle: &str, offset: u64, len: u32) -> Result<Option<Vec<u8>>> {
        self.disk.read(handle, offset, len).await
    }
    async fn close(&self, handle: String) -> Result<()> {
        self.disk.close(handle).await
    }
    async fn mkdir(&self, _: &str, _: u32) -> Result<()> {
        unimplemented!()
    }
    async fn create(&self, _: &str, _: u32) -> Result<String> {
        unimplemented!()
    }
    async fn write(&self, _: &str, _: u64, _: Vec<u8>) -> Result<()> {
        unimplemented!()
    }
    async fn symlink(&self, _: &str, _: &str) -> Result<()> {
        unimplemented!()
    }
    async fn rename(&self, _: &str, _: &str) -> Result<()> {
        unimplemented!()
    }
    async fn remove_file(&self, _: &str) -> Result<()> {
        unimplemented!()
    }
    async fn remove_dir(&self, _: &str) -> Result<()> {
        unimplemented!()
    }
    async fn touch(&self, _: &str) -> Result<()> {
        unimplemented!()
    }
    async fn set_attributes(&self, _: &str, _: Option<u32>, _: Option<u32>) -> Result<()> {
        unimplemented!()
    }
}

#[tokio::test]
async fn links_that_resolve_outside_locally_fail_the_fetch_whatever_the_server_says() {
    let disk = Disk::new(false);
    let home = disk.home.path().canonicalize().unwrap();
    let cases = [
        // `s` is `tree`, so `l` climbs out of the download: the lexical walk
        // that ignores where `s` leads sees nothing wrong.
        (
            "chain",
            vec![("a/b/s", "../.."), ("a/b/l", "s/../../..")],
            "chain/a/b/l",
        ),
        ("dangling", vec![("a/link", "missing")], "dangling/a/link"),
    ];
    for (name, links, named) in cases {
        let tree = home.join(name);
        std::fs::create_dir_all(tree.join("a/b")).unwrap();
        for (link, target) in links {
            std::os::unix::fs::symlink(target, tree.join(link)).unwrap();
        }
        let liar = Liar {
            disk: disk.clone(),
            root: path(&tree),
        };
        let planned = plan(&liar, &[path(&tree)]).await.unwrap();
        let cache = tempfile::tempdir().unwrap();
        let error = fetch(&liar, &planned, cache.path(), || true)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!("“{named}” links outside the copied folder")),
            "{name}: {error}"
        );
        assert!(names(cache.path()).is_empty(), "{name}");
    }
}

#[tokio::test]
async fn a_file_that_yields_more_than_planned_fails_and_the_budget_is_enforced() {
    let disk = Disk::new(false);
    let announced = disk.home.path().join("announced.txt");
    std::fs::write(&announced, b"").unwrap();
    let growing = disk.home.path().join("growing.bin");
    std::fs::write(&growing, vec![1u8; 100]).unwrap();
    for (file, grown) in [(announced, 5usize), (growing, 20 * 65_536)] {
        let planned = plan(&*disk, &[path(&file)]).await.unwrap();
        std::fs::write(&file, vec![2u8; grown]).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let error = fetch(&*disk, &planned, cache.path(), || true)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), CHANGED, "{file:?}");
        assert!(names(cache.path()).is_empty());
    }
    // Less budget left than the file's planned size.
    let file = disk.home.path().join("ten.txt");
    std::fs::write(&file, vec![3u8; 10]).unwrap();
    let planned = plan(&*disk, &[path(&file)]).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let error = write(
        &*disk,
        &planned.entries()[0],
        &dir.path().join("out"),
        3,
        &|| true,
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), TOO_LARGE);
    let copied = write(
        &*disk,
        &planned.entries()[0],
        &dir.path().join("ok"),
        10,
        &|| true,
    )
    .await
    .unwrap();
    assert_eq!(copied, 10);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn every_downloaded_item_is_quarantined() {
    let quarantine = |path: &Path| {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let mut buffer = [0u8; 256];
        let length = unsafe {
            libc::getxattr(
                path.as_ptr(),
                c"com.apple.quarantine".as_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
                libc::XATTR_NOFOLLOW,
            )
        };
        assert!(length > 0, "{path:?} has no quarantine attribute");
        String::from_utf8(buffer[..length as usize].to_vec()).unwrap()
    };
    let disk = Disk::new(false);
    let paths = copied(&disk);
    // A read-only file is marked too.
    let locked = disk.home.path().join("project/locked.txt");
    std::fs::write(&locked, b"x").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();
    let plan = plan(&*disk, &[paths, vec![path(&locked)]].concat())
        .await
        .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let folder = fetch(&*disk, &plan, cache.path(), || true).await.unwrap();
    for entry in plan.entries() {
        let value = quarantine(&folder.join(&entry.path));
        let parts: Vec<&str> = value.split(';').collect();
        assert_eq!(parts[0], "0081", "{value}");
        assert!(u64::from_str_radix(parts[1], 16).unwrap() > 1_700_000_000);
        assert_eq!(parts[2], "Newport");
    }
    assert_eq!(mode(&folder.join("locked.txt")), 0o444);
    assert_eq!(std::fs::read(folder.join("locked.txt")).unwrap(), b"x");
}

#[tokio::test]
async fn a_failed_download_removes_its_partial() {
    let disk = Disk::new(false);
    let paths = copied(&disk);
    let plan = plan(&*disk, &paths).await.unwrap();
    std::fs::remove_file(disk.home.path().join("project/名前 1.txt")).unwrap();
    let cache = tempfile::tempdir().unwrap();
    assert!(fetch(&*disk, &plan, cache.path(), || true).await.is_err());
    assert!(names(cache.path()).is_empty());
}

#[tokio::test]
async fn a_download_stops_between_reads_once_its_copy_is_no_longer_current() {
    let disk = Disk::new(false);
    let paths = copied(&disk);
    let plan = plan(&*disk, &paths).await.unwrap();
    let cache = tempfile::tempdir().unwrap();
    let current = AtomicBool::new(true);
    disk.hold_reads.store(true, Ordering::Release);
    let fetching = fetch(&*disk, &plan, cache.path(), || {
        current.load(Ordering::Acquire)
    });
    let (fetched, ()) = tokio::join!(fetching, async {
        until(|| disk.reading.load(Ordering::Acquire) > 0).await;
        // Let deep.txt finish; the large file's first read waits again.
        disk.reads.add_permits(2);
        until(|| disk.reading.load(Ordering::Acquire) > 0 && disk.reads.available_permits() == 0)
            .await;
        current.store(false, Ordering::Release);
        disk.reads.add_permits(Semaphore::MAX_PERMITS);
    });
    assert_eq!(fetched.unwrap_err().to_string(), SUPERSEDED);
    // One chunk of the large file was read; the next was never requested.
    assert!(names(cache.path()).is_empty());
}

#[tokio::test]
async fn a_dropped_download_removes_its_partial() {
    let disk = Disk::new(false);
    let paths = copied(&disk);
    let plan = plan(&*disk, &paths).await.unwrap();
    let cache = tempfile::tempdir().unwrap();
    disk.hold_reads.store(true, Ordering::Release);
    tokio::select! {
        _ = fetch(&*disk, &plan, cache.path(), || true) => panic!("held download completed"),
        () = until(|| disk.reading.load(Ordering::Acquire) > 0) => {}
    }
    until(|| names(cache.path()).is_empty()).await;
}

fn age(path: &Path, by: Duration) {
    std::fs::File::open(path)
        .unwrap()
        .set_modified(SystemTime::now() - by)
        .unwrap();
}

#[tokio::test]
async fn the_newest_copy_wins_and_superseded_downloads_go_after_the_grace_period() {
    let disk = Disk::new(false);
    let paths = copied(&disk);
    let plan = plan(&*disk, &paths).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().join("Clipboard"));
    let first = cache.begin(|| true);
    let a = cache.fetch(&*disk, &plan, &first).await.unwrap();
    let second = cache.begin(|| true);
    assert!(!first.is_current() && second.is_current());
    // A superseded copy's download stops and leaves nothing behind.
    let late = cache.fetch(&*disk, &plan, &first).await.unwrap_err();
    assert_eq!(late.to_string(), SUPERSEDED);
    let b = cache.fetch(&*disk, &plan, &second).await.unwrap();
    assert_eq!(names(cache.root()).len(), 2);
    // Downloads run before the clock is paused: their blocking file work
    // would let it auto-advance.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(9 * 60)).await;
    cache.tick();
    tokio::task::yield_now().await;
    assert_eq!(names(cache.root()).len(), 2);
    tokio::time::advance(Duration::from_secs(60)).await;
    cache.tick();
    until(|| names(cache.root()).len() == 1).await;
    assert!(!a.exists() && b.exists());
    // Retiring a superseded ticket keeps the newer copy's download; retiring
    // the current one retires it the same way.
    cache.retire(&first);
    assert!(second.is_current());
    cache.retire(&second);
    assert!(!second.is_current());
    tokio::time::advance(GRACE).await;
    cache.tick();
    until(|| names(cache.root()).is_empty()).await;
}

#[tokio::test]
async fn a_ticket_follows_its_fence() {
    let disk = Disk::new(false);
    let paths = copied(&disk);
    let plan = plan(&*disk, &paths).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().join("Clipboard"));
    let available = Arc::new(AtomicBool::new(false));
    let fence = available.clone();
    let ticket = cache.begin(move || fence.load(Ordering::Acquire));
    assert!(!ticket.is_current());
    let error = cache.fetch(&*disk, &plan, &ticket).await.unwrap_err();
    assert_eq!(error.to_string(), SUPERSEDED);
    assert!(names(cache.root()).is_empty());
    available.store(true, Ordering::Release);
    assert!(ticket.is_current());
}

#[test]
fn each_profile_gets_its_own_cache_folder() {
    let profiles = tempfile::tempdir().unwrap();
    let (a, b) = (profiles.path().join("a"), profiles.path().join("b"));
    std::fs::create_dir_all(&a).unwrap();
    let root = Cache::default_root(&a).unwrap();
    assert_eq!(root.parent().unwrap(), base_root().unwrap());
    let name = root.file_name().unwrap().to_str().unwrap();
    assert_eq!(name.len(), 16);
    assert!(
        name.bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
    );
    // Stable for the same profile, however it is spelled; distinct otherwise.
    assert_eq!(Cache::default_root(&a.join("../a")), Some(root.clone()));
    assert_eq!(Cache::default_root(&a), Some(root.clone()));
    assert_ne!(Cache::default_root(&b), Some(root.clone()));
    #[cfg(target_os = "macos")]
    {
        assert_eq!(
            base_root().unwrap(),
            dirs::home_dir()
                .unwrap()
                .join("Library/Caches/Newport/Clipboard")
        );
        let folder = root.join(uuid::Uuid::new_v4().to_string());
        assert_eq!(
            attachable(&folder).unwrap(),
            format!("{name}/{}", folder.file_name().unwrap().to_str().unwrap())
        );
        assert_eq!(attachable(Path::new("/tmp/elsewhere")), None);
    }
}

#[tokio::test]
async fn the_first_use_sweeps_partials_and_hour_old_downloads_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Clipboard");
    let cache = Cache::new(root.clone());
    assert!(!root.exists());
    std::fs::create_dir_all(&root).unwrap();
    let name = |prefix: &str| format!("{prefix}{}", uuid::Uuid::new_v4());
    let (partial, stale, fresh) = (name(PARTIAL), name(""), name(""));
    for folder in [&partial, &stale, &fresh] {
        std::fs::create_dir_all(root.join(folder).join("inner")).unwrap();
        std::fs::write(root.join(folder).join("inner/file"), b"x").unwrap();
    }
    age(&root.join(&stale), Duration::from_secs(2 * 60 * 60));
    std::fs::write(root.join("unrelated"), b"kept").unwrap();
    cache.sweep().await;
    let mut expected = vec![fresh.clone(), "unrelated".to_owned()];
    expected.sort();
    assert_eq!(names(&root), expected);
    // Only the first use sweeps.
    let later = name(PARTIAL);
    std::fs::create_dir(root.join(&later)).unwrap();
    cache.sweep().await;
    assert!(names(&root).contains(&later));

    cache.clear();
    assert_eq!(names(&root), ["unrelated"]);
}

#[tokio::test]
async fn clearing_cancels_the_current_download_and_removes_every_download() {
    let disk = Disk::new(false);
    let paths = copied(&disk);
    let plan = plan(&*disk, &paths).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().join("Clipboard"));
    let first = cache.begin(|| true);
    cache.fetch(&*disk, &plan, &first).await.unwrap();
    let second = cache.begin(|| true);
    cache.fetch(&*disk, &plan, &second).await.unwrap();
    let third = cache.begin(|| true);
    cache.clear();
    assert!(!third.is_current());
    assert!(names(cache.root()).is_empty());
}

#[test]
fn clearing_removes_downloads_even_when_nothing_swept_first() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Clipboard");
    let cache = Cache::new(root.clone());
    let id = uuid::Uuid::new_v4();
    for folder in [id.to_string(), format!("{PARTIAL}{id}")] {
        std::fs::create_dir_all(root.join(folder).join("inner")).unwrap();
    }
    std::fs::write(root.join("unrelated"), b"kept").unwrap();
    cache.clear();
    assert_eq!(names(&root), ["unrelated"]);
}

/// The real [`crate::ssh::sftp::Sftp`] against the fixture of
/// `cargo xtask test ssh`, whose SFTP server works in a real directory.
mod fixture {
    use super::*;
    use crate::clipboard_files::remove_tree;
    use crate::clipboard_files::tests::fixture::server;
    use crate::ssh::sftp::Sftp;

    async fn put(sftp: &Sftp, path: &str, data: &[u8], mode: u32) {
        let handle = sftp.create(path, mode).await.unwrap();
        sftp.write(&handle, 0, data.to_vec()).await.unwrap();
        sftp.close(handle).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard_downloads::tests::fixture"]
    async fn listing_a_folder_stops_at_the_item_limit_over_sftp() {
        let sftp = Sftp::connect(&server()).await.unwrap();
        let base = format!("/clipboard-limit-{}", uuid::Uuid::new_v4());
        sftp.mkdir(&base, 0o700).await.unwrap();
        for index in 0..MAX_ENTRIES {
            put(&sftp, &format!("{base}/{index}"), b"", 0o600).await;
        }
        assert_eq!(
            sftp.list(&base).await.unwrap().unwrap().len() as u64,
            MAX_ENTRIES
        );
        put(&sftp, &format!("{base}/one-more"), b"", 0o600).await;
        let error = sftp.list(&base).await.err().unwrap();
        assert_eq!(error.to_string(), TOO_LARGE);
        let error = plan(&sftp, std::slice::from_ref(&base)).await.unwrap_err();
        assert_eq!(error.to_string(), TOO_LARGE);
        // Too many to list, so it is emptied by name.
        let names = (0..MAX_ENTRIES).map(|index| index.to_string());
        for name in names.chain(["one-more".to_owned()]) {
            sftp.remove_file(&format!("{base}/{name}")).await.unwrap();
        }
        remove_tree(&sftp, &base).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "Run with cargo xtask test ssh -- clipboard_downloads::tests::fixture"]
    async fn a_tree_is_downloaded_over_sftp_and_renamed_into_place() {
        let sftp = Sftp::connect(&server()).await.unwrap();
        let base = format!("/clipboard-downloads-{}", uuid::Uuid::new_v4());
        sftp.mkdir(&base, 0o700).await.unwrap();
        let tree = format!("{base}/tree");
        sftp.mkdir(&tree, 0o755).await.unwrap();
        sftp.mkdir(&format!("{tree}/sub"), 0o755).await.unwrap();
        put(&sftp, &format!("{tree}/a.txt"), b"first", 0o640).await;
        let large: Vec<u8> = (0..70_000u32).map(|index| index as u8).collect();
        put(&sftp, &format!("{tree}/sub/b.bin"), &large, 0o700).await;
        sftp.symlink(&format!("{tree}/sub/link"), "../a.txt")
            .await
            .unwrap();
        put(&sftp, &format!("{base}/名前.txt"), b"named", 0o644).await;

        let paths = [tree.clone(), format!("{base}/名前.txt")];
        let planned = plan(&sftp, &paths).await.unwrap();
        assert_eq!(planned.count(), 6);
        assert_eq!(planned.bytes(), 5 + 70_000 + 5);
        assert_eq!(decide(&planned), Decision::Ask);
        let cache = tempfile::tempdir().unwrap();
        let folder = fetch(&sftp, &planned, cache.path(), || true).await.unwrap();
        assert_eq!(
            names(cache.path()),
            [folder.file_name().unwrap().to_str().unwrap()]
        );
        assert_eq!(std::fs::read(folder.join("tree/a.txt")).unwrap(), b"first");
        assert_eq!(std::fs::read(folder.join("tree/sub/b.bin")).unwrap(), large);
        assert_eq!(
            std::fs::read_link(folder.join("tree/sub/link")).unwrap(),
            Path::new("../a.txt")
        );
        assert_eq!(std::fs::read(folder.join("名前.txt")).unwrap(), b"named");
        assert_eq!(mode(&folder.join("tree/a.txt")), 0o640);
        assert_eq!(mode(&folder.join("tree/sub/b.bin")), 0o700);
        assert_eq!(mode(&folder.join("tree/sub")), 0o755);
        let stat = sftp.lstat(&format!("{tree}/a.txt")).await.unwrap().unwrap();
        assert_eq!(Some(mtime(&folder.join("tree/a.txt")) as u32), stat.mtime);

        // A link leaving the copied folder is refused over SFTP too.
        sftp.symlink(&format!("{tree}/escape"), "../名前.txt")
            .await
            .unwrap();
        let error = plan(&sftp, &paths).await.unwrap_err();
        assert!(
            error.to_string().contains("“tree/escape” links outside"),
            "{error}"
        );
        remove_tree(&sftp, &base).await.unwrap();
    }
}
