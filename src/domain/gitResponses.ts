import type { GitPath } from "./git";

export class GitResponseError extends Error {
  readonly code = "PROTOCOL_ERROR";
  constructor(field: string) {
    super(`Invalid Git response (${field}). Reconnect before continuing.`);
  }
}
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new GitResponseError("object");
  return value as Record<string, unknown>;
}
function text(value: unknown): string {
  if (typeof value !== "string") throw new GitResponseError("text");
  return value;
}
function hunkId(value: unknown): string {
  const id = text(value);
  if (!/^[0-9a-f]{64}$/.test(id)) throw new GitResponseError("hunk ID");
  return id;
}
function flag(value: unknown): boolean {
  if (typeof value !== "boolean") throw new GitResponseError("boolean");
  return value;
}
function integer(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value))
    throw new GitResponseError("integer");
  return value;
}
function count(value: unknown): number {
  const n = integer(value);
  if (n < 0) throw new GitResponseError("count");
  return n;
}
function nullable<T>(value: unknown, decode: (value: unknown) => T): T | null {
  return value === null ? null : decode(value);
}
function array<T>(
  value: unknown,
  decode: (value: unknown) => T,
  max: number,
): T[] {
  if (!Array.isArray(value) || value.length > max)
    throw new GitResponseError("array");
  return value.map(decode);
}
/** WirePath also carries commit messages and diff content, including empty data. */
export function decodeGitBytes(value: unknown): GitPath {
  const v = object(value);
  const bytesB64 = text(v.bytesB64);
  // Canonical padded Base64, matching Rust's STANDARD encoder.
  if (
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(
      bytesB64,
    )
  )
    throw new GitResponseError("bytesB64");
  if (btoa(atob(bytesB64)) !== bytesB64) throw new GitResponseError("bytesB64");
  return { bytesB64, display: text(v.display) };
}
/** Object ID length follows the repository's hash algorithm, never the reverse. */
const OID_LENGTH = { sha1: 40, sha256: 64 } as const;
export type GitObjectFormat = keyof typeof OID_LENGTH;
export function decodeGitOid(value: unknown) {
  const v = object(value);
  const hex = text(v.hex);
  const algorithm = text(v.algorithm ?? v.format);
  if (!(algorithm in OID_LENGTH))
    throw new GitResponseError("object ID algorithm");
  const expected = OID_LENGTH[algorithm as GitObjectFormat];
  if (!new RegExp(`^[0-9a-f]{${expected}}$`).test(hex))
    throw new GitResponseError("object ID");
  return { algorithm: algorithm as GitObjectFormat, hex };
}
export function decodeGitHead(value: unknown) {
  const v = object(value);
  return {
    oid: nullable(v.oid, decodeGitOid),
    name: nullable(v.name, decodeGitBytes),
    detached: flag(v.detached),
    unborn: flag(v.unborn),
  };
}
function integration(value: unknown) {
  return nullable(value, (value) => {
    const v = object(value);
    return {
      kind: text(v.kind),
      managed: flag(v.managed),
      canContinue: flag(v.canContinue),
      canAbort: flag(v.canAbort),
      canSkip: v.canSkip === undefined ? false : flag(v.canSkip),
      position: optional(v.position, count),
      total: optional(v.total, count),
    };
  });
}
export function decodeGitRepository(value: unknown) {
  const v = object(value);
  const capabilities = object(v.capabilities);
  const objectFormat = text(v.objectFormat);
  if (!(objectFormat in OID_LENGTH))
    throw new GitResponseError("object format");
  return {
    repoId: text(v.repoId),
    commonRepoId: text(v.commonRepoId),
    root: decodeGitBytes(v.root),
    bare: flag(v.bare),
    objectFormat: objectFormat as GitObjectFormat,
    head: decodeGitHead(v.head),
    operationState: text(v.operationState),
    integration: integration(v.integration),
    capabilities: {
      readOnly: flag(capabilities.readOnly),
      workingTree: flag(capabilities.workingTree),
    },
  };
}
export interface GitPage<T, M> {
  snapshot: string;
  entries: T[];
  nextCursor: string | null;
  metadata: M;
}
function page<T, M>(
  value: unknown,
  entry: (v: unknown) => T,
  metadata: (v: unknown) => M,
): GitPage<T, M> {
  const v = object(value);
  return {
    snapshot: text(v.snapshot),
    entries: array(v.entries, entry, 200),
    nextCursor: nullable(v.nextCursor, text),
    metadata: metadata(v.metadata),
  };
}
function conflictSide(value: unknown) {
  return nullable(value, (value) => {
    const v = object(value);
    return {
      oid: decodeGitOid(v.oid),
      path: decodeGitBytes(v.path),
      mode: count(v.mode),
    };
  });
}
function statusEntry(value: unknown) {
  const v = object(value);
  return {
    entryId: text(v.entryId),
    path: nullable(v.path, decodeGitBytes),
    oldPath: nullable(v.oldPath, decodeGitBytes),
    flags: count(v.flags),
    staged: flag(v.staged),
    unstaged: flag(v.unstaged),
    untracked: flag(v.untracked),
    conflicted: flag(v.conflicted),
    conflict: nullable(v.conflict, (value) => {
      const c = object(value);
      return {
        base: conflictSide(c.base),
        ours: conflictSide(c.ours),
        theirs: conflictSide(c.theirs),
      };
    }),
  };
}
export function decodeGitStatus(value: unknown) {
  return page(value, statusEntry, (value) => {
    const v = object(value);
    if (v.basis !== "stored_refs") throw new GitResponseError("tracking basis");
    return {
      head: decodeGitHead(v.head),
      operationState: text(v.operationState),
      integration: integration(v.integration),
      ahead: nullable(v.ahead, count),
      behind: nullable(v.behind, count),
      upstreamRef: nullable(v.upstreamRef, decodeGitBytes),
      basis: "stored_refs" as const,
      // A working tree larger than one read can carry is reported, never hidden.
      // The cap is the agent's, so it is read rather than assumed.
      truncated: v.truncated === undefined ? false : flag(v.truncated),
      totalEntries: v.totalEntries === undefined ? null : count(v.totalEntries),
      entryLimit: v.entryLimit === undefined ? null : count(v.entryLimit),
    };
  });
}
function historyEntry(value: unknown) {
  const v = object(value);
  const author = object(v.author);
  return {
    oid: decodeGitOid(v.oid),
    parents: array(v.parents, decodeGitOid, 10000),
    message: decodeGitBytes(v.message),
    messageTruncated: flag(v.messageTruncated),
    author: { name: text(author.name), email: text(author.email) },
    time: integer(v.time),
    offsetMinutes: integer(v.offsetMinutes),
  };
}
export function decodeGitHistory(value: unknown) {
  const result = page(value, historyEntry, (value) => {
    const v = object(value);
    // The agent returns empty metadata for an unborn HEAD.
    if (v.resolvedRevision === undefined && v.truncated === undefined)
      return { resolvedRevision: null, truncated: false };
    return {
      resolvedRevision: decodeGitOid(v.resolvedRevision),
      truncated: flag(v.truncated),
    };
  });
  if (
    result.metadata.resolvedRevision === null &&
    (result.entries.length > 0 || result.nextCursor !== null)
  )
    throw new GitResponseError("unborn history");
  return result;
}
export type GitRepository = ReturnType<typeof decodeGitRepository>;
export type GitStatus = ReturnType<typeof decodeGitStatus>;
export type GitHistory = ReturnType<typeof decodeGitHistory>;

