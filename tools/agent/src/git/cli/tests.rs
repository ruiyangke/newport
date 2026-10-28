use super::*;
use std::process::Command;
fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}
fn setup() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(
        dir.path(),
        &["init", "-q", "--initial-branch=main", "--template="],
    );
    git(dir.path(), &["config", "user.name", "Fixture"]);
    git(
        dir.path(),
        &["config", "user.email", "fixture@example.test"],
    );
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    git(dir.path(), &["config", "tag.gpgsign", "false"]);
    dir
}
fn request(service: &mut impl Adapter, method: &str, params: Value) -> Result<Value, Error> {
    let wire = json!({"method":method,"params":params});
    let (response, trace_response) =
        match service.call(serde_json::from_value(wire.clone()).unwrap())? {
            Output::Json(v) => (v.clone(), v),
            Output::Diff { snapshot, bytes } => {
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                (value.clone(), json!({"snapshot":snapshot,"diff":value}))
            }
        };
    // Every operation fixture preserves its contract through the wire codec.
    for value in [&wire, &trace_response] {
        let bytes = super::super::protocol::encode_value(value).unwrap();
        assert_eq!(
            super::super::protocol::decode_value(&bytes).unwrap(),
            *value
        );
    }
    if service.cli() {
        if let Some(path) = std::env::var_os("NEWPORT_CLI_TRACE_PATH") {
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(
                file,
                "{}",
                json!({"request":wire,"response":trace_response})
            )
            .unwrap();
        }
    }
    Ok(response)
}
trait Adapter {
    fn cli(&self) -> bool {
        false
    }
    fn call(&mut self, r: Request) -> Result<Output, Error>;
}
impl Adapter for Service {
    fn cli(&self) -> bool {
        true
    }
    fn call(&mut self, r: Request) -> Result<Output, Error> {
        self.request(r)
    }
}
#[test]
fn historical_renamed_file_diff_keeps_both_paths_and_only_the_selected_change() {
    for edited in [false, true] {
        let dir = setup();
        let original = (0..20).map(|n| format!("line {n}\n")).collect::<String>();
        fs::write(dir.path().join("old name"), &original).unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-qm", "base"]);
        git(dir.path(), &["mv", "old name", "new name"]);
        if edited {
            fs::write(
                dir.path().join("new name"),
                original.replace("line 10\n", "changed line\n"),
            )
            .unwrap();
        }
        fs::write(dir.path().join("unrelated"), "unrelated addition\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-qm", "rename"]);
        let head = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let mut service = Service::default();
        let id = opened(&mut service, dir.path());
        let listing = request(
            &mut service,
            "repo.commit_files",
            json!({"repoId":id,"commitOid":head}),
        )
        .unwrap();
        let renamed = listing["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["status"] == "Renamed")
            .unwrap();
        let diff = request(
            &mut service,
            "repo.commit_diff",
            json!({"repoId":id,"commitOid":head,"path":renamed["newPath"]}),
        )
        .unwrap();
        let files = diff["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["status"], "Renamed");
        assert_eq!(files[0]["oldPath"], renamed["oldPath"]);
        assert_eq!(files[0]["newPath"], renamed["newPath"]);
        assert_eq!(files[0]["additions"], usize::from(edited));
        assert_eq!(files[0]["deletions"], usize::from(edited));
        let old_path_diff = request(
            &mut service,
            "repo.commit_diff",
            json!({"repoId":id,"commitOid":head,"path":renamed["oldPath"]}),
        )
        .unwrap();
        assert_eq!(old_path_diff["files"], diff["files"]);
        let page = request(
            &mut service,
            "repo.commit_diff_page",
            json!({"repoId":id,"commitOid":head,"path":renamed["newPath"],"pageSize":1}),
        )
        .unwrap();
        assert_eq!(page["metadata"]["selectedPath"], renamed["newPath"]);
        assert_eq!(page["entries"][0]["oldPath"], renamed["oldPath"]);
    }
}
fn compare(root: &Path) {
    let mut cli = Service::default();
    let path = WirePath::new(root.as_os_str().as_bytes());
    let opened = request(&mut cli, "repo.open", json!({"path":path})).unwrap();
    assert_eq!(opened["bare"], false);
    let a = request(&mut cli, "repo.status_summary", json!({"path":path})).unwrap();
    let b = request(
        &mut cli,
        "repo.status_summary",
        json!({"repoId":opened["repoId"]}),
    )
    .unwrap();
    assert_eq!(a, b);
    let status = request(&mut cli, "repo.status", json!({"repoId":opened["repoId"]})).unwrap();
    assert_eq!(
        a["totalEntries"],
        status["entries"].as_array().unwrap().len()
    );
}
#[test]
fn status_resolves_configured_upstream_without_parsing_tracking_ref_names() {
    let dir = setup();
    git(dir.path(), &["commit", "--allow-empty", "-qm", "initial"]);
    git(
        dir.path(),
        &["remote", "add", "team/upstream", "/unused/local/remote"],
    );
    git(
        dir.path(),
        &[
            "config",
            "remote.team/upstream.fetch",
            "+refs/heads/release/next:refs/remotes/cache/local-alias",
        ],
    );
    git(
        dir.path(),
        &["config", "branch.main.remote", "team/upstream"],
    );
    git(
        dir.path(),
        &["config", "branch.main.merge", "refs/heads/release/next"],
    );
    git(
        dir.path(),
        &["update-ref", "refs/remotes/cache/local-alias", "HEAD"],
    );
    let mut cli = Service::default();
    let opened = request(
        &mut cli,
        "repo.open",
        json!({"path":WirePath::new(dir.path().as_os_str().as_bytes())}),
    )
    .unwrap();
    let params = json!({"repoId":opened["repoId"],"pageSize":1});
    let status = request(&mut cli, "repo.status", params.clone()).unwrap();
    let metadata = &status["metadata"];
    assert_eq!(
        metadata["upstreamRef"]["display"],
        "refs/remotes/cache/local-alias"
    );
    assert_eq!(metadata["upstream"]["remote"]["display"], "team/upstream");
    assert_eq!(
        metadata["upstream"]["reference"]["display"],
        "refs/heads/release/next"
    );
    assert_eq!(metadata["ahead"], 0);
    assert_eq!(metadata["behind"], 0);
    // Configuration alone is part of the write snapshot, even if HEAD and the
    // worktree remain unchanged. A reviewed transfer cannot survive retargeting.
    git(
        dir.path(),
        &["config", "branch.main.merge", "refs/heads/different"],
    );
    let changed = request(&mut cli, "repo.status", params.clone()).unwrap();
    assert_ne!(status["snapshot"], changed["snapshot"]);
    git(dir.path(), &["checkout", "--detach", "-q"]);
    let detached = request(&mut cli, "repo.status", params).unwrap();
    assert!(detached["metadata"]["upstream"].is_null());
}

#[test]
fn open_and_summary_contract_cover_repository_states() {
    let dir = setup();
    compare(dir.path());
    fs::write(dir.path().join("tracked"), "before\n").unwrap();
    git(dir.path(), &["add", "tracked"]);
    git(dir.path(), &["commit", "-qm", "initial"]);
    compare(dir.path());
    fs::write(dir.path().join("tracked"), "after\n").unwrap();
    fs::create_dir(dir.path().join("untracked")).unwrap();
    fs::write(dir.path().join("untracked/child"), "x").unwrap();
    fs::write(
        dir.path().join(OsStr::from_bytes(b"newline\nquoted name")),
        "x",
    )
    .unwrap();
    compare(dir.path());
    git(dir.path(), &["add", "tracked"]);
    fs::write(dir.path().join("tracked"), "again\n").unwrap();
    compare(dir.path());
    git(dir.path(), &["reset", "--hard", "-q", "HEAD"]);
    git(dir.path(), &["mv", "tracked", "renamed"]);
    compare(dir.path());
    git(dir.path(), &["commit", "-qm", "rename"]);
    git(dir.path(), &["branch", "tracking"]);
    git(
        dir.path(),
        &["branch", "--set-upstream-to=tracking", "main"],
    );
    compare(dir.path());
    git(dir.path(), &["checkout", "--detach", "-q"]);
    compare(dir.path());
}
#[test]
fn linked_worktree_and_bare_open_match() {
    let dir = setup();
    git(dir.path(), &["commit", "--allow-empty", "-qm", "initial"]);
    let holder = tempfile::tempdir().unwrap();
    let linked = holder.path().join("linked");
    git(
        dir.path(),
        &["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
    );
    compare(&linked);
    let bare = holder.path().join("bare");
    git(
        dir.path(),
        &[
            "clone",
            "--bare",
            "-q",
            dir.path().to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let params = json!({"path":WirePath::new(bare.as_os_str().as_bytes())});
    let opened = request(&mut Service::default(), "repo.open", params).unwrap();
    assert_eq!(opened["bare"], true);
    assert_eq!(opened["capabilities"]["workingTree"], false);
}
#[test]
fn paginated_history_and_commit_rows_match_and_anchor_the_original_tip() {
    let dir = setup();
    for n in 0..7 {
        git(
            dir.path(),
            &[
                "commit",
                "--allow-empty",
                "-qm",
                &format!("commit {n}\n\nlong body"),
            ],
        );
    }
    let repo = Repo::discover(WirePath::new(dir.path().as_os_str().as_bytes())).unwrap();
    let id = repo.id.encode();
    let mut cli = Service::default();
    let mut ca = Value::Null;
    let expected = String::from_utf8(git(dir.path(), &["rev-list", "HEAD"])).unwrap();
    let expected: Vec<_> = expected.lines().collect();
    let mut total = 0;
    loop {
        let a = request(
            &mut cli,
            "repo.history",
            json!({"repoId":id,"pageSize":2,"cursor":ca,"messageBytes":9}),
        )
        .unwrap();
        let entries = a["entries"].as_array().unwrap();
        for (offset, row) in entries.iter().enumerate() {
            assert_eq!(row["oid"]["hex"], expected[total + offset]);
            assert_eq!(row["messageTruncated"], true);
        }
        if total == 0 {
            git(dir.path(), &["commit", "--allow-empty", "-qm", "new tip"]);
        }
        for row in a["entries"].as_array().unwrap() {
            let params = json!({"repoId":id,"commitOid":row["oid"]["hex"]});
            let commit = request(&mut cli, "repo.commit", params).unwrap();
            let expected_message = git(
                dir.path(),
                &[
                    "show",
                    "-s",
                    "--format=%B",
                    row["oid"]["hex"].as_str().unwrap(),
                ],
            );
            assert_eq!(
                commit["message"]["display"].as_str().unwrap().trim_end(),
                String::from_utf8_lossy(&expected_message).trim_end()
            );
        }
        total += a["entries"].as_array().unwrap().len();
        ca = a["nextCursor"].clone();
        if ca.is_null() {
            break;
        }
    }
    assert_eq!(total, 7);
}
#[test]
fn refuses_bad_selectors_foreign_cursors_and_unimplemented_mutations() {
    let dir = setup();
    let repo = Repo::discover(WirePath::new(dir.path().as_os_str().as_bytes())).unwrap();
    let id = repo.id.encode();
    let mut cli = Service::default();
    assert_eq!(
        request(&mut cli, "repo.status_summary", json!({}))
            .unwrap_err()
            .code,
        "INVALID_REQUEST"
    );
    assert_eq!(
        request(
            &mut cli,
            "repo.status_summary",
            json!({"repoId":id,"path":WirePath::new(b"/tmp")})
        )
        .unwrap_err()
        .code,
        "INVALID_REQUEST"
    );

    fs::write(repo.git_dir.join("MERGE_HEAD"), "marker").unwrap();
    let summary = request(&mut cli, "repo.status_summary", json!({"repoId":id})).unwrap();
    assert_eq!(summary["operationState"], "Merge");
    assert_eq!(summary["integration"]["managed"], false);
}

#[test]
fn cursors_reject_changed_shallow_boundaries() {
    let dir = setup();
    for _ in 0..3 {
        git(dir.path(), &["commit", "--allow-empty", "-qm", "commit"]);
    }
    let repo = Repo::discover(WirePath::new(dir.path().as_os_str().as_bytes())).unwrap();
    let params = json!({"repoId":repo.id.encode(),"pageSize":1});
    let page = request(&mut Service::default(), "repo.history", params.clone()).unwrap();
    fs::write(
        repo.common.join("shallow"),
        git(dir.path(), &["rev-parse", "HEAD"]),
    )
    .unwrap();
    let mut next = params;
    next["cursor"] = page["nextCursor"].clone();
    assert_eq!(
        request(&mut Service::default(), "repo.history", next)
            .unwrap_err()
            .code,
        "SNAPSHOT_EXPIRED"
    );
}

#[test]
fn repository_identity_detects_replacement_and_history_rejects_options() {
    let dir = setup();
    let repo = Repo::discover(WirePath::new(dir.path().as_os_str().as_bytes())).unwrap();
    let id = repo.id.encode();
    assert_eq!(
        request(
            &mut Service::default(),
            "repo.history",
            json!({"repoId":id,"revision":"--all"})
        )
        .unwrap_err()
        .code,
        "NOT_FOUND"
    );
    fs::rename(&repo.git_dir, dir.path().join("old.git")).unwrap();
    git(dir.path(), &["init", "-q", "--template="]);
    assert_eq!(
        request(
            &mut Service::default(),
            "repo.status_summary",
            json!({"repoId":id})
        )
        .unwrap_err()
        .code,
        "REPO_REPLACED"
    );
}

#[test]
fn read_commands_leave_index_and_working_files_unchanged() {
    let dir = setup();
    fs::write(dir.path().join("file"), "before").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "initial"]);
    fs::write(dir.path().join("file"), "after").unwrap();
    let index = fs::read(dir.path().join(".git/index")).unwrap();
    compare(dir.path());
    assert_eq!(index, fs::read(dir.path().join(".git/index")).unwrap());
    assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"after");
    assert!(!dir.path().join(".git/index.lock").exists());
}

fn writable(home: &Path) -> Service {
    Service::with_journal(Some(
        super::super::journal::Journal::open(
            home.join("journal"),
            uuid::Uuid::new_v4().to_string(),
        )
        .unwrap(),
    ))
}
fn opened(service: &mut Service, path: &Path) -> String {
    request(
        service,
        "repo.open",
        json!({"path":WirePath::new(path.as_os_str().as_bytes())}),
    )
    .unwrap()["repoId"]
        .as_str()
        .unwrap()
        .into()
}
fn operation(service: &mut Service, id: &str, action: Value) -> Value {
    let method = if action["kind"].as_str().unwrap().starts_with("worktree.") {
        "repo.worktrees"
    } else {
        "repo.status"
    };
    let status = request(service, method, json!({"repoId":id})).unwrap();
    let params = json!({"operationId":uuid::Uuid::new_v4().to_string(),"repoId":id,"expectedSnapshot":status["snapshot"],"action":action});
    let result = request(service, "operation.start", params.clone()).unwrap();
    eprintln!("action {} -> {}", params["action"]["kind"], result["state"]);
    assert!(
        result["state"] == "succeeded" || result["state"] == "needs_resolution",
        "{result}"
    );
    assert_eq!(
        request(service, "operation.start", params).unwrap(),
        result,
        "idempotent replay"
    );
    let checked = request(
        service,
        "operation.get",
        json!({"operationId":result["operationId"]}),
    )
    .unwrap();
    assert_eq!(checked, result);
    result
}
#[test]
fn cli_stage_unstage_commit_amend_and_stale_snapshot_guards() {
    let dir = setup();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, dir.path());
    fs::write(dir.path().join("file"), "one\n").unwrap();
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    let entry = status["entries"][0]["entryId"].clone();
    operation(
        &mut service,
        &id,
        json!({"kind":"stage","entryIds":[entry]}),
    );
    assert!(!git(dir.path(), &["ls-files"]).is_empty());
    operation(
        &mut service,
        &id,
        json!({"kind":"unstage","entryIds":[entry]}),
    );
    assert!(git(dir.path(), &["ls-files"]).is_empty());
    operation(
        &mut service,
        &id,
        json!({"kind":"stage","entryIds":[entry]}),
    );
    let committed = operation(
        &mut service,
        &id,
        json!({"kind":"commit","message":"CLI initial"}),
    );
    let head = committed["result"]["commitOid"].as_str().unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"commit.amend","expectedOid":head,"message":"CLI amended"}),
    );
    assert_eq!(
        git(dir.path(), &["log", "-1", "--format=%s"]),
        b"CLI amended\n"
    );
    fs::write(dir.path().join("file"), "two\n").unwrap();
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    fs::write(dir.path().join("file"), "three\n").unwrap();
    assert_eq!(request(&mut service,"operation.start",json!({"operationId":uuid::Uuid::new_v4().to_string(),"repoId":id,"expectedSnapshot":status["snapshot"],"action":{"kind":"stage","entryIds":[entry]}})).unwrap_err().code,"STALE_SNAPSHOT");
    assert_eq!(git(dir.path(), &["show", ":file"]), b"one\n");
}
#[test]
fn cli_reset_modes_preserve_the_expected_index_and_worktree() {
    for mode in ["soft", "mixed", "hard"] {
        let dir = setup();
        fs::write(dir.path().join("file"), "base\n").unwrap();
        git(dir.path(), &["add", "file"]);
        git(dir.path(), &["commit", "-qm", "base"]);
        let base = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        fs::write(dir.path().join("file"), "head\n").unwrap();
        git(dir.path(), &["commit", "-qam", "head"]);
        let head = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        fs::write(dir.path().join("file"), "staged\n").unwrap();
        git(dir.path(), &["add", "file"]);
        fs::write(dir.path().join("file"), "working\n").unwrap();
        fs::write(dir.path().join("untracked"), "keep\n").unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut service = writable(home.path());
        let id = opened(&mut service, dir.path());
        operation(
            &mut service,
            &id,
            json!({"kind":"reset","targetOid":base,"expectedOid":head,"mode":mode}),
        );
        assert_eq!(
            String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
                .unwrap()
                .trim(),
            base
        );
        assert_eq!(
            git(dir.path(), &["show", ":file"]),
            if mode == "soft" {
                b"staged\n".as_slice()
            } else {
                b"base\n".as_slice()
            }
        );
        assert_eq!(
            fs::read(dir.path().join("file")).unwrap(),
            if mode == "hard" {
                b"base\n".as_slice()
            } else {
                b"working\n".as_slice()
            }
        );
        assert_eq!(fs::read(dir.path().join("untracked")).unwrap(), b"keep\n");
    }
}

