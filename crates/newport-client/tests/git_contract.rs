//! Replay captured agent responses through the validators used by GPUI.
//! The separate TypeScript oracle checks remaining field-level wire contracts.
use newport_client::{git_blob, git_diff_page, git_operation::Operation, git_page};
use serde_json::Value;
use std::collections::BTreeSet;

#[test]
#[ignore = "requires a real agent trace; run npm run test:git-cli-contract"]
fn real_agent_responses_satisfy_native_contracts() {
    let path = std::env::var("NEWPORT_GIT_TRACE_PATH").expect("agent trace path required");
    let trace = std::fs::read_to_string(path).expect("read agent trace");
    let mut methods = BTreeSet::new();
    let mut actions = BTreeSet::new();
    for (line, source) in trace.lines().enumerate() {
        let row: Value = serde_json::from_str(source).expect("trace JSON");
        let Some(response) = row.get("response") else {
            continue;
        };
        let method = row["request"]["method"].as_str().expect("request method");
        let params = &row["request"]["params"];
        let result = match method {
            "repo.diff_page" | "repo.commit_diff_page" => git_diff_page::decode(
                &mut response.clone(),
                method == "repo.diff_page",
                params["cursor"].is_null(),
            ),
            "repo.status" | "repo.history" | "repo.commit_files" | "repo.branches"
            | "repo.remote_names" | "repo.remote_refs" | "repo.stashes" | "repo.tags"
            | "repo.worktrees" => git_page::validate(response),
            "repo.blob_page" => {
                git_page::validate(response).expect("blob page envelope");
                if params["cursor"].is_null() {
                    let mut content = git_blob::Content::default();
                    content.oid = params["oid"].as_str().unwrap().into();
                    content.append(response)
                } else {
                    Ok(())
                }
            }
            "operation.start" | "operation.get" | "operation.review" | "repo.init"
            | "repo.clone" => {
                Operation::decode(response.clone(), params["operationId"].as_str().unwrap())
                    .map(|_| ())
            }
            "repo.open"
            | "repo.close"
            | "repo.status_summary"
            | "repo.commit"
            | "repo.remotes"
            | "repo.remote"
            | "repo.tag"
            | "repo.blob"
            | "repo.diff"
            | "repo.commit_diff" => Ok(()),
            _ => panic!("unhandled method: {method}"),
        };
        result.unwrap_or_else(|error| panic!("line {} ({method}): {error}", line + 1));
        methods.insert(method.to_owned());
        if method == "operation.start"
            && matches!(
                response["state"].as_str(),
                Some("succeeded" | "needs_resolution")
            )
        {
            actions.insert(params["action"]["kind"].as_str().unwrap().to_owned());
        }
    }
    assert_eq!(
        methods,
        EXPECTED_METHODS.iter().map(|v| (*v).to_owned()).collect()
    );
    assert_eq!(
        actions,
        EXPECTED_ACTIONS.iter().map(|v| (*v).to_owned()).collect()
    );
}

const EXPECTED_METHODS: &[&str] = &[
    "repo.open",
    "repo.close",
    "repo.status",
    "repo.status_summary",
    "repo.history",
    "repo.commit",
    "repo.branches",
    "repo.remotes",
    "repo.remote",
    "repo.remote_names",
    "repo.remote_refs",
    "repo.stashes",
    "repo.tags",
    "repo.tag",
    "repo.worktrees",
    "repo.blob",
    "repo.blob_page",
    "repo.diff",
    "repo.diff_page",
    "repo.commit_diff",
    "repo.commit_diff_page",
    "repo.commit_files",
    "operation.start",
    "operation.get",
    "operation.review",
    "repo.init",
    "repo.clone",
];

const EXPECTED_ACTIONS: &[&str] = &[
    "stage",
    "unstage",
    "discard",
    "conflict.resolve",
    "reset",
    "commit",
    "commit.amend",
    "branch.create",
    "branch.rename",
    "branch.delete",
    "branch.set_upstream",
    "branch.delete_remote",
    "checkout",
    "remote.add",
    "remote.rename",
    "remote.set_url",
    "remote.remove",
    "config.set",
    "fetch",
    "push",
    "push.with_lease",
    "pull.fast_forward",
    "merge",
    "merge.fast_forward",
    "merge.abort",
    "integration.continue",
    "integration.abort",
    "integration.skip",
    "cherry_pick",
    "revert",
    "rebase",
    "stash.save",
    "stash.apply",
    "stash.pop",
    "stash.drop",
    "tag.create",
    "tag.delete",
    "tag.push",
    "tag.delete_remote",
    "worktree.add",
    "worktree.remove",
    "worktree.prune",
    "worktree.repair",
    "worktree.lock",
    "worktree.unlock",
];