function delta(value: unknown) {
  const v = object(value);
  return {
    oldPath: nullable(v.oldPath, decodeGitBytes),
    newPath: nullable(v.newPath, decodeGitBytes),
    oldOid: nullable(v.oldOid, decodeGitOid),
    newOid: nullable(v.newOid, decodeGitOid),
    oldMode: count(v.oldMode),
    newMode: count(v.newMode),
    status: text(v.status),
  };
}
function comparison(value: unknown) {
  const v = object(value);
  const result = {
    commitOid: decodeGitOid(v.commitOid),
    parentOid: nullable(v.parentOid, decodeGitOid),
    parentIndex: nullable(v.parentIndex, count),
    parents: array(v.parents, decodeGitOid, 10000),
  };
  if (result.parents.length === 0) {
    if (result.parentIndex !== null || result.parentOid !== null)
      throw new GitResponseError("root commit parent");
  } else if (
    result.parentIndex === null ||
    result.parentOid === null ||
    result.parents[result.parentIndex]?.hex !== result.parentOid.hex
  ) {
    throw new GitResponseError("selected commit parent");
  }
  return result;
}
export function decodeGitCommitFiles(value: unknown) {
  return page(value, delta, (value) => {
    const v = object(value);
    return {
      ...comparison(v),
      totalFiles: count(v.totalFiles),
      truncated: flag(v.truncated),
    };
  });
}
function tracking(value: unknown) {
  return nullable(value, (value) => {
    const v = object(value);
    const configuration = object(v.configuration);
    const field = (value: unknown) =>
      array(
        value,
        (value) => {
          const entry = object(value);
          return {
            value: nullable(entry.value, decodeGitBytes),
            level: text(entry.level),
            includeDepth: count(entry.includeDepth),
          };
        },
        16,
      );
    return {
      token: text(v.token),
      editable: flag(v.editable),
      configuration: {
        remote: field(configuration.remote),
        merge: field(configuration.merge),
      },
    };
  });
}
export function decodeGitBranches(value: unknown) {
  return page(
    value,
    (value) => {
      const v = object(value);
      return {
        name: decodeGitBytes(v.name),
        reference: decodeGitBytes(v.reference),
        oid: nullable(v.oid, decodeGitOid),
        remote: flag(v.remote),
        current: flag(v.current),
        upstream: nullable(v.upstream, decodeGitBytes),
        tracking: tracking(v.tracking),
      };
    },
    (value) => {
      object(value);
      return {};
    },
  );
}