#[test]
fn cli_detached_checkout_preserves_branch_and_local_edits() {
    let dir = setup();
    fs::write(dir.path().join("file"), "first\n").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "first"]);
    let first = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    fs::write(dir.path().join("file"), "second\n").unwrap();
    git(dir.path(), &["commit", "-am", "second", "-q"]);
    let second = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, dir.path());
    fs::write(dir.path().join("file"), "local edit\n").unwrap();
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    let refused = request(&mut service, "operation.start", json!({"operationId":uuid::Uuid::new_v4().to_string(),"repoId":id,"expectedSnapshot":status["snapshot"],"action":{"kind":"checkout","target":{"kind":"detached","oid":first}}})).unwrap();
    assert_eq!(refused["state"], "failed");
    assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"local edit\n");
    assert_eq!(
        String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
            .unwrap()
            .trim(),
        second
    );
    // Restore only this disposable test file before checking the successful path.
    fs::write(dir.path().join("file"), "second\n").unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"checkout","target":{"kind":"detached","oid":first}}),
    );
    assert_eq!(git(dir.path(), &["branch", "--show-current"]), b"");
    assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"first\n");
    assert_eq!(
        String::from_utf8(git(dir.path(), &["rev-parse", "main"]))
            .unwrap()
            .trim(),
        second
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"checkout","target":{"kind":"branch","name":"main","expectedOid":second}}),
    );
    assert_eq!(git(dir.path(), &["branch", "--show-current"]), b"main\n");
}

