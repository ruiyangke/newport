import type {
  GitBranchOptions,
  GitStatusFilter,
  GitWorktreeOptions,
  GitBootstrapRequest,
  GitPath,
  GitReadRequest,
  GitRequest,
  GitWriteRequest,
} from "../domain/git";
import {
  decodeGitRepository,
  decodeGitStatus,
  decodeGitStatusMetadata,
  decodeGitHistory,
  decodeGitCommit,
  decodeGitDiff,
  decodeGitCommitDiffPage,
  decodeGitWorkingDiffPage,
  validateInitialGitDiffPage,
  decodeGitBranches,
  decodeGitCommitFiles,
  decodeGitOperation,
  decodeGitRemotes,
  decodeGitRemote,
  decodeGitRemoteRefs,
  decodeGitRemoteNames,
  decodeGitStashes,
  decodeGitTags,
  decodeGitTag,
  decodeGitWorktrees,
  decodeGitBlob,
  decodeGitBlobPage,
  validateInitialGitBlobPage,
  GitResponseError,
} from "../domain/gitResponses";
import { isGitMutation } from "../domain/git";
import { GitOperationError, type GitSession } from "./gitSession";

/** Decode the remote boundary once; components consume validated domain objects. */
export class GitRepositoryClient {
  constructor(
    private readonly session: Pick<GitSession, "request" | "forget">,
    private readonly signal?: AbortSignal,
  ) {}

  /** A per-read view; cancellation never changes the shared session or writes. */
  withSignal(signal: AbortSignal) {
    return new GitRepositoryClient(this.session, signal);
  }

  private async read<T>(
    request: GitRequest,
    decode: (value: unknown) => T,
  ): Promise<T> {
    const response = await (this.signal
      ? this.session.request(request, this.signal)
      : this.session.request(request));
    try {
      return decode(response);
    } catch (error) {
      if (error instanceof GitResponseError) void this.session.forget();
      if (isGitMutation(request))
        throw new GitOperationError(
          request.params.operationId,
          "OUTCOME_UNKNOWN",
          "The write returned an invalid response. Check the saved operation outcome before trying again.",
        );
      throw error;
    }
  }

