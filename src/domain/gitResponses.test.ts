import { describe, expect, it } from "vitest";
import {
  decodeGitBytes,
  decodeGitDiff,
  decodeGitBranches,
  decodeGitCommitFiles,
  appendGitPage,
  decodeGitHistory,
  decodeGitOid,
  decodeGitRepository,
  decodeGitStatus,
} from "./gitResponses";
import { gitPath } from "./git";

const oid = { algorithm: "sha1", hex: "a".repeat(40) };
const sha256Oid = { algorithm: "sha256" as const, hex: "b".repeat(64) };
const head = {
  oid: null,
  name: gitPath("refs/heads/main"),
  unborn: true,
  detached: false,
};
const metadata = {
  head,
  operationState: "Clean",
  integration: null,
  ahead: null,
  behind: null,
  basis: "stored_refs",
  upstreamRef: null,
};
const status = {
  snapshot: "status-1",
  nextCursor: null,
  metadata,
  entries: [
    {
      entryId: "file-1",
      path: { bytesB64: "L/8=", display: "/�" },
      oldPath: null,
      flags: 1,
      staged: true,
      unstaged: false,
      untracked: false,
      conflicted: true,
      conflict: {
        base: null,
        ours: { oid, path: gitPath("file"), mode: 33188 },
        theirs: null,
      },
    },
  ],
};