#[test]
fn cli_branch_tag_checkout_and_remote_controls() {
    let dir = setup();
    git(dir.path(), &["commit", "--allow-empty", "-qm", "initial"]);
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, dir.path());
    let head = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    operation(
        &mut service,
        &id,
        json!({"kind":"branch.create","name":"topic","startOid":head}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"checkout","target":{"kind":"branch","name":"topic","expectedOid":head}}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"branch.rename","name":"topic","newName":"renamed","expectedOid":head}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"checkout","target":{"kind":"branch","name":"main","expectedOid":head}}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"branch.delete","name":"renamed","expectedOid":head}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"tag.create","name":"v1","targetOid":head}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"tag.delete","name":"v1","expectedOid":head}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"remote.add","name":"origin","url":"https://example.test/repo"}),
    );
    git(
        dir.path(),
        &[
            "config",
            "remote.origin.url",
            "https://user:secret@example.test/repo?token=hidden",
        ],
    );
    let remote = request(
        &mut service,
        "repo.remote",
        json!({"repoId":id,"name":"origin"}),
    )
    .unwrap();
    assert!(!remote.to_string().contains("secret"));
    assert!(!remote.to_string().contains("hidden"));
    operation(
        &mut service,
        &id,
        json!({"kind":"remote.rename","name":"origin","newName":"upstream","expectedToken":remote["token"]}),
    );
    let remote = request(
        &mut service,
        "repo.remote",
        json!({"repoId":id,"name":"upstream"}),
    )
    .unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"remote.set_url","name":"upstream","url":"https://example.test/repo2","expectedToken":remote["token"]}),
    );
    let remote = request(
        &mut service,
        "repo.remote",
        json!({"repoId":id,"name":"upstream"}),
    )
    .unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"remote.remove","name":"upstream","expectedToken":remote["token"]}),
    );
    assert!(git(dir.path(), &["remote"]).is_empty());
}
#[test]
fn cli_status_and_branches_report_fixture_and_expire_pages() {
    let dir = setup();
    git(dir.path(), &["commit", "--allow-empty", "-qm", "initial"]);
    git(dir.path(), &["branch", "topic"]);
    for name in ["a", "b", "c"] {
        fs::write(dir.path().join(name), "data").unwrap();
    }
    git(dir.path(), &["add", "a"]);
    fs::write(dir.path().join("a"), "changed").unwrap();
    let id = opened(&mut Service::default(), dir.path());
    let mut cli = Service::default();
    let status = request(&mut cli, "repo.status", json!({"repoId":id})).unwrap();
    assert_eq!(status["entries"].as_array().unwrap().len(), 3);
    assert_eq!(status["entries"][0]["staged"], true);
    assert_eq!(status["entries"][0]["unstaged"], true);
    let branches = request(&mut cli, "repo.branches", json!({"repoId":id})).unwrap();
    assert_eq!(branches["entries"].as_array().unwrap().len(), 2);
    let first = request(&mut cli, "repo.status", json!({"repoId":id,"pageSize":1})).unwrap();
    fs::write(dir.path().join("d"), "data").unwrap();
    assert_eq!(
        request(
            &mut cli,
            "repo.status",
            json!({"repoId":id,"pageSize":1,"cursor":first["nextCursor"]})
        )
        .unwrap_err()
        .code,
        "SNAPSHOT_EXPIRED"
    );
}