/** Never combine pages from different reads, including refreshes of the same query. */
export function appendGitPage<T, M>(
  previous: GitPage<T, M>,
  incoming: GitPage<T, M>,
  requestedCursor: string,
): GitPage<T, M> {
  if (
    previous.nextCursor === null ||
    previous.nextCursor !== requestedCursor ||
    previous.snapshot !== incoming.snapshot ||
    JSON.stringify(previous.metadata) !== JSON.stringify(incoming.metadata)
  ) {
    throw new GitResponseError("page snapshot");
  }
  if (
    incoming.nextCursor === requestedCursor ||
    previous.entries.length + incoming.entries.length > 50000
  )
    throw new GitResponseError("page limit");
  return { ...incoming, entries: [...previous.entries, ...incoming.entries] };
}
export function decodeGitDiff(value: unknown) {
  const envelope = object(value);
  const diff = object(envelope.diff);
  return {
    snapshot: text(envelope.snapshot),
    comparison: diff.commitOid === undefined ? null : comparison(diff),
    truncated: flag(diff.truncated),
    readOnly: flag(diff.readOnly),
    files: array(
      diff.files,
      (value) => {
        const v = object(value);
        return {
          ...delta(v),
          binary: flag(v.binary),
          additions: count(v.additions),
          deletions: count(v.deletions),
          hunks: array(
            v.hunks,
            (value) => {
              const h = object(value);
              return {
                id: h.id === undefined ? null : nullable(h.id, hunkId),
                oldStart: count(h.oldStart),
                oldLines: count(h.oldLines),
                newStart: count(h.newStart),
                newLines: count(h.newLines),
                lines: array(
                  h.lines,
                  (value) => {
                    const line = object(value);
                    return {
                      // Only changed lines are addressable; context and
                      // truncated hunks carry no identifier.
                      id:
                        line.id === undefined
                          ? null
                          : nullable(line.id, hunkId),
                      origin: text(line.origin),
                      oldLine: nullable(line.oldLine, count),
                      newLine: nullable(line.newLine, count),
                      content: decodeGitBytes(line.content),
                    };
                  },
                  100000,
                ),
              };
            },
            100001,
          ),
        };
      },
      10000,
    ),
  };
}
export type GitDiff = ReturnType<typeof decodeGitDiff>;
export type GitBranches = ReturnType<typeof decodeGitBranches>;
export type GitCommitFiles = ReturnType<typeof decodeGitCommitFiles>;

export function decodeGitOperation(value: unknown, expectedId: string) {
  const v = object(value);
  const operationId = text(v.operationId);
  if (operationId !== expectedId) throw new GitResponseError("operation ID");
  const states = [
    "running",
    "succeeded",
    "failed",
    "needs_resolution",
    "outcome_unknown",
  ] as const;
  const state = states.find((state) => state === v.state);
  if (!state) throw new GitResponseError("operation state");
  const result = nullable(v.result, object);
  const error = nullable(v.error, (value) => {
    const e = object(value);
    return {
      code: text(e.code),
      message: text(e.message),
      retry: text(e.retry),
    };
  });
  if (
    ((state === "succeeded" || state === "needs_resolution") &&
      result === null) ||
    (state === "failed" && error === null)
  )
    throw new GitResponseError("operation outcome");
  return {
    operationId,
    repository: text(v.repository),
    payloadHash: text(v.payloadHash),
    state,
    seq: count(v.seq),
    result,
    error,
  };
}
export type GitOperation = ReturnType<typeof decodeGitOperation>;

