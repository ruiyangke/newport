use super::*;
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};

fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn resolver(root: &Path) -> Resolver<'_> {
    Resolver::new(root, Instant::now() + Duration::from_secs(2), &|| false)
}

#[test]
fn worktrees_of_one_repository_share_its_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "srv/repo/.git/HEAD", "ref: refs/heads/main\n");
    write(root, "srv/repo/.git/worktrees/feature/commondir", "../..\n");
    write(
        root,
        "srv/repo/.git/worktrees/feature/HEAD",
        "0123456789abcdef0123456789abcdef01234567\n",
    );
    write(
        root,
        "srv/feature/.git",
        "gitdir: ../repo/.git/worktrees/feature\n",
    );
    fs::create_dir_all(root.join("srv/feature/api")).unwrap();
    let mut resolver = resolver(root);
    assert_eq!(
        resolver.resolve("/srv/feature/api").unwrap().project,
        Some(Project {
            root: "/srv/repo".into(),
            marker: ".git".into(),
            worktree: Some("/srv/feature".into()),
            branch: Some("0123456".into()),
            main: Some(false),
        })
    );
    assert_eq!(
        resolver.resolve("/srv/repo").unwrap().project.unwrap().main,
        Some(true)
    );
}

#[test]
fn roots_follow_projectile_order_and_stop_at_home_and_system_boundaries() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "opt/mono/package.json", "{}");
    write(root, "opt/mono/packages/web/package.json", "{}");
    write(root, "srv/build/Makefile", "");
    write(root, "srv/build/sub/Makefile", "");
    write(root, "home/user/package.json", "{}");
    let mut resolver = resolver(root);
    assert_eq!(
        resolver
            .resolve("/opt/mono/packages/web")
            .unwrap()
            .project
            .unwrap()
            .root,
        "/opt/mono"
    );
    assert_eq!(
        resolver
            .resolve("/srv/build/sub")
            .unwrap()
            .project
            .unwrap()
            .root,
        "/srv/build"
    );
    assert!(resolver.resolve("/home/user").unwrap().project.is_none());
    assert!(
        resolver
            .resolve("/usr/lib/missing")
            .unwrap()
            .project
            .is_none()
    );
    assert!(resolver.resolve("/opt/mono/gone (deleted)").is_err());
}

#[test]
fn explicit_roots_precede_vcs_and_vcs_precedes_manifests() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "srv/mono/.projectile", "");
    write(root, "srv/mono/nested/.git/HEAD", "ref: refs/heads/main\n");
    write(root, "srv/mono/nested/child/Cargo.toml", "");
    let mut resolver = resolver(root);
    let project = resolver
        .resolve("/srv/mono/nested/child")
        .unwrap()
        .project
        .unwrap();
    assert_eq!(
        (project.root.as_str(), project.marker.as_str()),
        ("/srv/mono", ".projectile")
    );
    fs::remove_file(root.join("srv/mono/.projectile")).unwrap();
    let project = super::tests::resolver(root)
        .resolve("/srv/mono/nested/child")
        .unwrap()
        .project
        .unwrap();
    assert_eq!(project.root, "/srv/mono/nested");
}

#[test]
fn submodule_gitdir_is_its_own_checkout_and_sha256_head_is_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "srv/repo/.git/modules/sub/HEAD",
        &format!("{}\n", "a".repeat(64)),
    );
    write(root, "srv/repo/sub/.git", "gitdir: ../.git/modules/sub\n");
    let project = resolver(root)
        .resolve("/srv/repo/sub")
        .unwrap()
        .project
        .unwrap();
    assert_eq!(project.root, "/srv/repo/sub");
    assert_eq!(project.main, Some(true));
    assert_eq!(project.branch.as_deref(), Some("aaaaaaa"));
}