#[test]
fn cli_diff_pages_partial_lines_and_historical_reads() {
    let dir = setup();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    fs::write(dir.path().join("file"), "a\nb\nc\n").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "initial"]);
    let id = opened(&mut service, dir.path());
    fs::write(dir.path().join("file"), "a\nB\nc\nnew\n").unwrap();
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    let entry = status["entries"][0]["entryId"].clone();
    let diff=request(&mut service,"repo.diff",json!({"repoId":id,"snapshot":status["snapshot"],"entryId":entry,"side":"index_to_worktree","contextLines":3})).unwrap();
    assert_eq!(diff["files"][0]["additions"], 2);
    let hunk = &diff["files"][0]["hunks"][0];
    let selected = hunk["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["origin"] == "+" && l["content"]["display"] == "new\n")
        .map(|l| l["id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 1);
    let mut cursor = Value::Null;
    let mut pieces = 0;
    loop {
        let page=request(&mut service,"repo.diff_page",json!({"repoId":id,"snapshot":status["snapshot"],"entryId":entry,"side":"index_to_worktree","contextLines":3,"pageSize":5000,"maxBytes":65536,"cursor":cursor,"lineEncoding":"tuple_v1"})).unwrap();
        pieces += page["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                f["hunks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| h["lines"].as_array().unwrap().len())
                    .sum::<usize>()
            })
            .sum::<usize>();
        cursor = page["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(pieces, 5);
    operation(
        &mut service,
        &id,
        json!({"kind":"stage","entryIds":[entry],"hunks":{"ids":[hunk["id"]],"lines":selected,"contextLines":3}}),
    );
    assert_eq!(git(dir.path(), &["show", ":file"]), b"a\nb\nc\nnew\n");
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    let staged = request(
        &mut service,
        "repo.diff",
        json!({"repoId":id,"snapshot":status["snapshot"],"entryId":entry,"side":"head_to_index"}),
    )
    .unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"unstage","entryIds":[entry],"hunks":{"ids":[staged["files"][0]["hunks"][0]["id"]],"contextLines":3}}),
    );
    assert_eq!(git(dir.path(), &["show", ":file"]), b"a\nb\nc\n");
    operation(
        &mut service,
        &id,
        json!({"kind":"discard","entryIds":[entry],"source":"index"}),
    );
    assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"a\nb\nc\n");
    let head = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    request(
        &mut service,
        "repo.commit_files",
        json!({"repoId":id,"commitOid":head}),
    )
    .unwrap();
    request(
        &mut service,
        "repo.commit_diff",
        json!({"repoId":id,"commitOid":head}),
    )
    .unwrap();
    request(&mut service,"repo.commit_diff_page",json!({"repoId":id,"commitOid":head,"path":WirePath::new(b"file"),"pageSize":1,"lineEncoding":"tuple_v1"})).unwrap();
    let blob = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD:file"]))
        .unwrap()
        .trim()
        .to_owned();
    request(&mut service, "repo.blob", json!({"repoId":id,"oid":blob})).unwrap();
    request(
        &mut service,
        "repo.blob_page",
        json!({"repoId":id,"oid":blob}),
    )
    .unwrap();
    request(&mut service, "repo.close", json!({"repoId":id})).unwrap();
}
#[test]
fn cli_select_loaded_lines_without_fetching_remaining_diff_pages() {
    let dir = setup();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    git(dir.path(), &["commit", "--allow-empty", "-qm", "initial"]);
    fs::write(dir.path().join("new-file"), "one\ntwo\nthree\nfour\n").unwrap();
    let id = opened(&mut service, dir.path());
    for (side, kind, expected) in [
        ("index_to_worktree", "stage", "one\ntwo\n"),
        ("head_to_index", "unstage", "two\n"),
    ] {
        let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
        let entry = status["entries"][0]["entryId"].clone();
        let page = request(
            &mut service,
            "repo.diff_page",
            json!({
                "repoId":id,"snapshot":status["snapshot"],"entryId":entry,
                "side":side,"contextLines":3,"pageSize":if kind == "stage" {2} else {1}
            }),
        )
        .unwrap();
        assert!(page["nextCursor"].is_string());
        let hunk = &page["entries"][0]["hunks"][0];
        let lines: Vec<_> = hunk["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| {
                assert_eq!(line["lineComplete"], true);
                line["id"].clone()
            })
            .collect();
        operation(
            &mut service,
            &id,
            json!({
                "kind":kind,"entryIds":[entry],
                "hunks":{"ids":[hunk["id"]],"lines":lines,"contextLines":3}
            }),
        );
        assert_eq!(git(dir.path(), &["show", ":new-file"]), expected.as_bytes());
        assert_eq!(
            fs::read(dir.path().join("new-file")).unwrap(),
            b"one\ntwo\nthree\nfour\n"
        );
    }
}
#[test]
fn cli_stash_and_tag_round_trip() {
    let dir = setup();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    fs::write(dir.path().join("file"), "base").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "initial"]);
    let id = opened(&mut service, dir.path());
    fs::write(dir.path().join("file"), "changed").unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"stash.save","message":"saved","includeUntracked":true}),
    );
    for kind in ["stash.apply", "stash.pop"] {
        let list = request(&mut service, "repo.stashes", json!({"repoId":id})).unwrap();
        assert_eq!(list["entries"].as_array().unwrap().len(), 1);
        operation(
            &mut service,
            &id,
            json!({"kind":kind,"oid":list["entries"][0]["oid"],"index":0,"expectedToken":list["metadata"]["listToken"]}),
        );
        assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"changed");
        git(dir.path(), &["reset", "--hard", "-q", "HEAD"]);
    }
    fs::write(dir.path().join("file"), "changed again").unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"stash.save","message":"drop"}),
    );
    let list = request(&mut service, "repo.stashes", json!({"repoId":id})).unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"stash.drop","oid":list["entries"][0]["oid"],"index":0,"expectedToken":list["metadata"]["listToken"]}),
    );
    let head = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    operation(
        &mut service,
        &id,
        json!({"kind":"tag.create","name":"annotated","targetOid":head,"annotation":{"message":"release"}}),
    );
    let tags = request(&mut service, "repo.tags", json!({"repoId":id})).unwrap();
    assert_eq!(tags["entries"][0]["annotated"], true);
    request(
        &mut service,
        "repo.tag",
        json!({"repoId":id,"oid":tags["entries"][0]["oid"]["hex"]}),
    )
    .unwrap();
}
#[test]
fn a_failed_clone_says_what_git_said() {
    let parent = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let destination = parent.path().join("copy");
    let missing = parent.path().join("no-such-repository");
    let clone = request(
        &mut service,
        "repo.clone",
        json!({"operationId":uuid::Uuid::new_v4().to_string(),"path":WirePath::new(destination.as_os_str().as_bytes()),"url":missing.to_str().unwrap()}),
    )
    .unwrap();
    assert_eq!(clone["state"], "failed", "{clone}");
    let message = clone["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("Git couldn't clone the repository: ")
            && message.contains("no-such-repository"),
        "{message}"
    );
    assert!(!destination.exists());
}
#[test]
fn init_makes_a_missing_folder_but_not_its_parents() {
    let parent = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    fn init(service: &mut impl Adapter, path: &std::path::Path) -> Result<Value, Error> {
        request(
            service,
            "repo.init",
            json!({"operationId":uuid::Uuid::new_v4().to_string(),"path":WirePath::new(path.as_os_str().as_bytes()),"initialBranch":"main"}),
        )
    }
    let notes = parent.path().join("notes");
    assert_eq!(init(&mut service, &notes).unwrap()["state"], "succeeded");
    assert!(notes.join(".git").is_dir());
    let deep = parent.path().join("missing/notes");
    assert_eq!(
        init(&mut service, &deep).unwrap_err().code,
        "DIRECTORY_REQUIRED"
    );
    assert!(!parent.path().join("missing").exists());
    // A refused request leaves no folder behind.
    let nested = notes.join("nested");
    assert_eq!(
        init(&mut service, &nested).unwrap_err().code,
        "ALREADY_REPOSITORY"
    );
    assert!(!nested.exists());
}
#[test]
fn cli_init_clone_and_worktree_lifecycle() {
    let parent = tempfile::tempdir().unwrap();
    let project = parent.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("keep"), "keep").unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let params = json!({"operationId":uuid::Uuid::new_v4().to_string(),"path":WirePath::new(project.as_os_str().as_bytes()),"initialBranch":"main"});
    let initialized = request(&mut service, "repo.init", params.clone()).unwrap();
    assert_eq!(initialized["state"], "succeeded");
    assert_eq!(
        request(&mut service, "repo.init", params).unwrap(),
        initialized
    );
    assert_eq!(fs::read(project.join("keep")).unwrap(), b"keep");
    git(&project, &["config", "user.name", "Fixture"]);
    git(&project, &["config", "user.email", "fixture@example.test"]);
    git(&project, &["config", "commit.gpgsign", "false"]);
    git(&project, &["add", "keep"]);
    git(&project, &["commit", "-qm", "initial"]);
    let cloned = parent.path().join("cloned");
    let clone=request(&mut service,"repo.clone",json!({"operationId":uuid::Uuid::new_v4().to_string(),"path":WirePath::new(cloned.as_os_str().as_bytes()),"url":project.to_str().unwrap()})).unwrap();
    assert_eq!(clone["state"], "succeeded", "{clone}");
    let id = opened(&mut service, &project);
    let head = String::from_utf8(git(&project, &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    let linked = parent.path().join("checkout");
    operation(
        &mut service,
        &id,
        json!({"kind":"worktree.add","name":"custom-name","path":WirePath::new(linked.as_os_str().as_bytes()),"branch":"linked","expectedOid":head,"newBranch":true}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"worktree.lock","name":"custom-name","reason":"keep"}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"worktree.unlock","name":"custom-name"}),
    );
    let moved = parent.path().join("moved");
    fs::rename(&linked, &moved).unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"worktree.repair","name":"custom-name","path":WirePath::new(moved.as_os_str().as_bytes())}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"worktree.remove","name":"custom-name"}),
    );
    assert!(!moved.exists());
    operation(
        &mut service,
        &id,
        json!({"kind":"worktree.add","name":"gone","path":WirePath::new(linked.as_os_str().as_bytes()),"branch":"gone","expectedOid":head,"newBranch":true}),
    );
    fs::remove_dir_all(&linked).unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"worktree.prune","name":"gone"}),
    );
}