function optional<T>(value: unknown, decode: (value: unknown) => T): T | null {
  return value === undefined || value === null ? null : decode(value);
}
function oidText(value: unknown): string {
  const hex = text(value);
  if (!/^[0-9a-f]{40}$/.test(hex)) throw new GitResponseError("object ID");
  return hex;
}
function listToken(value: unknown) {
  return { listToken: text(object(value).listToken) };
}
export function decodeGitRemotes(value: unknown) {
  const v = object(value);
  const authentication = object(v.authentication);
  if (
    authentication.ssh !== "server_agent" ||
    authentication.https !== "anonymous"
  )
    throw new GitResponseError("remote authentication");
  return {
    entries: array(
      v.entries,
      (value) => {
        const remote = object(value);
        return {
          name: text(remote.name),
          url: nullable(remote.url, text),
          pushUrl: nullable(remote.pushUrl, text),
          token: text(remote.token),
        };
      },
      10000,
    ),
    authentication: {
      ssh: "server_agent" as const,
      https: "anonymous" as const,
    },
  };
}
export function decodeGitRemoteRefs(value: unknown) {
  return page(
    value,
    (value) => {
      const v = object(value);
      const kinds = ["head", "branch", "tag", "peeled_tag", "other"] as const;
      const kind = kinds.find((kind) => kind === v.kind);
      if (!kind) throw new GitResponseError("remote reference kind");
      return {
        reference: decodeGitBytes(v.reference),
        kind,
        oid: decodeGitOid(v.oid),
        symbolicTarget: nullable(v.symbolicTarget, decodeGitBytes),
      };
    },
    (value) => {
      const v = object(value);
      if (v.basis !== "remote_advertisement")
        throw new GitResponseError("remote reference basis");
      return {
        remote: text(v.remote),
        remoteToken: text(v.remoteToken),
        forPush: flag(v.forPush),
        basis: "remote_advertisement" as const,
        truncated: flag(v.truncated),
      };
    },
  );
}
export function decodeGitStashes(value: unknown) {
  return page(
    value,
    (value) => {
      const v = object(value);
      return {
        index: count(v.index),
        oid: oidText(v.oid),
        previousOid: oidText(v.previousOid),
        message: text(v.message),
        messageTruncated: flag(v.messageTruncated),
        time: integer(v.time),
      };
    },
    listToken,
  );
}
export function decodeGitWorktrees(value: unknown) {
  return page(
    value,
    (value) => {
      const v = object(value);
      const kinds = ["main", "bare", "linked"] as const;
      const states = [
        "available",
        "invalid",
        "missing",
        "unreadable",
        "unsupported",
      ] as const;
      const kind = kinds.find((kind) => kind === v.kind);
      const state = states.find((state) => state === v.state);
      if (!kind || !state) throw new GitResponseError("worktree state");
      return {
        name: nullable(v.name, decodeGitBytes),
        kind,
        state,
        path: nullable(v.path, decodeGitBytes),
        gitDir: decodeGitBytes(v.gitDir),
        current: flag(v.current),
        head: nullable(v.head, decodeGitHead),
        locked: nullable(v.locked, flag),
        lockReason: nullable(v.lockReason, decodeGitBytes),
        lockReasonUnavailable:
          v.lockReasonUnavailable === undefined
            ? false
            : flag(v.lockReasonUnavailable),
        prunable: nullable(v.prunable, flag),
        errorCode: optional(v.errorCode, text),
      };
    },
    listToken,
  );
}
export function decodeGitTags(value: unknown) {
  return page(
    value,
    (value) => {
      const v = object(value);
      return {
        name: decodeGitBytes(v.name),
        reference: decodeGitBytes(v.reference),
        oid: nullable(v.oid, decodeGitOid),
        symbolicTarget: nullable(v.symbolicTarget, decodeGitBytes),
        annotated: flag(v.annotated),
        detailsOmitted: flag(v.detailsOmitted),
        objectType: optional(v.objectType, text),
        targetOid: optional(v.targetOid, decodeGitOid),
        peeledOid: optional(v.peeledOid, decodeGitOid),
        peeledType: optional(v.peeledType, text),
        message: optional(v.message, decodeGitBytes),
        messageTruncated: optional(v.messageTruncated, flag),
        tagger: optional(v.tagger, (value) => {
          const tagger = object(value);
          return {
            name: text(tagger.name),
            email: text(tagger.email),
            time: integer(tagger.time),
            offsetMinutes: integer(tagger.offsetMinutes),
          };
        }),
      };
    },
    (value) => {
      object(value);
      return {};
    },
  );
}
export function decodeGitBlob(value: unknown) {
  const v = object(value);
  const oid = decodeGitOid(v.oid);
  const size = count(v.size);
  const truncated = flag(v.truncated);
  const bytesB64 = nullable(
    v.bytesB64,
    (value) => decodeGitBytes({ bytesB64: value, display: "" }).bytesB64,
  );
  if (
    (truncated && bytesB64 !== null) ||
    (!truncated && (bytesB64 === null || atob(bytesB64).length !== size)) ||
    (bytesB64 !== null && size > 512 * 1024)
  )
    throw new GitResponseError("blob content size");
  return { oid, size, truncated, bytesB64 };
}
export type GitRemotes = ReturnType<typeof decodeGitRemotes>;
export type GitRemoteRefs = ReturnType<typeof decodeGitRemoteRefs>;
export type GitStashes = ReturnType<typeof decodeGitStashes>;
export type GitTags = ReturnType<typeof decodeGitTags>;
export type GitWorktrees = ReturnType<typeof decodeGitWorktrees>;
export type GitBlob = ReturnType<typeof decodeGitBlob>;
