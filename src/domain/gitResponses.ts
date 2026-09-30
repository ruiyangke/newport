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
  // Canonical padded Base64, matching Rust's STANDARD encoder. Check the
  // unused trailing bits directly instead of decoding and re-encoding bytes.
  // Scan the alphabet without a repeated group or a permissive end anchor.
  const padding = bytesB64.endsWith("==") ? 2 : bytesB64.endsWith("=") ? 1 : 0;
  if (
    bytesB64.length % 4 !== 0 ||
    /[^A-Za-z0-9+/]/.test(bytesB64.slice(0, bytesB64.length - padding)) ||
    (padding === 2 && !/[AQgw]/.test(bytesB64.at(-3)!)) ||
    (padding === 1 && !/[AEIMQUYcgkosw048]/.test(bytesB64.at(-2)!))
  )
    throw new GitResponseError("bytesB64");
  return { bytesB64, display: text(v.display) };
}
/** Call only after canonical Base64 validation; lengths count bytes, not UTF-8 characters. */
function base64ByteLength(value: string): number {
  return (
    (value.length / 4) * 3 -
    (value.endsWith("==") ? 2 : value.endsWith("=") ? 1 : 0)
  );
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
  return page(value, statusEntry, decodeGitStatusMetadata);
}
export function decodeGitStatusMetadata(value: unknown) {
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
    ...(v.matchedEntries === undefined
      ? {}
      : {
          matchedEntries: (() => {
            const matched = count(v.matchedEntries);
            if (v.totalEntries !== undefined && matched > count(v.totalEntries))
              throw new GitResponseError("matched status count");
            return matched;
          })(),
        }),
    ...(v.groupCounts === undefined
      ? {}
      : {
          groupCounts: (() => {
            const groups = object(v.groupCounts);
            return {
              staged: count(groups.staged),
              unstaged: count(groups.unstaged),
              untracked: count(groups.untracked),
              conflicted: count(groups.conflicted),
            };
          })(),
        }),
    entryLimit: v.entryLimit === undefined ? null : count(v.entryLimit),
  };
}
export function decodeGitCommit(value: unknown) {
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
  const result = page(value, decodeGitCommit, (value) => {
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
export function validateGitPageContinuation<T, M>(
  previous: GitPage<T, M>,
  incoming: GitPage<T, M>,
  requestedCursor: string,
): void {
  if (
    previous.nextCursor === null ||
    previous.nextCursor !== requestedCursor ||
    previous.snapshot !== incoming.snapshot ||
    JSON.stringify(previous.metadata) !== JSON.stringify(incoming.metadata)
  ) {
    throw new GitResponseError("page snapshot");
  }
  if (incoming.nextCursor === requestedCursor)
    throw new GitResponseError("page cursor");
}
export function appendGitPage<T, M>(
  previous: GitPage<T, M>,
  incoming: GitPage<T, M>,
  requestedCursor: string,
): GitPage<T, M> {
  validateGitPageContinuation(previous, incoming, requestedCursor);
  return { ...incoming, entries: [...previous.entries, ...incoming.entries] };
}
/** Historical fragments contain no mutation identifiers. Text is decoded only
 * after contiguous byte pieces have been joined, including split UTF-8 bytes. */
function decodeDiffPage<
  M extends { totalUnits: number; hasOmissions: boolean },
>(value: unknown, working: boolean, decodeMetadata: (value: unknown) => M) {
  let units = 0;
  const result = page(
    value,
    (value) => {
      const file = object(value);
      const omissionReason = nullable(file.omissionReason, text);
      if (
        omissionReason !== null &&
        omissionReason !== "binary" &&
        omissionReason !== "file_size_limit"
      )
        throw new GitResponseError("diff omission reason");
      const hunks = array(
        file.hunks,
        (value) => {
          const hunk = object(value);
          const lines = array(
            hunk.lines,
            (value) => {
              // Accept the original rows as well as the opt-in tuple_v1 format.
              // Both pass exactly the same validation below.
              if (Array.isArray(value) && value.length !== (working ? 8 : 7))
                throw new GitResponseError("diff line tuple");
              const line = Array.isArray(value)
                ? {
                    lineIndex: value[0],
                    byteOffset: value[1],
                    lineComplete: value[2],
                    origin: value[3],
                    oldLine: value[4],
                    newLine: value[5],
                    contentBytesB64: value[6],
                    id: value[7],
                  }
                : object(value);
              const contentBytesB64 = decodeGitBytes({
                bytesB64: line.contentBytesB64,
                display: "",
              }).bytesB64;
              const byteLength = base64ByteLength(contentBytesB64);
              const origin = text(line.origin);
              if (
                ![" ", "+", "-", "=", ">", "<"].includes(origin) ||
                byteLength > 4096
              )
                throw new GitResponseError("diff line piece");
              const lineComplete = flag(line.lineComplete);
              if (!lineComplete && byteLength !== 4096)
                throw new GitResponseError("incomplete diff piece");
              const id = working ? nullable(line.id, hunkId) : null;
              if (
                working &&
                (lineComplete && (origin === "+" || origin === "-")) !==
                  (id !== null)
              )
                throw new GitResponseError("diff line identifier");
              units++;
              return {
                id,
                lineIndex: count(line.lineIndex),
                byteOffset: count(line.byteOffset),
                lineComplete,
                origin,
                oldLine: nullable(line.oldLine, count),
                newLine: nullable(line.newLine, count),
                contentBytesB64,
                byteLength,
              };
            },
            5000,
          );
          if (!lines.length) throw new GitResponseError("empty diff hunk");
          const totalLines = working ? count(hunk.totalLines) : null;
          if (
            totalLines !== null &&
            (!totalLines || lines.some((line) => line.lineIndex >= totalLines))
          )
            throw new GitResponseError("diff hunk length");
          return {
            id: working ? hunkId(hunk.id) : null,
            totalLines,
            index: count(hunk.index),
            oldStart: count(hunk.oldStart),
            oldLines: count(hunk.oldLines),
            newStart: count(hunk.newStart),
            newLines: count(hunk.newLines),
            lines,
          };
        },
        5000,
      );
      if (!hunks.length) units++;
      if (omissionReason && hunks.length)
        throw new GitResponseError("omitted diff content");
      return {
        ...delta(file),
        fileIndex: count(file.fileIndex),
        binary: flag(file.binary),
        omissionReason,
        additions: count(file.additions),
        deletions: count(file.deletions),
        hunks,
      };
    },
    decodeMetadata,
  );
  if (
    units > 5000 ||
    units > result.metadata.totalUnits ||
    (result.nextCursor !== null && !units)
  )
    throw new GitResponseError("diff page size");
  if (
    !result.metadata.hasOmissions &&
    result.entries.some((file) => file.omissionReason !== null)
  )
    throw new GitResponseError("diff omission metadata");
  validateDiffSequence(result.entries, false);
  return result;
}
export function decodeGitCommitDiffPage(value: unknown) {
  return decodeDiffPage(value, false, (value) => {
    const v = object(value);
    if (v.readOnly !== true)
      throw new GitResponseError("historical diff must be read-only");
    const contextLines = count(v.contextLines);
    if (contextLines > 100) throw new GitResponseError("diff context");
    return {
      ...comparison(v),
      contextLines,
      selectedPath: decodeGitBytes(v.selectedPath),
      readOnly: true as const,
      hasOmissions: flag(v.hasOmissions),
      totalUnits: count(v.totalUnits),
    };
  });
}
export function decodeGitWorkingDiffPage(value: unknown) {
  return decodeDiffPage(value, true, (value) => {
    const v = object(value);
    if (v.readOnly !== false) throw new GitResponseError("working diff mode");
    const side = text(v.side);
    if (
      side !== "head_to_index" &&
      side !== "index_to_worktree" &&
      side !== "head_to_worktree"
    )
      throw new GitResponseError("working diff side");
    const contextLines = count(v.contextLines);
    if (contextLines > 100) throw new GitResponseError("diff context");
    return {
      sourceSnapshot: text(v.sourceSnapshot),
      entryId: text(v.entryId),
      side,
      contextLines,
      readOnly: false as const,
      hasOmissions: flag(v.hasOmissions),
      totalFiles: count(v.totalFiles),
      totalUnits: count(v.totalUnits),
    };
  });
}
export type GitWorkingDiffPage = ReturnType<typeof decodeGitWorkingDiffPage>;
export type GitCommitDiffPage = ReturnType<typeof decodeGitCommitDiffPage>;
type DiffPage = {
  snapshot: string;
  nextCursor: string | null;
  entries: GitCommitDiffPage["entries"];
  metadata: { totalUnits: number; hasOmissions: boolean };
};
type DiffFragment = GitCommitDiffPage["entries"][number];
type DiffPiece = DiffFragment["hunks"][number]["lines"][number];

function followsPiece(previous: DiffPiece, incoming: DiffPiece) {
  const continues = !previous.lineComplete;
  if (
    incoming.lineIndex !== previous.lineIndex + (continues ? 0 : 1) ||
    incoming.byteOffset !==
      (continues ? previous.byteOffset + previous.byteLength : 0) ||
    (continues &&
      (incoming.origin !== previous.origin ||
        incoming.oldLine !== previous.oldLine ||
        incoming.newLine !== previous.newLine))
  )
    throw new GitResponseError("non-contiguous diff pieces");
}
function validateDiffSequence(files: DiffFragment[], fromStart: boolean) {
  let lastFile = -1;
  for (const [fi, file] of files.entries()) {
    if (file.fileIndex <= lastFile)
      throw new GitResponseError("diff file order");
    lastFile = file.fileIndex;
    let lastHunk = -1;
    for (const [hi, hunk] of file.hunks.entries()) {
      const startsHunk = fromStart || fi > 0 || hi > 0;
      if (
        (hi > 0 && hunk.index !== lastHunk + 1) ||
        ((fromStart || fi > 0) && hi === 0 && hunk.index !== 0)
      )
        throw new GitResponseError("diff hunk order");
      lastHunk = hunk.index;
      if (
        startsHunk &&
        (hunk.lines[0].lineIndex !== 0 || hunk.lines[0].byteOffset !== 0)
      )
        throw new GitResponseError("diff hunk start");
      for (let i = 1; i < hunk.lines.length; i++)
        followsPiece(hunk.lines[i - 1], hunk.lines[i]);
      if (
        (hi + 1 < file.hunks.length || fi + 1 < files.length) &&
        (!hunk.lines.at(-1)!.lineComplete ||
          (hunk.totalLines !== null &&
            hunk.lines.at(-1)!.lineIndex + 1 !== hunk.totalLines))
      )
        throw new GitResponseError("unfinished diff line");
    }
  }
}
export function validateInitialGitDiffPage<T extends DiffPage>(page: T) {
  validateDiffSequence(page.entries, true);
  validateDiffEnd(page);
  return page;
}
function diffUnits(page: DiffPage) {
  return page.entries.reduce(
    (sum, file) =>
      sum +
      (file.hunks.length
        ? file.hunks.reduce((n, hunk) => n + hunk.lines.length, 0)
        : 1),
    0,
  );
}
function validateDiffEnd(page: DiffPage) {
  if ("totalFiles" in page.metadata) {
    const last = page.entries.at(-1)?.hunks.at(-1);
    if (
      page.entries.some((file, index) => file.fileIndex !== index) ||
      page.entries.length > Number(page.metadata.totalFiles) ||
      (page.nextCursor === null &&
        (page.entries.length !== page.metadata.totalFiles ||
          (last && last.lines.at(-1)!.lineIndex + 1 !== last.totalLines)))
    )
      throw new GitResponseError("working diff completeness");
  }
  const loadedUnits = diffUnits(page);
  if (
    loadedUnits > page.metadata.totalUnits ||
    (page.nextCursor === null &&
      (loadedUnits !== page.metadata.totalUnits ||
        page.entries.at(-1)?.hunks.at(-1)?.lines.at(-1)?.lineComplete ===
          false)) ||
    (page.nextCursor !== null && loadedUnits >= page.metadata.totalUnits)
  )
    throw new GitResponseError("diff continuation boundary");
}
/** Merge at the fragment boundary; never silently deduplicate raw text. A
 * repeated or missing piece is a protocol error rather than a changed diff. */
export function appendGitDiffPage<T extends DiffPage>(
  previous: T,
  incoming: T,
  cursor: string,
): T {
  if (
    previous.nextCursor !== cursor ||
    incoming.nextCursor === cursor ||
    previous.snapshot !== incoming.snapshot ||
    JSON.stringify(previous.metadata) !== JSON.stringify(incoming.metadata) ||
    !diffUnits(incoming)
  )
    throw new GitResponseError("diff page snapshot");
  const entries = [...previous.entries];
  for (const fragment of incoming.entries) {
    const tail = entries.at(-1);
    if (!tail || tail.fileIndex !== fragment.fileIndex) {
      if (tail && tail.fileIndex >= fragment.fileIndex)
        throw new GitResponseError("diff file order");
      entries.push(fragment);
      continue;
    }
    const { hunks: previousHunks, ...previousFile } = tail;
    const { hunks: incomingHunks, ...incomingFile } = fragment;
    if (
      JSON.stringify(previousFile) !== JSON.stringify(incomingFile) ||
      !previousHunks.length ||
      !incomingHunks.length
    )
      throw new GitResponseError("diff file continuation");
    const hunks = [...previousHunks];
    for (const hunk of incomingHunks) {
      const last = hunks.at(-1)!;
      if (hunk.index !== last.index) {
        if (
          hunk.index !== last.index + 1 ||
          !last.lines.at(-1)!.lineComplete ||
          hunk.lines[0].lineIndex !== 0 ||
          hunk.lines[0].byteOffset !== 0
        )
          throw new GitResponseError("diff hunk continuation");
        hunks.push(hunk);
        continue;
      }
      const { lines: oldLines, ...oldHeader } = last;
      const { lines: newLines, ...newHeader } = hunk;
      if (JSON.stringify(oldHeader) !== JSON.stringify(newHeader))
        throw new GitResponseError("diff hunk header");
      followsPiece(oldLines.at(-1)!, newLines[0]);
      hunks[hunks.length - 1] = { ...hunk, lines: [...oldLines, ...newLines] };
    }
    entries[entries.length - 1] = { ...tail, hunks };
  }
  const result = {
    ...incoming,
    entries,
  };
  validateDiffSequence(entries, true);
  validateDiffEnd(result);
  return result;
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
    "reviewed_unknown",
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
export function decodeGitRemoteNames(value: unknown) {
  const result = page(
    value,
    (value) => ({ name: text(object(value).name) }),
    (value) => ({ totalEntries: count(object(value).totalEntries) }),
  );
  if (
    result.entries.length > result.metadata.totalEntries ||
    (result.nextCursor !== null &&
      (result.entries.length === 0 ||
        result.entries.length >= result.metadata.totalEntries)) ||
    new Set(result.entries.map((entry) => entry.name)).size !==
      result.entries.length
  )
    throw new GitResponseError("remote names page");
  return result;
}
export function decodeGitRemote(value: unknown) {
  const remote = object(value);
  return {
    name: text(remote.name),
    url: nullable(remote.url, text),
    pushUrl: nullable(remote.pushUrl, text),
    token: text(remote.token),
  };
}
export function decodeGitRemotes(value: unknown) {
  const v = object(value);
  const authentication = object(v.authentication);
  if (
    authentication.ssh !== "server_agent" ||
    (authentication.https !== "anonymous" &&
      authentication.https !== "server_helpers")
  )
    throw new GitResponseError("remote authentication");
  return {
    entries: array(v.entries, decodeGitRemote, 10000),
    authentication: {
      ssh: "server_agent" as const,
      https: authentication.https as "anonymous" | "server_helpers",
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
    (value) => {
      const v = object(value);
      return {
        ...listToken(value),
        ...(v.totalEntries === undefined
          ? {}
          : { totalEntries: count(v.totalEntries) }),
      };
    },
  );
}
function decodeGitWorktree(value: unknown) {
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
}
export function decodeGitWorktrees(value: unknown) {
  return page(value, decodeGitWorktree, (value) => {
    const v = object(value);
    const total =
      v.totalEntries === undefined ? undefined : count(v.totalEntries);
    const matching =
      v.matchingEntries === undefined ? undefined : count(v.matchingEntries);
    if (total !== undefined && matching !== undefined && matching > total)
      throw new GitResponseError("worktree counts");
    return {
      ...listToken(value),
      ...(total === undefined ? {} : { totalEntries: total }),
      ...(matching === undefined ? {} : { matchingEntries: matching }),
      ...(v.current === undefined
        ? {}
        : { current: nullable(v.current, decodeGitWorktree) }),
      ...(v.main === undefined
        ? {}
        : { main: nullable(v.main, decodeGitWorktree) }),
    };
  });
}

function tagDetails(v: Record<string, unknown>) {
  return {
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
}
export function decodeGitTag(value: unknown) {
  const v = object(value);
  return { oid: decodeGitOid(v.oid), ...tagDetails(v) };
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
        ...tagDetails(v),
      };
    },
    (value) => {
      object(value);
      return {};
    },
  );
}
/** Blob pages carry raw byte ranges; offsets never count decoded characters. */
export function decodeGitBlobPage(value: unknown) {
  const result = page(
    value,
    (value) => {
      const v = object(value);
      const bytesB64 = text(v.bytesB64);
      if (bytesB64.length > 524288)
        throw new GitResponseError("blob page size");
      decodeGitBytes({ bytesB64, display: "" });
      const byteLength = base64ByteLength(bytesB64);
      if (!byteLength) throw new GitResponseError("empty blob chunk");
      return { offset: count(v.offset), bytesB64, byteLength };
    },
    (value) => {
      const v = object(value);
      return { oid: decodeGitOid(v.oid), size: count(v.size) };
    },
  );
  if (result.entries.length > 1) throw new GitResponseError("blob page chunks");
  const last = result.entries.at(-1);
  const end = last ? last.offset + last.byteLength : 0;
  if (
    !Number.isSafeInteger(end) ||
    end > result.metadata.size ||
    (result.nextCursor === null
      ? end !== result.metadata.size
      : !last || end >= result.metadata.size)
  )
    throw new GitResponseError("blob page boundary");
  return result;
}
export type GitBlobPage = ReturnType<typeof decodeGitBlobPage>;
export function validateInitialGitBlobPage(page: GitBlobPage) {
  if (page.entries.length && page.entries[0].offset !== 0)
    throw new GitResponseError("blob initial offset");
  return page;
}
export function appendGitBlobPage(
  previous: GitBlobPage,
  incoming: GitBlobPage,
  cursor: string,
): GitBlobPage {
  const last = previous.entries.at(-1);
  const expectedOffset = last ? last.offset + last.byteLength : 0;
  if (!incoming.entries.length || incoming.entries[0].offset !== expectedOffset)
    throw new GitResponseError("non-contiguous blob pages");
  return appendGitPage(previous, incoming, cursor);
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
    (!truncated &&
      (bytesB64 === null || base64ByteLength(bytesB64) !== size)) ||
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
