//! Benchmarks disposable repositories through the production MessagePack codec.
//! Usage: cargo run --release --example git_benchmark -- --size small --rounds 3 --output report.json
use newport_agent::git::protocol::{self, Message, Path as WirePath};
use serde_json::{json, Value};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use uuid::Uuid;

struct Agent {
    child: Child,
    input: ChildStdin,
    receiver: mpsc::Receiver<io::Result<(Message, usize)>>,
    delay: Duration,
}
impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Agent {
    fn start(binary: &Path, home: &Path, latency: f64) -> Result<Self, Box<dyn std::error::Error>> {
        let mut child = Command::new(binary)
            .args(["git-rpc", "--stdio"])
            .env("HOME", home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env_remove("NEWPORT_GIT_TIMINGS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        let (tx, receiver) = mpsc::sync_channel(16);
        std::thread::spawn(move || loop {
            let result = (|| {
                let mut header = [0; 5];
                output.read_exact(&mut header)?;
                let len = protocol::payload_length(header)?;
                let mut body = vec![0; len];
                output.read_exact(&mut body)?;
                Ok((protocol::decode(&body)?, len + 5))
            })();
            let failed = result.is_err();
            if tx.send(result).is_err() || failed {
                break;
            }
        });
        let mut agent = Self {
            child,
            input,
            receiver,
            delay: Duration::ZERO,
        };
        match agent.receive()?.0 {
            Message::Hello { versions, .. } if versions.contains(&protocol::VERSION) => {}
            _ => return Err("Incompatible agent handshake".into()),
        }
        agent.send(&Message::Initialize {
            id: Uuid::new_v4().to_string(),
            version: protocol::VERSION,
            client_id: Uuid::new_v4().to_string(),
            client_version: "benchmark".into(),
            command_logs: false,
        })?;
        if !matches!(agent.receive()?.0, Message::Ready { .. }) {
            return Err("Agent did not initialize".into());
        }
        agent.delay = Duration::from_secs_f64(latency / 1000.);
        Ok(agent)
    }
    fn receive(&self) -> Result<(Message, usize), Box<dyn std::error::Error>> {
        Ok(self.receiver.recv_timeout(Duration::from_secs(60))??)
    }
    fn send(&mut self, message: &Message) -> io::Result<usize> {
        let frame = protocol::encode(message)?;
        std::thread::sleep(self.delay);
        self.input.write_all(&frame)?;
        self.input.flush()?;
        Ok(frame.len())
    }
    fn call(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(Value, Value), Box<dyn std::error::Error>> {
        let id = Uuid::new_v4().to_string();
        let start = Instant::now();
        let mut sent = self.send(&Message::Request {
            id: id.clone(),
            method: method.into(),
            params,
        })?;
        let (mut received, mut frames, mut chunks) = (0, 0, 0);
        loop {
            let (message, len) = self.receive()?;
            received += len;
            frames += 1;
            match message {
                Message::Response {
                    id: reply,
                    result,
                    error,
                } => {
                    if reply != id {
                        return Err("Mismatched response".into());
                    }
                    if let Some(error) = error {
                        return Err(format!("{method}: {}: {}", error.code, error.message).into());
                    }
                    let result = result.ok_or("Missing result")?;
                    let sample = json!({"elapsedMs":start.elapsed().as_secs_f64()*1000.,"requestBytes":sent,"responseBytes":received,"responseFrames":frames,"acknowledgementFrames":chunks,"rows":result["entries"].as_array().map(Vec::len)});
                    return Ok((result, sample));
                }
                Message::Chunk {
                    id: reply,
                    stream_id,
                    seq,
                    ..
                } => {
                    if reply != id {
                        return Err("Mismatched stream".into());
                    }
                    chunks += 1;
                    sent += self.send(&Message::Ack { stream_id, seq })?;
                }
                Message::Begin { id: reply, .. } if reply == id => {}
                _ => return Err("Unexpected frame".into()),
            }
        }
    }
}
fn git(
    root: &Path,
    args: &[&str],
    input: Option<&[u8]>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input)?;
    } else {
        drop(child.stdin.take());
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(String::from_utf8(out.stdout)?.trim().into())
}
fn fixture(root: &Path, size: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let (files, commits, refs) = match size {
        "small" => (32, 100, 20),
        "medium" => (500, 2000, 200),
        "large" => (5000, 20000, 2000),
        _ => return Err("size must be small, medium or large".into()),
    };
    git(
        root,
        &["init", "-q", "--initial-branch=main", "--template="],
        None,
    )?;
    git(root, &["config", "user.name", "Benchmark"], None)?;
    git(
        root,
        &["config", "user.email", "benchmark@example.test"],
        None,
    )?;
    let mut stream = Vec::new();
    for n in 0..commits {
        let message = format!("Commit {n}\n{}\n", "x".repeat(1024));
        write!(stream,"commit refs/heads/main\nmark :{}\ncommitter Benchmark <benchmark@example.test> {} +0000\ndata {}\n{}",n+1,1700000000+n,message.len(),message)?;
        if n == 0 {
            for f in 0..files {
                writeln!(stream, "M 100644 inline file-{f:05}\ndata 7\nbefore\n")?;
            }
            let content = "before\n".repeat(10000);
            write!(
                stream,
                "M 100644 inline large.txt\ndata {}\n{}\n",
                content.len(),
                content
            )?;
        }
        writeln!(stream)?;
    }
    for n in 0..refs {
        let message = format!("Release {n}\n{}\n", "x".repeat(8192));
        write!(stream,"tag v{n:05}\nfrom :{commits}\ntagger Benchmark <benchmark@example.test> 1700000000 +0000\ndata {}\n{}\n",message.len(),message)?;
    }
    git(root, &["fast-import", "--quiet"], Some(&stream))?;
    git(root, &["reset", "--hard", "HEAD"], None)?;
    let head = git(root, &["rev-parse", "HEAD"], None)?;
    let refs_input = (0..refs)
        .map(|n| format!("create refs/heads/branch-{n:05} {head}\n"))
        .collect::<String>();
    git(
        root,
        &["update-ref", "--stdin"],
        Some(refs_input.as_bytes()),
    )?;
    // A populated stash list and local-only remote keep selected-detail calls meaningful.
    std::fs::write(root.join("file-00000"), "stash\n")?;
    git(root, &["stash", "push", "-m", "benchmark stash"], None)?;
    git(
        root,
        &[
            "remote",
            "add",
            "origin",
            root.to_str().ok_or("non-UTF8 fixture path")?,
        ],
        None,
    )?;
    for f in 0..files {
        std::fs::write(root.join(format!("file-{f:05}")), "after\n")?;
    }
    std::fs::write(root.join("large.txt"), "after\n".repeat(10000))?;
    Ok(
        json!({"trackedFiles":files+1,"commits":commits,"branches":refs+1,"annotatedTags":refs,"rootCommit":git(root,&["rev-list","--max-parents=0","HEAD"],None)?,"head":head}),
    )
}
fn measured(
    agent: &mut Agent,
    samples: &mut Vec<Value>,
    method: &str,
    params: Value,
    label: &str,
    round: usize,
) -> Result<Value, Box<dyn std::error::Error>> {
    let (value, mut sample) = agent.call(method, params)?;
    sample["method"] = json!(method);
    sample["operation"] = json!(label);
    sample["round"] = json!(round);
    samples.push(sample);
    Ok(value)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = std::collections::HashMap::new();
    let mut args = std::env::args().skip(1);
    while let Some(key) = args.next() {
        if !["--agent", "--size", "--rounds", "--latency-ms", "--output"].contains(&key.as_str()) {
            return Err(format!("Unknown argument {key}").into());
        }
        options.insert(key, args.next().ok_or("Missing option value")?);
    }
    let binary = options
        .get("--agent")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/newport-agent")
        });
    let size = options.get("--size").map(String::as_str).unwrap_or("small");
    let rounds: usize = options
        .get("--rounds")
        .map(String::as_str)
        .unwrap_or("3")
        .parse()?;
    let latency: f64 = options
        .get("--latency-ms")
        .map(String::as_str)
        .unwrap_or("0")
        .parse()?;
    if rounds == 0 || !latency.is_finite() || latency < 0. {
        return Err("Invalid rounds or latency".into());
    }
    let output = options.get("--output").ok_or("--output is required")?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)?;
    let home = temp.path().join("home");
    std::fs::create_dir(&home)?;
    let dimensions = fixture(&repo, size)?;
    let mut samples = Vec::new();
    for round in 0..rounds {
        let mut agent = Agent::start(&binary, &home, latency)?;
        let opened = measured(
            &mut agent,
            &mut samples,
            "repo.open",
            json!({"path":WirePath::new(repo.as_os_str().as_encoded_bytes())}),
            "open",
            round,
        )?;
        let id = &opened["repoId"];
        for method in [
            "status",
            "history",
            "branches",
            "tags",
            "worktrees",
            "stashes",
            "remote_names",
            "commit_files",
        ] {
            let mut params = json!({"repoId":id,"pageSize":20});
            if method == "commit_files" {
                params["commitOid"] = dimensions["rootCommit"].clone();
            }
            if method == "history" || method == "tags" {
                params["messageBytes"] = json!(512);
            }
            let page = measured(
                &mut agent,
                &mut samples,
                &format!("repo.{method}"),
                params.clone(),
                &format!("{method}.first"),
                round,
            )?;
            if page["nextCursor"].is_string() {
                params["cursor"] = page["nextCursor"].clone();
                measured(
                    &mut agent,
                    &mut samples,
                    &format!("repo.{method}"),
                    params,
                    &format!("{method}.next"),
                    round,
                )?;
            }
            if method == "tags" {
                measured(
                    &mut agent,
                    &mut samples,
                    "repo.tag",
                    json!({"repoId":id,"oid":page["entries"][0]["oid"]["hex"]}),
                    "tag.selected",
                    round,
                )?;
            }
        }
        measured(
            &mut agent,
            &mut samples,
            "repo.status_summary",
            json!({"repoId":id}),
            "status.summary",
            round,
        )?;
        measured(
            &mut agent,
            &mut samples,
            "repo.branches",
            json!({"repoId":id,"filter":"branch-000","pageSize":20}),
            "branches.filtered",
            round,
        )?;
        measured(
            &mut agent,
            &mut samples,
            "repo.remote_names",
            json!({"repoId":id,"filter":"origin","pageSize":20}),
            "remotes.filtered",
            round,
        )?;
        measured(
            &mut agent,
            &mut samples,
            "repo.remotes",
            json!({"repoId":id}),
            "remotes",
            round,
        )?;
        let remote = measured(
            &mut agent,
            &mut samples,
            "repo.remote",
            json!({"repoId":id,"name":"origin"}),
            "remote.selected",
            round,
        )?;
        measured(
            &mut agent,
            &mut samples,
            "repo.remote_refs",
            json!({"repoId":id,"remote":"origin","expectedToken":remote["token"],"pageSize":20,"filter":"branch-000"}),
            "remote.refs.filtered",
            round,
        )?;
        measured(
            &mut agent,
            &mut samples,
            "repo.commit",
            json!({"repoId":id,"commitOid":dimensions["head"]}),
            "commit.selected",
            round,
        )?;
        let status = measured(
            &mut agent,
            &mut samples,
            "repo.status",
            json!({"repoId":id,"filter":{"text":"large.txt"},"pageSize":20}),
            "status.filtered",
            round,
        )?;
        let entry = status["entries"]
            .as_array()
            .ok_or("No status entries")?
            .iter()
            .find(|e| e["path"]["display"] == "large.txt")
            .ok_or("Missing diff fixture")?;
        for method in ["repo.diff_page", "repo.diff"] {
            let mut params = json!({"repoId":id,"snapshot":status["snapshot"],"entryId":entry["entryId"],"side":"index_to_worktree"});
            if method.ends_with("_page") {
                params["pageSize"] = json!(100);
                params["maxBytes"] = json!(16384);
                params["lineEncoding"] = json!("tuple_v1");
            }
            let first = measured(
                &mut agent,
                &mut samples,
                method,
                params.clone(),
                method,
                round,
            )?;
            if first["nextCursor"].is_string() {
                params["cursor"] = first["nextCursor"].clone();
                measured(&mut agent, &mut samples, method, params, "diff.next", round)?;
            }
        }
        let oid = git(&repo, &["rev-parse", "HEAD:large.txt"], None)?;
        let blob = measured(
            &mut agent,
            &mut samples,
            "repo.blob_page",
            json!({"repoId":id,"oid":oid,"maxBytes":16384}),
            "blob.first",
            round,
        )?;
        if blob["nextCursor"].is_string() {
            measured(
                &mut agent,
                &mut samples,
                "repo.blob_page",
                json!({"repoId":id,"oid":oid,"maxBytes":16384,"cursor":blob["nextCursor"]}),
                "blob.next",
                round,
            )?;
        }
        measured(
            &mut agent,
            &mut samples,
            "repo.commit_diff_page",
            json!({"repoId":id,"commitOid":dimensions["rootCommit"],"path":WirePath::new(b"large.txt"),"pageSize":100,"maxBytes":16384,"lineEncoding":"tuple_v1"}),
            "commit.diff.first",
            round,
        )?;
        measured(
            &mut agent,
            &mut samples,
            "repo.close",
            json!({"repoId":id}),
            "close",
            round,
        )?;
    }
    use sha2::{Digest, Sha256};
    let report = json!({"transport":"local stdio, production MessagePack v4 codec","scope":"Disposable CLI read workloads; fixture construction excluded. Write correctness is covered by test-git-cli-contract.mjs.","simulatedLatencyMs":latency,"latencyModel":"Synchronous delay before every outbound frame; ACK pacing included, not a network throughput model","agentSha256":Sha256::digest(std::fs::read(&binary)?).iter().map(|b|format!("{b:02x}")).collect::<String>(),"size":size,"dimensions":dimensions,"samples":samples});
    if let Some(parent) = Path::new(output)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        output,
        format!("{}\n", serde_json::to_string_pretty(&report)?),
    )?;
    println!("Recorded {} RPC samples to {output}", samples.len());
    Ok(())
}
