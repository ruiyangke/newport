//! Replay captured agent responses through the validators used by GPUI.
use newport_client::{git_blob, git_diff_page, git_operation, git_page};
use newport_protocol::git as protocol;
use serde_json::Value;
use std::collections::BTreeSet;

#[test]
#[ignore = "requires a real agent trace; run cargo xtask test git-cli-contract"]
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
        let request: protocol::Request =
            serde_json::from_value(row["request"].clone()).expect("request");
        // A whole diff arrives as a stream, so the trace holds the diff it
        // carried rather than the reply that ends the stream.
        if matches!(
            request,
            protocol::Request::Diff { .. } | protocol::Request::CommitDiff { .. }
        ) {
            serde_json::from_value::<protocol::Diff>(response["diff"].clone())
                .map_err(|error| error.to_string())
                .and_then(|diff| diff.validate().map_err(|error| error.to_string()))
                .unwrap_or_else(|error| panic!("line {} ({method}): {error}", line + 1));
            methods.insert(method.to_owned());
            continue;
        }
        let reply = request
            .reply_kind()
            .deserialize_result(response.clone())
            .unwrap_or_else(|error| panic!("line {} ({method}): {error}", line + 1));
        let result = match (&request, &reply) {
            (protocol::Request::DiffPage { cursor, .. }, protocol::Reply::DiffPage(page))
            | (
                protocol::Request::CommitDiffPage { cursor, .. },
                protocol::Reply::CommitDiffPage(page),
            ) => git_diff_page::validate(page, method == "repo.diff_page", cursor.is_none()),
            (_, protocol::Reply::Status(page)) => git_page::validate_page(page),
            (_, protocol::Reply::History(page)) => git_page::validate_page(page),
            (_, protocol::Reply::CommitFiles(page)) => git_page::validate_page(page),
            (_, protocol::Reply::Branches(page)) => git_page::validate_page(page),
            (_, protocol::Reply::RemoteNames(page)) => git_page::validate_page(page),
            (_, protocol::Reply::RemoteRefs(page)) => git_page::validate_page(page),
            (_, protocol::Reply::Stashes(page)) => git_page::validate_page(page),
            (_, protocol::Reply::Tags(page)) => git_page::validate_page(page),
            (_, protocol::Reply::Worktrees(page)) => git_page::validate_page(page),
            (protocol::Request::BlobPage { oid, cursor, .. }, protocol::Reply::BlobPage(page)) => {
                git_page::validate_page(page).expect("blob page envelope");
                if cursor.is_none() {
                    let mut content = git_blob::Content::default();
                    content.oid = oid.clone();
                    content.append(page)
                } else {
                    Ok(())
                }
            }
            (_, protocol::Reply::Operation(_)) => {
                git_operation::confirmed(reply.clone(), params["operationId"].as_str().unwrap())
                    .map(|_| ())
            }
            _ => Ok(()),
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