  open(path: GitPath) {
    return this.read(
      { method: "repo.open", params: { path } },
      decodeGitRepository,
    );
  }
  start(params: GitWriteRequest["params"]) {
    const captured = structuredClone(params);
    return this.read({ method: "operation.start", params: captured }, (value) =>
      decodeGitOperation(value, captured.operationId),
    );
  }
  bootstrap(request: GitBootstrapRequest) {
    const captured = structuredClone(request);
    return this.read(captured, (value) =>
      decodeGitOperation(value, captured.params.operationId),
    );
  }
  operation(operationId: string) {
    return this.read(
      { method: "operation.get", params: { operationId } },
      (value) => decodeGitOperation(value, operationId),
    );
  }
  statusSummary(target: string | GitPath) {
    return this.read(
      {
        method: "repo.status_summary",
        params:
          typeof target === "string" ? { repoId: target } : { path: target },
      },
      (value) => {
        const result = decodeGitStatusMetadata(value);
        if (result.totalEntries === null || result.truncated)
          throw new GitResponseError("complete status summary");
        return result;
      },
    );
  }
  status(repoId: string, cursor?: string, filter?: GitStatusFilter) {
    return this.read(
      {
        method: "repo.status",
        params: { repoId, cursor, ...(filter ? { filter } : {}) },
      },
      decodeGitStatus,
    );
  }
  history(repoId: string, revision = "HEAD", cursor?: string) {
    return this.read(
      {
        method: "repo.history",
        params: { repoId, revision, cursor, messageBytes: 512 },
      },
      decodeGitHistory,
    );
  }
  commit(repoId: string, commitOid: string) {
    return this.read(
      { method: "repo.commit", params: { repoId, commitOid } },
      (value) => {
        const commit = decodeGitCommit(value);
        if (commit.oid.hex !== commitOid.toLowerCase())
          throw new GitResponseError("requested commit");
        return commit;
      },
    );
  }
  async close(repoId: string): Promise<void> {
    await this.session.request({ method: "repo.close", params: { repoId } });
  }
  branches(repoId: string, cursor?: string, options?: GitBranchOptions) {
    return this.read(
      { method: "repo.branches", params: { ...options, repoId, cursor } },
      decodeGitBranches,
    );
  }
  remoteNames(
    params: Extract<GitReadRequest, { method: "repo.remote_names" }>["params"],
  ) {
    const expected = structuredClone(params);
    return this.read(
      { method: "repo.remote_names", params: expected },
      (value) => {
        const result = decodeGitRemoteNames(value);
        if (
          result.entries.length > (expected.pageSize ?? 100) ||
          (!expected.cursor &&
            result.nextCursor === null &&
            result.entries.length !== result.metadata.totalEntries)
        )
          throw new GitResponseError("remote names completeness");
        return result;
      },
    );
  }
  remote(repoId: string, name: string) {
    return this.read(
      { method: "repo.remote", params: { repoId, name } },
      (value) => {
        const remote = decodeGitRemote(value);
        if (remote.name !== name) throw new GitResponseError("remote identity");
        return remote;
      },
    );
  }
  remotes(repoId: string) {
    return this.read(
      { method: "repo.remotes", params: { repoId } },
      decodeGitRemotes,
    );
  }
  remoteRefs(
    params: Extract<GitReadRequest, { method: "repo.remote_refs" }>["params"],
  ) {
    const expected = structuredClone(params);
    return this.read(
      { method: "repo.remote_refs", params: expected },
      (value) => {
        const result = decodeGitRemoteRefs(value);
        if (
          result.metadata.remote !== expected.remote ||
          result.metadata.remoteToken !== expected.expectedToken ||
          result.metadata.forPush !== (expected.forPush ?? false)
        )
          throw new GitResponseError("requested remote advertisement");
        return result;
      },
    );
  }
  stashes(repoId: string, cursor?: string) {
    return this.read(
      { method: "repo.stashes", params: { repoId, cursor } },
      decodeGitStashes,
    );
  }
  tags(repoId: string, cursor?: string) {
    return this.read(
      { method: "repo.tags", params: { repoId, cursor, messageBytes: 512 } },
      decodeGitTags,
    );
  }
  tag(repoId: string, oid: string) {
    return this.read(
      { method: "repo.tag", params: { repoId, oid } },
      (value) => {
        const result = decodeGitTag(value);
        if (result.oid.hex !== oid.toLowerCase())
          throw new GitResponseError("requested tag");
        return result;
      },
    );
  }
  worktrees(repoId: string, cursor?: string, options?: GitWorktreeOptions) {
    const { atSnapshot, ...filters } = options ?? {};
    return this.read(
      {
        method: "repo.worktrees",
        params: {
          repoId,
          cursor,
          ...filters,
          ...(!cursor && atSnapshot !== undefined ? { atSnapshot } : {}),
        },
      },
      decodeGitWorktrees,
    );
  }
  blobPage(
    params: Extract<GitReadRequest, { method: "repo.blob_page" }>["params"],
  ) {
    const expected = structuredClone(params);
    return this.read(
      { method: "repo.blob_page", params: expected },
      (value) => {
        const result = decodeGitBlobPage(value);
        if (result.metadata.oid.hex !== expected.oid.toLowerCase())
          throw new GitResponseError("requested blob page");
        return expected.cursor ? result : validateInitialGitBlobPage(result);
      },
    );
  }
  blob(repoId: string, oid: string) {
    return this.read(
      { method: "repo.blob", params: { repoId, oid } },
      (value) => {
        const result = decodeGitBlob(value);
        if (result.oid.hex !== oid.toLowerCase())
          throw new GitResponseError("requested blob");
        return result;
      },
    );
  }
  commitFiles(
    params: Extract<GitReadRequest, { method: "repo.commit_files" }>["params"],
  ) {
    const expected = structuredClone(params);
    return this.read(
      { method: "repo.commit_files", params: expected },
      (value) => {
        const result = decodeGitCommitFiles(value);
        checkComparison(result.metadata, expected);
        return result;
      },
    );
  }
  diff(params: Extract<GitReadRequest, { method: "repo.diff" }>["params"]) {
    const expected = structuredClone(params);
    return this.read({ method: "repo.diff", params: expected }, (value) => {
      const result = decodeGitDiff(value);
      if (result.snapshot !== expected.snapshot || result.comparison !== null)
        throw new GitResponseError("working diff snapshot");
      return result;
    });
  }
  diffPage(
    params: Extract<GitReadRequest, { method: "repo.diff_page" }>["params"],
  ) {
    const expected = {
      ...structuredClone(params),
      lineEncoding: "tuple_v1" as const,
    };
    return this.read(
      { method: "repo.diff_page", params: expected },
      (value) => {
        const result = decodeGitWorkingDiffPage(value);
        if (
          result.metadata.sourceSnapshot !== expected.snapshot ||
          result.metadata.entryId !== expected.entryId ||
          result.metadata.side !== expected.side ||
          result.metadata.contextLines !== (expected.contextLines ?? 3)
        )
          throw new GitResponseError("working diff selection");
        return expected.cursor ? result : validateInitialGitDiffPage(result);
      },
    );
  }
  commitDiffPage(
    params: Extract<
      GitReadRequest,
      { method: "repo.commit_diff_page" }
    >["params"],
  ) {
    const expected = {
      ...structuredClone(params),
      lineEncoding: "tuple_v1" as const,
    };
    return this.read(
      { method: "repo.commit_diff_page", params: expected },
      (value) => {
        const result = decodeGitCommitDiffPage(value);
        checkComparison(result.metadata, expected);
        if (
          result.metadata.selectedPath.bytesB64 !== expected.path.bytesB64 ||
          result.metadata.contextLines !== (expected.contextLines ?? 3) ||
          result.entries.some(
            (file) =>
              file.oldPath?.bytesB64 !== expected.path.bytesB64 &&
              file.newPath?.bytesB64 !== expected.path.bytesB64,
          )
        )
          throw new GitResponseError("selected paged diff");
        return expected.cursor ? result : validateInitialGitDiffPage(result);
      },
    );
  }
  commitDiff(
    params: Extract<GitReadRequest, { method: "repo.commit_diff" }>["params"],
  ) {
    const expected = structuredClone(params);
    return this.read(
      { method: "repo.commit_diff", params: expected },
      (value) => {
        const result = decodeGitDiff(value);
        if (result.snapshot.toLowerCase() !== expected.commitOid.toLowerCase())
          throw new GitResponseError("commit diff snapshot");
        checkComparison(result.comparison, expected);
        if (
          expected.path &&
          (!result.files.length ||
            result.files.some(
              (file) =>
                file.oldPath?.bytesB64 !== expected.path?.bytesB64 &&
                file.newPath?.bytesB64 !== expected.path?.bytesB64,
            ))
        )
          throw new GitResponseError("selected diff file");
        return result;
      },
    );
  }
}

function checkComparison(
  comparison: ReturnType<typeof decodeGitDiff>["comparison"],
  expected: { commitOid: string; parentIndex?: number },
) {
  if (
    !comparison ||
    comparison.commitOid.hex !== expected.commitOid.toLowerCase() ||
    (comparison.parentIndex ?? 0) !== (expected.parentIndex ?? 0)
  ) {
    throw new GitResponseError("requested commit comparison");
  }
}
