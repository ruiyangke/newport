/** Mirrors the agent's protocol.rs. Paths returned by Git must retain bytesB64. */
export interface GitPath {
  bytesB64: string;
  display: string;
}
export interface GitProject {
  id: string;
  serverId: string;
  name: string;
  path: GitPath;
}
export interface GitOperationReceipt {
  operationId: string;
  serverId: string;
  action: string;
  state:
    | "pending"
    | "succeeded"
    | "failed"
    | "rejected"
    | "needs_resolution"
    | "outcome_unknown";
}
export interface GitConnection {
  serverId: string;
  info: {
    capabilities: {
      methods: string[];
      actions: string[];
      features: string[];
      objectFormats: string[];
    };
    limits: {
      maxFrameBytes: number;
      maxInflight: number;
      maxPageItems: number;
      maxChunkBytes: number;
      streamWindow: number;
    };
  };
}
type Page = { repoId: string; pageSize?: number; cursor?: string | null };
/** Read operations never mutate repository state. */
export type GitReadRequest =
  | { method: "repo.open"; params: { path: GitPath } }
  | { method: "repo.close" | "repo.remotes"; params: { repoId: string } }
  | {
      method:
        | "repo.status"
        | "repo.branches"
        | "repo.worktrees"
        | "repo.stashes"
        | "repo.tags";
      params: Page;
    }
  | { method: "repo.history"; params: Page & { revision?: string } }
  | {
      method: "repo.remote_refs";
      params: Page & {
        remote: string;
        expectedToken: string;
        forPush?: boolean;
      };
    }
  | { method: "repo.blob"; params: { repoId: string; oid: string } }
  | {
      method: "repo.commit_files";
      params: Page & { commitOid: string; parentIndex?: number };
    }
  | {
      method: "repo.commit_diff";
      params: {
        repoId: string;
        commitOid: string;
        parentIndex?: number;
        path?: GitPath;
        contextLines?: number;
      };
    }
  | {
      method: "repo.diff";
      params: {
        repoId: string;
        snapshot: string;
        entryId: string;
        side: "head_to_index" | "index_to_worktree" | "head_to_worktree";
        contextLines?: number;
      };
    }
  | { method: "operation.get"; params: { operationId: string } };