#[test]
fn unreadable_and_oversized_git_metadata_keep_the_observed_project_marker() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "srv/repo/.git/HEAD", &"x".repeat(MAX_FIELD + 1));
    let result = resolver(root).resolve("/srv/repo").unwrap();
    assert_eq!(result.project.unwrap().root, "/srv/repo");
    assert_eq!(
        result.limitation.unwrap().kind(),
        io::ErrorKind::FileTooLarge
    );
    fs::set_permissions(
        root.join("srv/repo/.git/HEAD"),
        fs::Permissions::from_mode(0o0),
    )
    .unwrap();
    if crate::paths::effective_uid() != 0 {
        let result = resolver(root).resolve("/srv/repo").unwrap();
        assert!(result.project.is_some());
        assert_eq!(
            result.limitation.unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}

#[test]
fn invalid_head_and_symlinked_metadata_are_not_exposed_as_branches() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "srv/repo/.git/HEAD", "unrelated-file-content\n");
    let result = resolver(root).resolve("/srv/repo").unwrap();
    assert!(result.project.unwrap().branch.is_none());
    assert!(result.limitation.is_some());
    fs::remove_file(root.join("srv/repo/.git/HEAD")).unwrap();
    write(root, "private", "ref: refs/heads/secret\n");
    symlink(root.join("private"), root.join("srv/repo/.git/HEAD")).unwrap();
    assert!(
        resolver(root)
            .resolve("/srv/repo")
            .unwrap()
            .limitation
            .is_some()
    );
}

#[test]
fn symbolic_head_links_name_the_branch_without_following_the_target() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("srv/repo/.git")).unwrap();
    symlink("refs/heads/feature", root.join("srv/repo/.git/HEAD")).unwrap();
    let result = resolver(root).resolve("/srv/repo").unwrap();
    assert!(result.limitation.is_none());
    assert_eq!(result.project.unwrap().branch.as_deref(), Some("feature"));
    for name in [
        "refs/heads/../private",
        "refs/heads/name.lock",
        "refs/heads/name with space",
        "refs/heads/topic@{1}",
        "refs/heads/name/",
    ] {
        assert!(!valid_reference(name), "{name}");
    }
}

#[test]
fn fifo_metadata_never_waits_for_a_writer() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("srv/repo/.git")).unwrap();
    let path = std::ffi::CString::new(
        root.join("srv/repo/.git/HEAD")
            .as_os_str()
            .as_encoded_bytes(),
    )
    .unwrap();
    // SAFETY: path is a terminated pathname and mode is a valid permission mask.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let start = Instant::now();
    let result = resolver(root).resolve("/srv/repo").unwrap();
    assert!(result.project.is_some());
    assert_eq!(
        result.limitation.unwrap().kind(),
        io::ErrorKind::InvalidData
    );
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn expired_cancelled_and_deep_scans_report_limits() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    assert_eq!(
        Resolver::new(root, Instant::now(), &|| false)
            .resolve("/srv/repo")
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::TimedOut
    );
    assert_eq!(
        Resolver::new(root, Instant::now() + TIME_LIMIT, &|| true)
            .resolve("/srv/repo")
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::Interrupted
    );
    let logical = format!("/srv/{}", vec!["deep"; MAX_DEPTH + 1].join("/"));
    fs::create_dir_all(root.join(logical.trim_start_matches('/'))).unwrap();
    assert_eq!(
        resolver(root).resolve(&logical).err().unwrap().kind(),
        io::ErrorKind::FileTooLarge
    );
}

#[test]
fn cache_is_per_scan_and_missing_directories_cannot_inherit_parent_projects() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "srv/repo/Cargo.toml", "");
    fs::create_dir_all(root.join("srv/repo/child")).unwrap();
    let mut cached = resolver(root);
    let first = cached.resolve("/srv/repo/child").unwrap();
    assert_eq!(
        cached.resolve("/srv/repo/child").unwrap().project,
        first.project
    );
    fs::remove_dir(root.join("srv/repo/child")).unwrap();
    assert!(cached.resolve("/srv/repo/child").is_err());
    fs::remove_file(root.join("srv/repo/Cargo.toml")).unwrap();
    assert!(
        resolver(root)
            .resolve("/srv/repo")
            .unwrap()
            .project
            .is_none()
    );
}
