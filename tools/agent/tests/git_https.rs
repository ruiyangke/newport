//! Opt-in local TLS fixture, started by scripts/test-git-https.mjs.
use newport_agent::git::protocol::{self, Message, Path as WirePath};
use serde_json::{json, Value};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
};
struct Agent(Child);
impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Agent {
    fn send(&mut self, message: Message) {
        protocol::write(self.0.stdin.as_mut().unwrap(), &message).unwrap();
    }
    fn read(&mut self) -> Message {
        protocol::read(self.0.stdout.as_mut().unwrap()).unwrap()
    }
    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = uuid::Uuid::new_v4().to_string();
        self.send(Message::Request {
            id: id.clone(),
            method: method.into(),
            params,
        });
        match self.read() {
            Message::Response {
                id: reply,
                result: Some(value),
                error: None,
            } => {
                assert_eq!(reply, id);
                value
            }
            other => panic!("Unexpected reply: {other:?}"),
        }
    }
}
fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
#[test]
#[ignore = "Run with node scripts/test-git-https.mjs; disposable authenticated HTTPS fixture"]
fn authenticated_https_respects_repository_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let home = tempfile::tempdir().unwrap();
    git(
        root,
        &["init", "-q", "--initial-branch=main", "--template="],
    );
    git(root, &["config", "user.name", "Fixture"]);
    git(root, &["config", "user.email", "fixture@example.test"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("file"), "fixture\n").unwrap();
    git(root, &["add", "file"]);
    git(root, &["commit", "-qm", "fixture"]);
    let head = git(root, &["rev-parse", "HEAD"]);
    git(
        root,
        &[
            "remote",
            "add",
            "origin",
            &std::env::var("NEWPORT_GIT_HTTPS_URL").unwrap(),
        ],
    );
    git(
        root,
        &[
            "config",
            "http.sslCAInfo",
            &std::env::var("NEWPORT_GIT_HTTPS_CERT").unwrap(),
        ],
    );
    git(root,&["config","credential.helper","!f() { if [ \"$1\" = get ]; then printf 'username=fixture\\npassword=fixture-token\\n'; fi; }; f"]);
    let mut agent = Agent(
        Command::new(env!("CARGO_BIN_EXE_newport-agent"))
            .args(["git-rpc", "--stdio"])
            .env("HOME", home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    assert!(matches!(agent.read(), Message::Hello { .. }));
    agent.send(Message::Initialize {
        id: uuid::Uuid::new_v4().to_string(),
        version: protocol::VERSION,
        client_id: uuid::Uuid::new_v4().to_string(),
        client_version: "https-fixture".into(),
        command_logs: false,
    });
    assert!(matches!(agent.read(), Message::Ready { .. }));
    let opened = agent.call(
        "repo.open",
        json!({"path":WirePath::new(root.as_os_str().as_encoded_bytes())}),
    );
    let id = &opened["repoId"];
    let remote = agent.call("repo.remote", json!({"repoId":id,"name":"origin"}));
    for action in [
        json!({"kind":"push","remote":"origin","expectedToken":remote["token"],"branch":"main","expectedOid":head,"destinationBranch":"main"}),
        json!({"kind":"fetch","remote":"origin","expectedToken":remote["token"]}),
    ] {
        let status = agent.call("repo.status", json!({"repoId":id}));
        let outcome=agent.call("operation.start",json!({"operationId":uuid::Uuid::new_v4().to_string(),"repoId":id,"expectedSnapshot":status["snapshot"],"action":action}));
        assert_eq!(outcome["state"], "succeeded", "{outcome}");
    }
    assert_eq!(git(root, &["rev-parse", "refs/remotes/origin/main"]), head);
    // A bad helper must fail normally, never fall back to an interactive prompt.
    git(
        root,
        &[
            "config",
            "credential.helper",
            "!printf 'username=fixture\\npassword=wrong\\n'",
        ],
    );
    let status = agent.call("repo.status", json!({"repoId":id}));
    let outcome=agent.call("operation.start",json!({"operationId":uuid::Uuid::new_v4().to_string(),"repoId":id,"expectedSnapshot":status["snapshot"],"action":{"kind":"fetch","remote":"origin","expectedToken":remote["token"]}}));
    assert_eq!(outcome["state"], "failed", "{outcome}");
}