export interface GitAuthor {
  name: string;
  email: string;
}
/** Every mutating action advertised by the Rust agent. Guards are mandatory. */
export type GitWriteAction =
  | {
      kind: "stage" | "unstage";
      entryIds: string[];
      /** Omitting `lines` keeps whole-hunk behavior and its payload hash. */
      hunks?: { ids: string[]; lines?: string[]; contextLines: number };
    }
  | {
      kind: "conflict.resolve";
      entryIds: string[];
      side: "base" | "ours" | "theirs";
      /** The blob that side held when the conflict was read, or null if absent. */
      expectedOid: string | null;
    }
  | {
      kind: "discard";
      entryIds: string[];
      source: "index" | "head";
      /** Restricts the discard to selected hunks or lines of one file. */
      hunks?: { ids: string[]; lines?: string[]; contextLines: number };
    }
  | { kind: "commit"; message: string; author?: GitAuthor }
  | {
      kind: "commit.amend";
      expectedOid: string;
      message: string;
      author?: GitAuthor;
      committer?: GitAuthor;
    }
  | { kind: "branch.create"; name: string; startOid: string }
  | {
      kind: "branch.rename";
      name: string;
      newName: string;
      expectedOid: string;
    }
  | {
      kind: "branch.delete";
      name: string;
      expectedOid: string;
      force?: boolean;
    }
  | {
      kind: "branch.set_upstream";
      name: string;
      expectedOid: string;
      expectedToken: string;
      upstream: string | null;
    }
  | {
      kind: "checkout";
      target:
        | { kind: "branch"; name: string; expectedOid: string }
        | { kind: "detached"; oid: string };
    }
  | { kind: "remote.add"; name: string; url: string }
  | {
      kind: "remote.rename";
      name: string;
      newName: string;
      expectedToken: string;
    }
  | { kind: "remote.set_url"; name: string; url: string; expectedToken: string }
  | { kind: "remote.remove"; name: string; expectedToken: string }
  | { kind: "fetch"; remote: string; expectedToken: string; prune?: boolean }
  | {
      kind: "push";
      remote: string;
      expectedToken: string;
      branch: string;
      expectedOid: string;
      destinationBranch: string;
    }
  | {
      kind: "push.with_lease";
      remote: string;
      expectedToken: string;
      branch: string;
      expectedOid: string;
      destinationBranch: string;
      expectedRemoteOid: string;
    }
  | {
      kind: "branch.delete_remote";
      remote: string;
      expectedToken: string;
      branch: string;
      expectedOid: string;
    }
  | { kind: "merge" | "merge.fast_forward"; targetOid: string }
  | {
      kind: "pull.fast_forward";
      remote: string;
      expectedToken: string;
      remoteBranch: string;
    }
  | { kind: "merge.abort" | "integration.abort" | "integration.skip" }
  | { kind: "integration.continue"; message?: string; author?: GitAuthor }
  | {
      kind: "cherry_pick" | "revert";
      targetOid: string;
      mainline?: number;
      author?: GitAuthor;
    }
  | {
      kind: "rebase";
      upstreamOid: string;
      ontoOid?: string;
      committer?: GitAuthor;
    }
  | {
      kind: "reset";
      targetOid: string;
      expectedOid: string;
      mode: "soft" | "mixed" | "hard";
    }
  | {
      kind: "stash.save";
      message?: string;
      includeUntracked?: boolean;
      keepIndex?: boolean;
      author?: GitAuthor;
    }
  | {
      kind: "stash.apply" | "stash.pop";
      oid: string;
      index?: number;
      expectedToken: string;
      reinstateIndex?: boolean;
    }
  | { kind: "stash.drop"; oid: string; index?: number; expectedToken: string }
  | {
      kind: "tag.create";
      name: string;
      targetOid: string;
      annotation?: { message: string; author?: GitAuthor };
    }
  | { kind: "tag.delete"; name: string; expectedOid: string }
  | {
      kind: "tag.push" | "tag.delete_remote";
      remote: string;
      expectedToken: string;
      name: string;
      expectedOid: string;
    }
  | {
      kind: "worktree.add";
      name: string;
      path: GitPath;
      branch: string;
      /** The branch's commit; with `newBranch`, the commit it starts at. */
      expectedOid: string;
      locked?: boolean;
      /** Create `branch` for this worktree (`git worktree add -b`). */
      newBranch?: boolean;
    }
  | {
      kind: "worktree.remove" | "worktree.prune" | "worktree.unlock";
      name: string;
    }
  | { kind: "worktree.lock"; name: string; reason?: string }
  | { kind: "worktree.repair"; name: string; path: GitPath };
export type GitWorktreeAction = Extract<
  GitWriteAction,
  { kind: `worktree.${string}` }
>;
export type GitWriteRequest = {
  method: "operation.start";
  params: {
    operationId: string;
    repoId: string;
    expectedSnapshot: string;
    action: GitWriteAction;
  };
};
export type GitBootstrapRequest =
  | {
      method: "repo.init";
      params: { operationId: string; path: GitPath; initialBranch: string };
    }
  | {
      method: "repo.clone";
      params: {
        operationId: string;
        url: string;
        path: GitPath;
        branch?: string;
        bare?: boolean;
      };
    };
export type GitRequest = GitReadRequest | GitWriteRequest | GitBootstrapRequest;

export function isGitMutation(
  request: GitRequest,
): request is GitWriteRequest | GitBootstrapRequest {
  return (
    request.method === "operation.start" ||
    request.method === "repo.init" ||
    request.method === "repo.clone"
  );
}

/** Only for newly entered UTF-8 paths; never reconstruct a returned path from display. */
export function gitPath(display: string): GitPath {
  const bytes = new TextEncoder().encode(display);
  if (!bytes.length || bytes.length > 4096 || bytes.includes(0)) {
    throw new Error(
      "Enter a path between 1 and 4096 bytes without null characters.",
    );
  }
  return { display, bytesB64: btoa(String.fromCharCode(...bytes)) };
}