describe("Git response validation", () => {
  it("keeps merge-parent selection and historical rename paths in commit file pages", () => {
    const parent = { ...oid, hex: "b".repeat(40) };
    const value = {
      snapshot: "commit-files",
      nextCursor: "page-2",
      metadata: {
        commitOid: oid,
        parentOid: parent,
        parentIndex: 1,
        parents: [oid, parent],
        totalFiles: 205,
        truncated: false,
      },
      entries: [
        {
          oldPath: gitPath("before"),
          newPath: gitPath("after"),
          oldOid: parent,
          newOid: oid,
          oldMode: 33188,
          newMode: 33261,
          status: "Renamed",
        },
      ],
    };
    const result = decodeGitCommitFiles(value);
    expect(result.metadata.parentIndex).toBe(1);
    expect(result.metadata.totalFiles).toBe(205);
    expect(result.entries[0].oldPath?.display).toBe("before");
    expect(result.entries[0].newMode).toBe(33261);
    expect(() =>
      decodeGitCommitFiles({
        ...value,
        metadata: { ...value.metadata, parentIndex: -1 },
      }),
    ).toThrow();
  });
  it("retains upstream configuration guards without assuming every branch can be edited", () => {
    const local = {
      name: gitPath("main"),
      reference: gitPath("refs/heads/main"),
      oid,
      remote: false,
      current: true,
      upstream: null,
      tracking: {
        token: "guard",
        editable: false,
        configuration: {
          remote: [
            { value: gitPath("origin"), level: "Global", includeDepth: 1 },
          ],
          merge: [],
        },
      },
    };
    const remote = { ...local, remote: true, current: false, tracking: null };
    const result = decodeGitBranches({
      snapshot: "branches",
      nextCursor: null,
      metadata: {},
      entries: [local, remote],
    });
    expect(result.entries[0].tracking?.editable).toBe(false);
    expect(
      result.entries[0].tracking?.configuration.remote[0].includeDepth,
    ).toBe(1);
    expect(result.entries[1].tracking).toBeNull();
  });
  it("appends only the requested next page of the same snapshot", () => {
    const first = {
      snapshot: "one",
      entries: [1],
      nextCursor: "next",
      metadata: { truncated: false },
    };
    const next = { ...first, entries: [2], nextCursor: null };
    expect(appendGitPage(first, next, "next").entries).toEqual([1, 2]);
    expect(first.entries).toEqual([1]);
    expect(() =>
      appendGitPage(first, { ...next, snapshot: "refresh" }, "next"),
    ).toThrow();
    expect(() => appendGitPage(first, next, "wrong cursor")).toThrow();
    expect(() =>
      appendGitPage(first, { ...next, metadata: { truncated: true } }, "next"),
    ).toThrow();
    expect(() =>
      appendGitPage(first, { ...next, nextCursor: "next" }, "next"),
    ).toThrow();
    expect(() => appendGitPage(next, next, "next")).toThrow();
  });
  it("preserves diff truncation, binary files and missing line numbers", () => {
    const file = {
      oldPath: null,
      newPath: gitPath("image.png"),
      oldOid: null,
      newOid: oid,
      oldMode: 0,
      newMode: 33188,
      status: "Added",
      binary: true,
      additions: 0,
      deletions: 0,
      hunks: [],
    };
    const hunk = {
      id: "a".repeat(64),
      oldStart: 0,
      oldLines: 0,
      newStart: 1,
      newLines: 1,
      lines: [
        { origin: "+", oldLine: null, newLine: 1, content: gitPath("hello\n") },
      ],
    };
    const value = {
      snapshot: "status",
      diff: {
        truncated: true,
        readOnly: true,
        files: [file, { ...file, binary: false, additions: 1, hunks: [hunk] }],
      },
    };
    const result = decodeGitDiff(value);
    expect(result.truncated).toBe(true);
    expect(result.files[1].hunks[0].id).toBe("a".repeat(64));
    const withoutId = { ...hunk, id: undefined };
    expect(
      decodeGitDiff({
        ...value,
        diff: { ...value.diff, files: [{ ...file, hunks: [withoutId] }] },
      }).files[0].hunks[0].id,
    ).toBeNull();
    expect(() =>
      decodeGitDiff({
        ...value,
        diff: {
          ...value.diff,
          files: [{ ...file, hunks: [{ ...hunk, id: "invalid" }] }],
        },
      }),
    ).toThrow();

    expect(result.files[0].binary).toBe(true);
    expect(result.files[1].hunks[0].lines[0].oldLine).toBeNull();
    expect(result.files[1].hunks[0].lines[0].content.display).toBe("hello\n");
    expect(() =>
      decodeGitDiff({
        ...value,
        diff: { ...value.diff, files: [{ ...file, binary: undefined }] },
      }),
    ).toThrow();
  });
  it("preserves missing tracking information and conflict sides without inventing zeros", () => {
    const decoded = decodeGitStatus(status);
    expect(decoded.metadata.ahead).toBeNull();
    expect(decoded.metadata.behind).toBeNull();
    expect(decoded.entries[0].path?.bytesB64).toBe("L/8=");
    expect(decoded.entries[0].conflict?.base).toBeNull();
    expect(decoded.entries[0].conflict?.ours?.oid.hex).toBe(oid.hex);
    expect(decoded).not.toBe(status);
  });
  it("rejects incomplete and malformed status pages instead of showing an empty clean repository", () => {
    for (const value of [
      null,
      {},
      { ...status, entries: null },
      { ...status, nextCursor: 2 },
      { ...status, metadata: { ...metadata, ahead: -1 } },
      { ...status, entries: [{ ...status.entries[0], staged: "false" }] },
      { ...status, entries: Array(201).fill(status.entries[0]) },
    ]) {
      expect(() => decodeGitStatus(value)).toThrow("Invalid Git response");
    }
  });
  it("accepts unborn history and preserves an empty commit message", () => {
    expect(
      decodeGitHistory({
        snapshot: "empty",
        entries: [],
        metadata: {},
        nextCursor: null,
      }).entries,
    ).toEqual([]);
    const commit = {
      oid,
      parents: [],
      message: { bytesB64: "", display: "" },
      messageTruncated: false,
      author: { name: "A", email: "a@example.com" },
      time: -100,
      offsetMinutes: -480,
    };
    const page = {
      snapshot: "history",
      entries: [commit],
      metadata: { resolvedRevision: oid, truncated: true },
      nextCursor: "next",
    };
    expect(decodeGitHistory(page).metadata.truncated).toBe(true);
    expect(decodeGitHistory(page).entries[0].time).toBe(-100);
    expect(() => decodeGitHistory({ ...page, metadata: {} })).toThrow(
      "unborn history",
    );
    expect(() =>
      decodeGitHistory({
        ...page,
        entries: [{ ...commit, parents: ["invalid"] }],
      }),
    ).toThrow();
  });
  it("handles detached and bare repository capabilities explicitly", () => {
    const repository = {
      repoId: "repo",
      commonRepoId: "common",
      root: gitPath("/repo.git"),
      bare: true,
      objectFormat: "sha1",
      head: { ...head, oid, unborn: false, detached: true },
      operationState: "Clean",
      integration: null,
      capabilities: { readOnly: true, workingTree: false },
    };
    expect(decodeGitRepository(repository).capabilities.workingTree).toBe(
      false,
    );
    expect(decodeGitRepository(repository).head.detached).toBe(true);
    expect(() =>
      decodeGitRepository({ ...repository, capabilities: {} }),
    ).toThrow();
    // Both hash algorithms are supported; an unknown one is still refused.
    expect(
      decodeGitRepository({
        ...repository,
        objectFormat: "sha256",
        head: { ...head, oid: sha256Oid, unborn: false, detached: true },
      }).objectFormat,
    ).toBe("sha256");
    expect(decodeGitRepository(repository).objectFormat).toBe("sha1");
    expect(() =>
      decodeGitRepository({ ...repository, objectFormat: "sha512" }),
    ).toThrow();
  });
  it("rejects corrupt opaque bytes and normalizes the agent's two OID envelopes", () => {
    expect(decodeGitOid({ format: "sha1", hex: oid.hex })).toEqual(oid);
    expect(() => decodeGitOid({ ...oid, hex: "abc" })).toThrow();
    // Length follows the declared algorithm, in both directions.
    expect(decodeGitOid(sha256Oid)).toEqual(sha256Oid);
    expect(() => decodeGitOid({ algorithm: "sha256", hex: oid.hex })).toThrow();
    expect(() =>
      decodeGitOid({ algorithm: "sha1", hex: sha256Oid.hex }),
    ).toThrow();
    for (const bytesB64 of ["?", "Zg", "Zh==", "Zg==\n"]) {
      expect(() => decodeGitBytes({ bytesB64, display: "f" })).toThrow();
    }
  });
});
