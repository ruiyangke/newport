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
    assert_eq!(ACTIONS.len(), 44);
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
    let entry = status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"]["display"] == "directory/")
        .unwrap()["entryId"]
        .clone();
    let rejected = request(&mut service,"operation.start",json!({"repoId":id,"operationId":uuid::Uuid::new_v4().to_string(),"expectedSnapshot":status["snapshot"],"action":{"kind":"discard","entryIds":[entry],"source":"index"}})).unwrap();
    assert_eq!(rejected["error"]["code"], "UNSUPPORTED_CAPABILITY");
    assert_eq!(
        fs::read(dir.path().join("directory/keep")).unwrap(),
        b"keep"
    );
}
