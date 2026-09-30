import type {
  GitBootstrapRequest,
  GitPath,
  GitReadRequest,
  GitRequest,
  GitWriteRequest,
} from "../domain/git";
import {
  decodeGitRepository,
  decodeGitStatus,
  decodeGitHistory,
  decodeGitDiff,
  decodeGitBranches,
  decodeGitCommitFiles,
  decodeGitOperation,
  decodeGitRemotes,
  decodeGitRemoteRefs,
  decodeGitStashes,
  decodeGitTags,
  decodeGitWorktrees,
  decodeGitBlob,
  GitResponseError,
} from "../domain/gitResponses";
import { isGitMutation } from "../domain/git";
import { GitOperationError, type GitSession } from "./gitSession";

/** Decode the remote boundary once; components consume validated domain objects. */
export class GitRepositoryClient {
  constructor(
    private readonly session: Pick<GitSession, "request" | "forget">,
  ) {}

  private async read<T>(
    request: GitRequest,
    decode: (value: unknown) => T,
  ): Promise<T> {
    const response = await this.session.request(request);
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
  status(repoId: string, cursor?: string) {
    return this.read(
      { method: "repo.status", params: { repoId, cursor } },
      decodeGitStatus,
    );
  }
  history(repoId: string, revision = "HEAD", cursor?: string) {
    return this.read(
      { method: "repo.history", params: { repoId, revision, cursor } },
      decodeGitHistory,
    );
  }
  async close(repoId: string): Promise<void> {
    await this.session.request({ method: "repo.close", params: { repoId } });
  }
  branches(repoId: string, cursor?: string) {
    return this.read(
      { method: "repo.branches", params: { repoId, cursor } },
      decodeGitBranches,
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
      { method: "repo.tags", params: { repoId, cursor } },
      decodeGitTags,
    );
  }
  worktrees(repoId: string, cursor?: string) {
    return this.read(
      { method: "repo.worktrees", params: { repoId, cursor } },
      decodeGitWorktrees,
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