#[test]
fn cli_remote_fetch_pull_push_leases_and_tracking() {
    let source = setup();
    fs::write(source.path().join("file"), "one").unwrap();
    git(source.path(), &["add", "file"]);
    git(source.path(), &["commit", "-qm", "initial"]);
    let holder = tempfile::tempdir().unwrap();
    let remote = holder.path().join("remote.git");
    git(
        source.path(),
        &[
            "clone",
            "--bare",
            "-q",
            source.path().to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );
    let checkout = holder.path().join("checkout");
    git(
        source.path(),
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    git(&checkout, &["config", "commit.gpgsign", "false"]);
    git(&checkout, &["config", "user.name", "Fixture"]);
    git(&checkout, &["config", "user.email", "fixture@example.test"]);
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, &checkout);
    let details = request(
        &mut service,
        "repo.remote",
        json!({"repoId":id,"name":"origin"}),
    )
    .unwrap();
    let token = details["token"].clone();
    request(&mut service, "repo.remotes", json!({"repoId":id})).unwrap();
    request(&mut service, "repo.remote_names", json!({"repoId":id})).unwrap();
    let refs = request(
        &mut service,
        "repo.remote_refs",
        json!({"repoId":id,"remote":"origin","expectedToken":token}),
    )
    .unwrap();
    assert!(!refs["entries"].as_array().unwrap().is_empty());
    git(
        source.path(),
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    fs::write(source.path().join("file"), "two").unwrap();
    git(source.path(), &["commit", "-qam", "remote change"]);
    git(source.path(), &["push", "-q", "origin", "main"]);
    operation(
        &mut service,
        &id,
        json!({"kind":"fetch","remote":"origin","expectedToken":token}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"pull.fast_forward","remote":"origin","expectedToken":token,"remoteBranch":"main"}),
    );
    assert_eq!(fs::read(checkout.join("file")).unwrap(), b"two");
    let old = String::from_utf8(git(&checkout, &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    git(&checkout, &["commit", "--allow-empty", "-qm", "local"]);
    let head = String::from_utf8(git(&checkout, &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    operation(
        &mut service,
        &id,
        json!({"kind":"push","remote":"origin","expectedToken":token,"branch":"main","expectedOid":head,"destinationBranch":"topic"}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"push.with_lease","remote":"origin","expectedToken":token,"branch":"main","expectedOid":head,"destinationBranch":"main","expectedRemoteOid":old}),
    );
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    let stale_push = request(&mut service, "operation.start", json!({
        "repoId":id,"operationId":uuid::Uuid::new_v4().to_string(),
        "expectedSnapshot":status["snapshot"],
        "action":{"kind":"push.with_lease","remote":"origin","expectedToken":token,
            "branch":"main","expectedOid":head,"destinationBranch":"main","expectedRemoteOid":old}
    })).unwrap();
    assert_eq!(
        stale_push["error"]["code"], "STALE_REMOTE_REFERENCE",
        "{stale_push}"
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"branch.delete_remote","remote":"origin","expectedToken":token,"branch":"topic","expectedOid":head}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"tag.create","name":"pushed","targetOid":head}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"tag.push","remote":"origin","expectedToken":token,"name":"pushed","expectedOid":head}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"tag.delete_remote","remote":"origin","expectedToken":token,"name":"pushed","expectedOid":head}),
    );
    let branches = request(
        &mut service,
        "repo.branches",
        json!({"repoId":id,"filter":"main","branchKind":"local"}),
    )
    .unwrap();
    let tracking = branches["entries"][0]["tracking"]["token"].clone();
    operation(
        &mut service,
        &id,
        json!({"kind":"branch.set_upstream","name":"main","expectedOid":head,"expectedToken":tracking,"upstream":"origin/main"}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"reset","expectedOid":head,"targetOid":old,"mode":"soft"}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"merge.fast_forward","targetOid":head}),
    );
    // Known command refusal is a failed outcome, never an ambiguous receipt.
    let params = json!({"operationId":uuid::Uuid::new_v4().to_string(),"repoId":id,"expectedSnapshot":request(&mut service,"repo.status",json!({"repoId":id})).unwrap()["snapshot"],"action":{"kind":"commit","message":"empty"}});
    let failed = request(&mut service, "operation.start", params).unwrap();
    assert_eq!(failed["state"], "failed");
}
#[test]
fn cli_fetch_prune_removes_deleted_tracking_branches() {
    let source = setup();
    fs::write(source.path().join("file"), "one").unwrap();
    git(source.path(), &["add", "file"]);
    git(source.path(), &["commit", "-qm", "initial"]);
    let holder = tempfile::tempdir().unwrap();
    let remote = holder.path().join("remote.git");
    git(
        source.path(),
        &[
            "clone",
            "--bare",
            "-q",
            source.path().to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );
    let checkout = holder.path().join("checkout");
    git(
        source.path(),
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    git(&checkout, &["checkout", "-qb", "doomed"]);
    git(&checkout, &["push", "-q", "origin", "doomed"]);
    git(&checkout, &["checkout", "-q", "main"]);
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, &checkout);
    let token = request(
        &mut service,
        "repo.remote",
        json!({"repoId":id,"name":"origin"}),
    )
    .unwrap()["token"]
        .clone();
    let tracking = || {
        String::from_utf8(git(
            &checkout,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/remotes/origin/doomed",
            ],
        ))
        .unwrap()
        .trim()
        .to_owned()
    };
    let fetch = |service: &mut Service, prune: bool| {
        operation(
            service,
            &id,
            json!({"kind":"fetch","remote":"origin","expectedToken":token,"prune":prune}),
        )
    };
    fetch(&mut service, false);
    assert_eq!(tracking(), "refs/remotes/origin/doomed");
    // Deleted directly upstream -- a `push --delete` would also drop the
    // local tracking ref, so bypass it to model a stale one. A plain fetch
    // keeps it.
    git(
        source.path(),
        &[
            "--git-dir",
            remote.to_str().unwrap(),
            "branch",
            "-D",
            "doomed",
        ],
    );
    fetch(&mut service, false);
    assert_eq!(tracking(), "refs/remotes/origin/doomed");
    // A pruning fetch removes it.
    fetch(&mut service, true);
    assert_eq!(tracking(), "");
}
#[test]
fn cli_repository_configuration_round_trip() {
    let dir = setup();
    fs::write(dir.path().join("file"), "one").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "initial"]);
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, dir.path());
    let configuration =
        request(&mut service, "repo.remotes", json!({"repoId":id})).unwrap()["configuration"]
            .clone();
    // Seed values come from the fixture repository itself.
    assert_eq!(configuration["fetchPrune"], false);
    assert_eq!(configuration["userName"], "Fixture");
    assert_eq!(configuration["userEmail"], "fixture@example.test");
    let token = configuration["token"].as_str().unwrap().to_owned();
    let set = |service: &mut Service, token: &str, params: Value| {
        let mut params = params;
        params["expectedToken"] = token.into();
        operation(service, &id, params)
    };
    // A stale token fails instead of racing a concurrent edit.
    let snapshot =
        request(&mut service, "repo.status", json!({"repoId":id})).unwrap()["snapshot"].clone();
    let stale = request(
        &mut service,
        "operation.start",
        json!({"operationId":uuid::Uuid::new_v4().to_string(),"repoId":id,
            "expectedSnapshot":snapshot,
            "action":{"kind":"config.set","expectedToken":"stale","fetchPrune":true}}),
    )
    .unwrap();
    assert_eq!(stale["error"]["code"], "STALE_CONFIGURATION");
    // Empty writes and invalid values are refused, not stored.
    for params in [
        json!({"kind":"config.set"}),
        json!({"kind":"config.set","userName":""}),
        json!({"kind":"config.set","userEmail":"-x"}),
        json!({"kind":"config.set","fetchPrune":true,"userName":"Changed","userEmail":"-x"}),
        json!({"kind":"config.set","fetchPrune":true,"userName":"Changed","userEmail":"bad\nemail"}),
    ] {
        let mut params = params;
        params["expectedToken"] = token.clone().into();
        let snapshot =
            request(&mut service, "repo.status", json!({"repoId":id})).unwrap()["snapshot"].clone();
        let outcome = request(
            &mut service,
            "operation.start",
            json!({"operationId":uuid::Uuid::new_v4().to_string(),"repoId":id,
                "expectedSnapshot":snapshot,
                "action":params}),
        )
        .unwrap();
        assert_eq!(outcome["state"], "failed");
        assert_eq!(
            request(&mut service, "repo.remotes", json!({"repoId":id})).unwrap()["configuration"],
            configuration,
            "Rejected settings must not partially change earlier fields"
        );
    }
    set(
        &mut service,
        &token,
        json!({"kind":"config.set","fetchPrune":true,"userName":"Repo Author","userEmail":"repo@example.test"}),
    );
    let configuration =
        request(&mut service, "repo.remotes", json!({"repoId":id})).unwrap()["configuration"]
            .clone();
    assert_eq!(configuration["fetchPrune"], true);
    assert_eq!(configuration["userName"], "Repo Author");
    assert_eq!(configuration["userEmail"], "repo@example.test");
    assert_ne!(configuration["token"], token);
    // The old token no longer applies after the write.
    let token = configuration["token"].as_str().unwrap().to_owned();
    set(
        &mut service,
        &token,
        json!({"kind":"config.set","fetchPrune":false}),
    );
    let configuration =
        request(&mut service, "repo.remotes", json!({"repoId":id})).unwrap()["configuration"]
            .clone();
    assert_eq!(configuration["fetchPrune"], false);
    // Untouched values survive a partial write.
    assert_eq!(configuration["userName"], "Repo Author");
}
#[test]
fn cli_merge_conflict_resolution_continue_and_abort() {
    let dir = setup();
    fs::write(dir.path().join("file"), "base\n").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "base"]);
    git(dir.path(), &["checkout", "-qb", "topic"]);
    fs::write(dir.path().join("file"), "topic\n").unwrap();
    git(dir.path(), &["commit", "-qam", "topic"]);
    let topic = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    git(dir.path(), &["checkout", "-q", "main"]);
    fs::write(dir.path().join("file"), "main\n").unwrap();
    git(dir.path(), &["commit", "-qam", "main"]);
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, dir.path());
    for abort in ["merge.abort", "integration.abort"] {
        let result = operation(&mut service, &id, json!({"kind":"merge","targetOid":topic}));
        assert_eq!(result["state"], "needs_resolution");
        operation(&mut service, &id, json!({"kind":abort}));
        assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"main\n");
    }
    operation(&mut service, &id, json!({"kind":"merge","targetOid":topic}));
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    let conflict = &status["entries"][0];
    operation(
        &mut service,
        &id,
        json!({"kind":"conflict.resolve","entryIds":[conflict["entryId"]],"side":"theirs","expectedOid":conflict["conflict"]["theirs"]["oid"]["hex"]}),
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"integration.continue","message":"merge topic"}),
    );
    assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"topic\n");
    assert!(!dir.path().join(".git/MERGE_HEAD").exists());
}
#[test]
fn cli_cherry_pick_revert_rebase_skip_and_review() {
    let dir = setup();
    fs::write(dir.path().join("file"), "base\n").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let base = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    git(dir.path(), &["checkout", "-qb", "topic"]);
    fs::write(dir.path().join("extra"), "extra").unwrap();
    git(dir.path(), &["add", "extra"]);
    git(dir.path(), &["commit", "-qm", "extra"]);
    let topic = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    git(dir.path(), &["checkout", "-q", "main"]);
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, dir.path());
    operation(
        &mut service,
        &id,
        json!({"kind":"cherry_pick","targetOid":topic}),
    );
    let cherry = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    operation(
        &mut service,
        &id,
        json!({"kind":"revert","targetOid":cherry}),
    );
    assert!(!dir.path().join("extra").exists());
    git(dir.path(), &["checkout", "-q", "topic"]);
    operation(
        &mut service,
        &id,
        json!({"kind":"rebase","upstreamOid":base}),
    );
    git(dir.path(), &["checkout", "-q", "main"]);
    fs::write(dir.path().join("file"), "main\n").unwrap();
    git(dir.path(), &["commit", "-qam", "main change"]);
    let main = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned();
    git(dir.path(), &["checkout", "-q", "topic"]);
    fs::write(dir.path().join("file"), "topic\n").unwrap();
    git(dir.path(), &["commit", "-qam", "topic change"]);
    let result = operation(
        &mut service,
        &id,
        json!({"kind":"rebase","upstreamOid":main}),
    );
    assert_eq!(result["state"], "needs_resolution");
    operation(&mut service, &id, json!({"kind":"integration.skip"}));
    let receipt = uuid::Uuid::new_v4().to_string();
    let journal = service.journal.as_ref().unwrap();
    journal
        .begin(&receipt, "1".repeat(64), "2".repeat(64))
        .unwrap();
    let unknown = request(
        &mut service,
        "operation.get",
        json!({"operationId":receipt}),
    )
    .unwrap();
    assert_eq!(unknown["state"], "outcome_unknown");
    let reviewed = request(
        &mut service,
        "operation.review",
        json!({"operationId":receipt}),
    )
    .unwrap();
    assert_eq!(reviewed["state"], "reviewed_unknown");
}

