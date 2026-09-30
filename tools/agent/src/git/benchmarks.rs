//! Reproducible local-agent read benchmark. Fixture setup is outside timings.
//! Run with --ignored --nocapture; JSON contains timings/counts, never repo data.
use super::*;
use std::time::Instant;

#[test]
#[ignore = "Explicit large-repository benchmark"]
fn large_repository_benchmark() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Repository::init(temp.path()).unwrap();
    for i in 0..2000 {
        fs::write(temp.path().join(format!("file-{i:05}")), "before\n").unwrap();
    }
    tests::commit(&repo, "initial");
    let tree = repo.head().unwrap().peel_to_tree().unwrap();
    let sig = git2::Signature::now("Benchmark", "benchmark@example.test").unwrap();
    for i in 0..3000 {
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            &format!("Commit {i}"),
            &tree,
            &[&parent],
        )
        .unwrap();
    }
    let tip = repo.head().unwrap().peel_to_commit().unwrap();
    for i in 0..1000 {
        repo.branch(&format!("branch-{i:04}"), &tip, false).unwrap();
        repo.tag_lightweight(&format!("tag-{i:04}"), tip.as_object(), false)
            .unwrap();
    }
    for i in 0..2000 {
        fs::write(temp.path().join(format!("file-{i:05}")), "after\n").unwrap();
    }
    let mut service = Service::default();
    let opened = match service
        .request(Request::Open {
            path: wire_path(temp.path()),
        })
        .unwrap()
    {
        Output::Json(v) => v,
        _ => unreachable!(),
    };
    let repo_id = opened["repoId"].as_str().unwrap().to_owned();
    let requests = [
        (
            "status.first",
            Request::Status {
                filter: None,
                repo_id: repo_id.clone(),
                page_size: 20,
                cursor: None,
            },
        ),
        (
            "history.first",
            Request::History {
                message_bytes: None,
                repo_id: repo_id.clone(),
                page_size: 20,
                cursor: None,
                revision: "HEAD".into(),
            },
        ),
        (
            "branches.first",
            Request::Branches {
                filter: String::new(),
                branch_kind: None,
                repo_id: repo_id.clone(),
                page_size: 20,
                cursor: None,
            },
        ),
        (
            "tags.first",
            Request::Tags {
                message_bytes: None,
                repo_id: repo_id.clone(),
                page_size: 20,
                cursor: None,
            },
        ),
    ];
    let mut samples = Vec::new();
    for (label, request) in requests {
        for round in 0..5 {
            let mut service = Service::default();
            let start = Instant::now();
            let Output::Json(value) = service.request(request.clone()).unwrap() else {
                panic!()
            };
            samples.push(json!({"operation":label,"round":round,"elapsedMs":start.elapsed().as_secs_f64()*1000.0,"responseJsonBytes":serde_json::to_vec(&value).unwrap().len(),"rows":value["entries"].as_array().unwrap().len()}));
            if label == "history.first" {
                for reconnect in [false, true] {
                    let request = Request::History {
                        message_bytes: None,
                        repo_id: repo_id.clone(),
                        revision: "HEAD".into(),
                        page_size: 20,
                        cursor: value["nextCursor"].as_str().map(str::to_owned),
                    };
                    let mut fresh = Service::default();
                    let target = if reconnect { &mut fresh } else { &mut service };
                    let start = Instant::now();
                    let Output::Json(page) = target.request(request).unwrap() else {
                        panic!()
                    };
                    samples.push(json!({"operation":if reconnect {"history.next.reconnect"} else {"history.next.cached"},"round":round,"elapsedMs":start.elapsed().as_secs_f64()*1000.0,"responseJsonBytes":serde_json::to_vec(&page).unwrap().len(),"rows":page["entries"].as_array().unwrap().len()}));
                }
            }
        }
    }
    let entries: Vec<_> = statuses(&repo)
        .unwrap()
        .iter()
        .take(30)
        .map(|e| EntryRef::new(&entry_paths(&e)).encode())
        .collect();
    for round in 0..5 {
        let start = Instant::now();
        let paths = Service::selected_paths_now(&repo, &entries).unwrap();
        samples.push(json!({"operation":"selection.validate_30","round":round,"elapsedMs":start.elapsed().as_secs_f64()*1000.0,"paths":paths.len()}));
    }
    let report = json!({"fixture":{"trackedFiles":2000,"changedFiles":2000,"commits":3001,"branches":1001,"tags":1000},"measurement":"agent computation plus response construction; no SSH or JSON serialization in elapsed time","samples":samples});
    if let Some(path) = std::env::var_os("NEWPORT_GIT_BENCHMARK_PATH") {
        fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    println!("{}", serde_json::to_string(&report).unwrap());
}