#[test]
fn cli_replay_continue_uses_custom_message_once_and_preserves_authorship() {
    for kind in ["cherry_pick", "revert", "rebase"] {
        let dir = setup();
        fs::write(dir.path().join("file"), "base\n").unwrap();
        git(dir.path(), &["add", "file"]);
        git(dir.path(), &["commit", "-qm", "base"]);
        git(dir.path(), &["checkout", "-qb", "topic"]);
        fs::write(dir.path().join("file"), "topic\n").unwrap();
        git(
            dir.path(),
            &[
                "commit",
                "-qam",
                "topic",
                "--author=Topic Author <topic@example.test>",
            ],
        );
        let topic = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        git(dir.path(), &["checkout", "-q", "main"]);
        fs::write(dir.path().join("file"), "main\n").unwrap();
        git(dir.path(), &["commit", "-qam", "main"]);
        let main = String::from_utf8(git(dir.path(), &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let action = match kind {
            "revert" => {
                git(dir.path(), &["checkout", "-q", "topic"]);
                json!({"kind":kind,"targetOid":main})
            }
            "rebase" => {
                fs::write(dir.path().join("later"), "later\n").unwrap();
                git(dir.path(), &["add", "later"]);
                git(dir.path(), &["commit", "-qm", "Later unchanged"]);
                json!({"kind":kind,"upstreamOid":topic})
            }
            _ => json!({"kind":kind,"targetOid":topic}),
        };
        let home = tempfile::tempdir().unwrap();
        let mut service = writable(home.path());
        let id = opened(&mut service, dir.path());
        let result = operation(&mut service, &id, action);
        assert_eq!(result["state"], "needs_resolution", "{kind}: {result}");
        fs::write(dir.path().join("file"), "resolved\n").unwrap();
        git(dir.path(), &["add", "file"]);
        // These characters are message content, never shell source.
        let message = "Resolution $(touch SHOULD_NOT_EXIST) `false` 'quoted'\n\nBody.";
        let result = operation(
            &mut service,
            &id,
            json!({"kind":"integration.continue","message":message}),
        );
        assert_eq!(result["state"], "succeeded", "{kind}: {result}");
        let commit = if kind == "rebase" { "HEAD^" } else { "HEAD" };
        assert_eq!(
            String::from_utf8(git(dir.path(), &["log", "-1", "--format=%B", commit]))
                .unwrap()
                .trim_end(),
            message,
            "{kind}"
        );
        let author = String::from_utf8(git(
            dir.path(),
            &["show", "-s", "--format=%an <%ae>", commit],
        ))
        .unwrap();
        assert_eq!(
            author.trim(),
            if kind == "cherry_pick" {
                "Topic Author <topic@example.test>"
            } else {
                "Fixture <fixture@example.test>"
            }
        );
        if kind == "rebase" {
            assert_eq!(
                String::from_utf8(git(dir.path(), &["log", "-1", "--format=%s"]))
                    .unwrap()
                    .trim(),
                "Later unchanged"
            );
        }
        assert!(!dir.path().join("SHOULD_NOT_EXIST").exists());
        assert!(git(dir.path(), &["status", "--porcelain"]).is_empty());
    }
}

#[test]
fn cli_long_line_pages_and_large_blob_windows_are_bounded() {
    let dir = setup();
    fs::write(dir.path().join("file"), "before\n").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let id = opened(&mut Service::default(), dir.path());
    let mut service = Service::default();
    let text = "x".repeat(12000);
    fs::write(dir.path().join("file"), &text).unwrap();
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    let mut cursor = Value::Null;
    let mut recovered = Vec::new();
    loop {
        let page=request(&mut service,"repo.diff_page",json!({"repoId":id,"snapshot":status["snapshot"],"entryId":status["entries"][0]["entryId"],"side":"index_to_worktree","pageSize":1,"maxBytes":16384,"cursor":cursor})).unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() <= 16384);
        for file in page["entries"].as_array().unwrap() {
            for hunk in file["hunks"].as_array().unwrap() {
                for line in hunk["lines"].as_array().unwrap() {
                    if line["origin"] == "+" {
                        use base64::Engine;
                        recovered.extend(
                            base64::engine::general_purpose::STANDARD
                                .decode(line["contentBytesB64"].as_str().unwrap())
                                .unwrap(),
                        );
                    }
                }
            }
        }
        cursor = page["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(recovered, text.as_bytes());
    let big = dir.path().join("big");
    fs::write(&big, vec![42; 33 * 1024 * 1024]).unwrap();
    let object = String::from_utf8(git(dir.path(), &["hash-object", "-w", "big"]))
        .unwrap()
        .trim()
        .to_owned();
    let first = request(
        &mut service,
        "repo.blob_page",
        json!({"repoId":id,"oid":object,"maxBytes":16384}),
    )
    .unwrap();
    assert!(serde_json::to_vec(&first).unwrap().len() <= 16384);
    let mut last =
        super::super::tokens::CursorRef::decode(first["nextCursor"].as_str().unwrap()).unwrap();
    last.o = 33 * 1024 * 1024 - 19;
    let last = request(
        &mut service,
        "repo.blob_page",
        json!({"repoId":id,"oid":object,"maxBytes":16384,"cursor":last.encode()}),
    )
    .unwrap();
    assert!(last["nextCursor"].is_null());
    assert_eq!(last["metadata"]["size"], 33 * 1024 * 1024);
}
#[test]
fn cli_untracked_directory_lists_and_stages_individual_files() {
    let dir = setup();
    fs::write(dir.path().join(".gitignore"), "*.ignored\n").unwrap();
    git(dir.path(), &["add", ".gitignore"]);
    git(dir.path(), &["commit", "-qm", "initial"]);
    fs::create_dir_all(dir.path().join("docs/nested")).unwrap();
    fs::write(dir.path().join("docs/a.md"), "one\ntwo\n").unwrap();
    fs::write(dir.path().join("docs/nested/b.md"), "other\n").unwrap();
    fs::write(dir.path().join("docs/secret.ignored"), "ignored\n").unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, dir.path());
    let summary = request(&mut service, "repo.status_summary", json!({"repoId":id})).unwrap();
    assert_eq!(summary["totalEntries"], 2);
    assert_eq!(summary["groupCounts"]["untracked"], 2);
    let first = request(
        &mut service,
        "repo.status",
        json!({"repoId":id,"pageSize":1}),
    )
    .unwrap();
    assert_eq!(first["entries"][0]["path"]["display"], "docs/a.md");
    let second = request(
        &mut service,
        "repo.status",
        json!({"repoId":id,"pageSize":1,"cursor":first["nextCursor"]}),
    )
    .unwrap();
    assert_eq!(second["entries"][0]["path"]["display"], "docs/nested/b.md");
    assert!(second["nextCursor"].is_null());
    let entry = first["entries"][0]["entryId"].clone();
    let page = request(
        &mut service,
        "repo.diff_page",
        json!({
            "repoId":id,"snapshot":first["snapshot"],"entryId":entry,
            "side":"index_to_worktree","pageSize":1
        }),
    )
    .unwrap();
    assert_eq!(page["metadata"]["totalFiles"], 1);
    assert_eq!(page["entries"][0]["newPath"]["display"], "docs/a.md");
    let hunk = &page["entries"][0]["hunks"][0];
    operation(
        &mut service,
        &id,
        json!({"kind":"stage","entryIds":[entry],
        "hunks":{"ids":[hunk["id"]],"lines":[hunk["lines"][0]["id"]],"contextLines":3}}),
    );
    assert_eq!(git(dir.path(), &["show", ":docs/a.md"]), b"one\n");
    assert_eq!(
        git(dir.path(), &["diff", "--cached", "--name-only"]),
        b"docs/a.md\n"
    );
    assert_eq!(
        fs::read(dir.path().join("docs/a.md")).unwrap(),
        b"one\ntwo\n"
    );
    assert_eq!(
        fs::read(dir.path().join("docs/nested/b.md")).unwrap(),
        b"other\n"
    );
}
#[test]
fn cli_status_excludes_ignored_files_from_write_fingerprints() {
    let dir = setup();
    fs::write(dir.path().join(".gitignore"), "*.ignored\n").unwrap();
    git(dir.path(), &["add", ".gitignore"]);
    git(dir.path(), &["commit", "-qm", "ignore"]);
    fs::create_dir(dir.path().join("folder")).unwrap();
    fs::write(dir.path().join("folder/keep"), "one").unwrap();
    let ignored = fs::File::create(dir.path().join("folder/large.ignored")).unwrap();
    ignored.set_len(300 * 1024 * 1024).unwrap();
    let repo = Repo::discover(WirePath::new(dir.path().as_os_str().as_bytes())).unwrap();
    let before = status::capture(&repo).unwrap().fingerprint;
    ignored.set_len(301 * 1024 * 1024).unwrap();
    assert_eq!(before, status::capture(&repo).unwrap().fingerprint);
    fs::write(dir.path().join("folder/keep"), "two").unwrap();
    assert_ne!(before, status::capture(&repo).unwrap().fingerprint);
}
#[test]
fn cli_capabilities_cover_every_protocol_method() {
    let mut methods = METHODS.to_vec();
    methods.sort();
    let mut expected = super::super::protocol::METHODS.to_vec();
    expected.sort();
    assert_eq!(methods, expected);
    assert_eq!(ACTIONS.len(), 45);
    check_runtime().unwrap();
}

#[test]
fn cli_status_large_untracked_files_are_bounded_and_detect_edits() {
    use std::io::{Seek, SeekFrom, Write};
    let dir = setup();
    let mut file = fs::File::create(dir.path().join("large-build-artifact")).unwrap();
    file.set_len(400 * 1024 * 1024).unwrap();
    let mut service = Service::default();
    let id = opened(&mut service, dir.path());
    let first = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    assert_eq!(first["entries"].as_array().unwrap().len(), 1);
    let same = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    assert_eq!(first["snapshot"], same["snapshot"]);
    file.seek(SeekFrom::Start(300 * 1024 * 1024)).unwrap();
    file.write_all(b"changed").unwrap();
    file.sync_all().unwrap();
    let changed = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    assert_ne!(first["snapshot"], changed["snapshot"]);
}

#[test]
fn cli_stashes_distinguish_absent_ref_from_unreadable_history() {
    let dir = setup();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, dir.path());
    let empty = request(&mut service, "repo.stashes", json!({"repoId":id})).unwrap();
    assert!(empty["entries"].as_array().unwrap().is_empty());

    fs::write(dir.path().join("file"), "base").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "initial"]);
    fs::write(dir.path().join("file"), "changed").unwrap();
    git(dir.path(), &["stash", "push", "-qm", "saved"]);
    let stash = String::from_utf8(git(dir.path(), &["rev-parse", "refs/stash"])).unwrap();
    let stash = stash.trim();
    fs::remove_file(
        dir.path()
            .join(".git/objects")
            .join(&stash[..2])
            .join(&stash[2..]),
    )
    .unwrap();
    assert!(request(&mut service, "repo.stashes", json!({"repoId":id})).is_err());
}

#[test]
fn cli_missing_remote_preserves_selection_recovery_contract() {
    let dir = setup();
    let mut service = Service::default();
    let id = opened(&mut service, dir.path());
    let error = request(
        &mut service,
        "repo.remote",
        json!({"repoId":id,"name":"origin"}),
    )
    .unwrap_err();
    assert_eq!(error.code, "REMOTE_NOT_FOUND");
}

#[test]
fn cli_signal_after_commit_is_an_unknown_outcome() {
    use std::os::unix::fs::PermissionsExt;
    let dir = setup();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    fs::write(dir.path().join("file"), "base").unwrap();
    git(dir.path(), &["add", "file"]);
    let hook = dir.path().join(".git/hooks/post-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(&hook, "#!/bin/sh\nkill -KILL \"$PPID\"\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    let id = opened(&mut service, dir.path());
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    let result = request(
        &mut service,
        "operation.start",
        json!({
            "operationId":uuid::Uuid::new_v4().to_string(),
            "repoId":id,
            "expectedSnapshot":status["snapshot"],
            "action":{"kind":"commit","message":"completed before signal"}
        }),
    )
    .unwrap();
    assert_eq!(result["state"], "outcome_unknown");
    assert_eq!(result["error"]["code"], "OUTCOME_UNKNOWN");
    assert!(!git(dir.path(), &["rev-parse", "--verify", "HEAD"]).is_empty());
}

#[test]
fn cli_discard_untracked_is_literal_and_preserves_other_files_and_index() {
    let dir = setup();
    fs::write(dir.path().join("tracked"), "base\n").unwrap();
    git(dir.path(), &["add", "tracked"]);
    git(dir.path(), &["commit", "-qm", "base"]);
    fs::write(dir.path().join("tracked"), "staged\n").unwrap();
    git(dir.path(), &["add", "tracked"]);
    fs::write(dir.path().join("new[1].txt"), "remove").unwrap();
    fs::write(dir.path().join("new1.txt"), "keep").unwrap();
    fs::create_dir(dir.path().join("directory")).unwrap();
    fs::write(dir.path().join("directory/keep"), "keep").unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, dir.path());
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    let entry = status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"]["display"] == "new[1].txt")
        .unwrap()["entryId"]
        .clone();
    operation(
        &mut service,
        &id,
        json!({"kind":"discard","entryIds":[entry],"source":"index"}),
    );
    assert!(!dir.path().join("new[1].txt").exists());
    assert_eq!(fs::read(dir.path().join("new1.txt")).unwrap(), b"keep");
    assert_eq!(git(dir.path(), &["show", ":tracked"]), b"staged\n");
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    assert!(
        status["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["path"]["display"] == "directory/keep")
    );
    // An old collapsed directory selection cannot silently widen this action.
    let entry = super::super::tokens::EntryRef::new(&[b"directory/".to_vec()]).encode();
    let rejected = request(&mut service,"operation.start",json!({"repoId":id,"operationId":uuid::Uuid::new_v4().to_string(),"expectedSnapshot":status["snapshot"],"action":{"kind":"discard","entryIds":[entry],"source":"index"}})).unwrap();
    assert_eq!(rejected["error"]["code"], "STALE_SNAPSHOT");
    assert_eq!(
        fs::read(dir.path().join("directory/keep")).unwrap(),
        b"keep"
    );
}

#[cfg(unix)]
#[test]
fn unreadable_files_do_not_fail_status() {
    use std::os::unix::fs::PermissionsExt;
    let dir = setup();
    fs::write(dir.path().join("tracked"), "before\n").unwrap();
    fs::create_dir(dir.path().join("locked")).unwrap();
    fs::write(dir.path().join("locked/child"), "x").unwrap();
    git(dir.path(), &["add", "tracked"]);
    git(dir.path(), &["commit", "-qm", "initial"]);
    // Modify after committing so the status walk must open the file, then
    // revoke everything: the open and the directory listing both fail.
    fs::write(dir.path().join("tracked"), "after\n").unwrap();
    let locked = dir.path().join("locked");
    let tracked = dir.path().join("tracked");
    fs::set_permissions(&tracked, fs::Permissions::from_mode(0o000)).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let mut cli = Service::default();
    let path = WirePath::new(dir.path().as_os_str().as_bytes());
    let opened = request(&mut cli, "repo.open", json!({"path":path})).unwrap();
    let first = request(&mut cli, "repo.status", json!({"repoId":opened["repoId"]})).unwrap();
    let second = request(&mut cli, "repo.status", json!({"repoId":opened["repoId"]})).unwrap();
    // The page renders instead of failing with IO_ERROR, and the
    // fingerprint is stable across scans. (As root the files stay readable;
    // the assertions hold on that path too.)
    assert!(first["entries"].as_array().is_some());
    assert_eq!(first["snapshot"], second["snapshot"]);
    fs::set_permissions(&tracked, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn cli_stash_pop_conflict_keeps_saved_entry() {
    let dir = setup();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    fs::write(dir.path().join("file"), "base\n").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let id = opened(&mut service, dir.path());
    fs::write(dir.path().join("file"), "stashed\n").unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"stash.save","message":"conflict fixture"}),
    );
    let before = request(&mut service, "repo.stashes", json!({"repoId":id})).unwrap();
    fs::write(dir.path().join("file"), "new committed content\n").unwrap();
    git(dir.path(), &["commit", "-qam", "divergent change"]);
    let result = operation(
        &mut service,
        &id,
        json!({
            "kind":"stash.pop", "oid":before["entries"][0]["oid"], "index":0,
            "expectedToken":before["metadata"]["listToken"], "reinstateIndex":false
        }),
    );
    assert_eq!(result["state"], "needs_resolution");
    let after = request(&mut service, "repo.stashes", json!({"repoId":id})).unwrap();
    assert_eq!(after["entries"], before["entries"]);
    assert_eq!(
        after["metadata"]["listToken"],
        before["metadata"]["listToken"]
    );
    assert!(!git(dir.path(), &["ls-files", "-u"]).is_empty());
    let content = fs::read_to_string(dir.path().join("file")).unwrap();
    assert!(content.contains("stashed") && content.contains("new committed content"));
}

#[test]
fn cli_stash_drop_rejects_changed_list_then_drops_only_reviewed_entry() {
    let dir = setup();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    fs::write(dir.path().join("file"), "base\n").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "base"]);
    let id = opened(&mut service, dir.path());
    fs::write(dir.path().join("file"), "first\n").unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"stash.save","message":"first"}),
    );
    let first = request(&mut service, "repo.stashes", json!({"repoId":id})).unwrap();
    fs::write(dir.path().join("file"), "second\n").unwrap();
    operation(
        &mut service,
        &id,
        json!({"kind":"stash.save","message":"second"}),
    );
    let before = request(&mut service, "repo.stashes", json!({"repoId":id})).unwrap();
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    let rejected = request(&mut service, "operation.start", json!({
        "operationId":uuid::Uuid::new_v4(),"repoId":id,"expectedSnapshot":status["snapshot"],
        "action":{"kind":"stash.drop","oid":first["entries"][0]["oid"],"index":0,"expectedToken":first["metadata"]["listToken"]}
    })).unwrap();
    assert_eq!(rejected["state"], "failed");
    let unchanged = request(&mut service, "repo.stashes", json!({"repoId":id})).unwrap();
    assert_eq!(unchanged["entries"], before["entries"]);
    operation(
        &mut service,
        &id,
        json!({"kind":"stash.drop","oid":before["entries"][1]["oid"],"index":1,"expectedToken":before["metadata"]["listToken"]}),
    );
    let after = request(&mut service, "repo.stashes", json!({"repoId":id})).unwrap();
    assert_eq!(after["entries"].as_array().unwrap().len(), 1);
    assert_eq!(after["entries"][0]["oid"], before["entries"][0]["oid"]);
    assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"base\n");
    assert!(git(dir.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn cli_write_errors_explain_duplicate_names_and_missing_identity() {
    let dir = setup();
    fs::write(dir.path().join("file"), "base\n").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "base"]);
    git(dir.path(), &["tag", "existing"]);
    git(dir.path(), &["branch", "existing"]);
    for (args, code) in [
        (vec!["tag", "existing", "HEAD"], "TAG_EXISTS"),
        (vec!["branch", "existing", "HEAD"], "BRANCH_EXISTS"),
        (
            vec![
                "-c",
                "user.name=",
                "-c",
                "user.email=",
                "tag",
                "-a",
                "identity-check",
                "HEAD",
                "-m",
                "annotation",
            ],
            "IDENTITY_REQUIRED",
        ),
    ] {
        let arguments = args
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let error = command::write(dir.path(), &arguments, vec![]).unwrap_err();
        assert_eq!(error.code, code, "{}", error.message);
        assert!(
            !error.message.contains("fatal:"),
            "Raw stderr should not be exposed"
        );
    }
    assert_eq!(git(dir.path(), &["tag", "--list"]), b"existing\n");
    assert!(git(dir.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn cli_tag_delete_rejects_moved_object_and_preserves_worktree() {
    let dir = setup();
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    fs::write(dir.path().join("file"), "first\n").unwrap();
    git(dir.path(), &["add", "file"]);
    git(dir.path(), &["commit", "-qm", "first"]);
    git(dir.path(), &["tag", "reviewed"]);
    let old = String::from_utf8(git(dir.path(), &["rev-parse", "refs/tags/reviewed"]))
        .unwrap()
        .trim()
        .to_owned();
    fs::write(dir.path().join("file"), "second\n").unwrap();
    git(dir.path(), &["commit", "-qam", "second"]);
    git(dir.path(), &["tag", "-f", "reviewed"]);
    let current = String::from_utf8(git(dir.path(), &["rev-parse", "refs/tags/reviewed"]))
        .unwrap()
        .trim()
        .to_owned();
    let id = opened(&mut service, dir.path());
    let status = request(&mut service, "repo.status", json!({"repoId":id})).unwrap();
    let result = request(&mut service, "operation.start", json!({"operationId":uuid::Uuid::new_v4(),"repoId":id,"expectedSnapshot":status["snapshot"],"action":{"kind":"tag.delete","name":"reviewed","expectedOid":old}})).unwrap();
    assert_eq!(result["state"], "failed");
    assert_eq!(result["error"]["code"], "STALE_REFERENCE");
    assert_eq!(
        String::from_utf8(git(dir.path(), &["rev-parse", "refs/tags/reviewed"]))
            .unwrap()
            .trim(),
        current
    );
    operation(
        &mut service,
        &id,
        json!({"kind":"tag.delete","name":"reviewed","expectedOid":current}),
    );
    assert!(git(dir.path(), &["tag", "--list"]).is_empty());
    assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"second\n");
    assert!(git(dir.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn cli_worktree_removal_preserves_dirty_and_locked_checkouts() {
    let root = setup();
    fs::write(root.path().join("tracked"), "baseline\n").unwrap();
    git(root.path(), &["add", "tracked"]);
    git(root.path(), &["commit", "-qm", "initial"]);
    let parent = tempfile::tempdir().unwrap();
    let linked = parent.path().join("protected");
    git(
        root.path(),
        &[
            "worktree",
            "add",
            "-b",
            "protected",
            linked.to_str().unwrap(),
        ],
    );
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, root.path());
    let remove = |service: &mut Service| {
        let page = request(service, "repo.worktrees", json!({"repoId":id})).unwrap();
        request(service, "operation.start", json!({"operationId":uuid::Uuid::new_v4().to_string(),"repoId":id,"expectedSnapshot":page["snapshot"],"action":{"kind":"worktree.remove","name":"protected"}})).unwrap()
    };
    fs::write(linked.join("untracked"), "preserve me\n").unwrap();
    assert_eq!(remove(&mut service)["state"], "failed");
    assert_eq!(
        fs::read(linked.join("untracked")).unwrap(),
        b"preserve me\n"
    );
    fs::remove_file(linked.join("untracked")).unwrap();
    git(root.path(), &["worktree", "lock", linked.to_str().unwrap()]);
    assert_eq!(remove(&mut service)["state"], "failed");
    assert!(linked.join("tracked").exists());
    git(
        root.path(),
        &["worktree", "unlock", linked.to_str().unwrap()],
    );
    assert_eq!(remove(&mut service)["state"], "succeeded");
    assert!(!linked.exists());
    assert!(!git(root.path(), &["branch", "--list", "protected"]).is_empty());
    assert!(git(root.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn cli_worktree_text_filter_matches_branch_and_keeps_cursor_scope() {
    let root = setup();
    fs::write(root.path().join("tracked"), "baseline\n").unwrap();
    git(root.path(), &["add", "tracked"]);
    git(root.path(), &["commit", "-qm", "initial"]);
    let parent = tempfile::tempdir().unwrap();
    for (path, branch) in [
        ("checkout-a", "feature/needle-a"),
        ("checkout-b", "feature/needle-b"),
    ] {
        git(
            root.path(),
            &[
                "worktree",
                "add",
                "-b",
                branch,
                parent.path().join(path).to_str().unwrap(),
            ],
        );
    }
    let home = tempfile::tempdir().unwrap();
    let mut service = writable(home.path());
    let id = opened(&mut service, root.path());
    let first = request(
        &mut service,
        "repo.worktrees",
        json!({"repoId":id,"filter":"NEEDLE","pageSize":1}),
    )
    .unwrap();
    assert_eq!(first["entries"].as_array().unwrap().len(), 1);
    assert_eq!(first["metadata"]["matchingEntries"], 2);
    assert_eq!(first["metadata"]["totalEntries"], 3);
    assert_eq!(first["metadata"]["current"]["current"], true);
    let second = request(
        &mut service,
        "repo.worktrees",
        json!({"repoId":id,"filter":"NEEDLE","pageSize":1,"cursor":first["nextCursor"]}),
    )
    .unwrap();
    assert_eq!(second["entries"].as_array().unwrap().len(), 1);
    assert_ne!(first["entries"][0]["path"], second["entries"][0]["path"]);
    assert!(second["nextCursor"].is_null());
    assert!(
        request(
            &mut service,
            "repo.worktrees",
            json!({"repoId":id,"filter":"checkout-a","cursor":first["nextCursor"]})
        )
        .is_err()
    );
    let path = request(
        &mut service,
        "repo.worktrees",
        json!({"repoId":id,"filter":"checkout-a"}),
    )
    .unwrap();
    assert_eq!(path["entries"].as_array().unwrap().len(), 1);
    assert_eq!(
        path["entries"][0]["head"]["name"]["display"],
        "refs/heads/feature/needle-a"
    );
}
