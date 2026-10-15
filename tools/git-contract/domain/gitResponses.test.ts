import { describe, expect, it } from "vitest";
import {
  decodeGitRemotes,
  decodeGitBlobPage,
  decodeGitBlob,
  decodeGitBytes,
  decodeGitDiff,
  decodeGitBranches,
  decodeGitCommitFiles,
  appendGitPage,
  decodeGitHistory,
  decodeGitOid,
  decodeGitRepository,
  decodeGitStatus,
  decodeGitStashes,
  decodeGitOperation,
  decodeGitWorktrees,
} from "./gitResponses";
import { gitPath } from "./git";

it("preserves reviewed interruption as an unknown outcome", () => {
  const outcome = decodeGitOperation(
    {
      operationId: "op",
      repository: "repo",
      payloadHash: "hash",
      state: "reviewed_unknown",
      seq: 3,
      result: null,
      error: {
        code: "OUTCOME_UNKNOWN",
        message: "Interrupted",
        retry: "never",
      },
    },
    "op",
  );
  expect(outcome.state).toBe("reviewed_unknown");
  expect(outcome.result).toBeNull();
  expect(outcome.error?.code).toBe("OUTCOME_UNKNOWN");
});

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
  it("continues past fifty thousand entries without an artificial end boundary", () => {
    const first = {
      snapshot: "large",
      entries: Array.from({ length: 50000 }, (_, i) => i),
      nextCursor: "tail",
      metadata: { total: 50005 },
    };
    const last = {
      ...first,
      entries: [50000, 50001, 50002, 50003, 50004],
      nextCursor: null,
    };
    const joined = appendGitPage(first, last, "tail");
    expect(joined.entries).toHaveLength(50005);
    expect(joined.entries.slice(-6)).toEqual([
      49999, 50000, 50001, 50002, 50003, 50004,
    ]);
    expect(joined.nextCursor).toBeNull();
    expect(first.entries).toHaveLength(50000);
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

describe("Git HTTPS authentication capability", () => {
  it.each(["anonymous", "server_helpers"])("accepts %s agents", (https) => {
    expect(
      decodeGitRemotes({
        entries: [],
        authentication: { ssh: "server_agent", https },
      }).authentication.https,
    ).toBe(https);
  });
  it("rejects unknown authentication modes", () => {
    expect(() =>
      decodeGitRemotes({
        entries: [],
        authentication: { ssh: "server_agent", https: "prompt" },
      }),
    ).toThrow();
  });
});

it("validates optional stash totals while accepting older agents", () => {
  const page = {
    snapshot: "s",
    nextCursor: null,
    entries: [],
    metadata: { listToken: "token" },
  };
  expect(decodeGitStashes(page).metadata).toEqual({ listToken: "token" });
  expect(
    decodeGitStashes({
      ...page,
      metadata: { ...page.metadata, totalEntries: 20000 },
    }).metadata.totalEntries,
  ).toBe(20000);
  for (const totalEntries of [-1, 1.5, "20000", null]) {
    expect(() =>
      decodeGitStashes({
        ...page,
        metadata: { ...page.metadata, totalEntries },
      }),
    ).toThrow();
  }
});

it("validates filtered worktree totals independently of the current page", () => {
  const page = {
    snapshot: "s",
    nextCursor: null,
    entries: [],
    metadata: { listToken: "t" },
  };
  for (const metadata of [
    { totalEntries: -1 },
    { matchingEntries: 0.5 },
    { totalEntries: 1, matchingEntries: 2 },
    { current: {} },
    { main: {} },
  ])
    expect(() =>
      decodeGitWorktrees({
        ...page,
        metadata: { ...page.metadata, ...metadata },
      }),
    ).toThrow();
  expect(
    decodeGitWorktrees({
      ...page,
      metadata: {
        ...page.metadata,
        totalEntries: 10000,
        matchingEntries: 0,
        current: null,
        main: null,
      },
    }).metadata,
  ).toMatchObject({ totalEntries: 10000, matchingEntries: 0 });
  expect(decodeGitWorktrees(page).metadata).toEqual({ listToken: "t" });
});

it("accepts every canonical one- and two-byte value and rejects nonzero padding bits", () => {
  for (let length = 1; length <= 2; length++) {
    const count = length === 1 ? 256 : 65536;
    for (let value = 0; value < count; value++) {
      const raw =
        length === 1
          ? String.fromCharCode(value)
          : String.fromCharCode(value >> 8, value & 255);
      const bytesB64 = btoa(raw);
      const result = decodeGitBytes({ bytesB64, display: "" });
      if (result.bytesB64 !== bytesB64)
        throw new Error("Canonical bytes changed");
    }
  }
  const alphabet =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  for (let index = 0; index < alphabet.length; index++) {
    for (const [value, valid] of [
      [`A${alphabet[index]}==`, index % 16 === 0],
      [`AA${alphabet[index]}=`, index % 4 === 0],
    ] as const) {
      if (valid)
        expect(decodeGitBytes({ bytesB64: value, display: "" }).bytesB64).toBe(
          value,
        );
      else
        expect(() =>
          decodeGitBytes({ bytesB64: value, display: "" }),
        ).toThrow();
    }
  }
});

it("rejects illegal alphabet, misplaced padding, and trailing whitespace", () => {
  for (const bytesB64 of [
    "A",
    "AAA",
    "=AAA",
    "A=AA",
    "AA=A",
    "====",
    "AA==AAAA",
    "AAAA=",
    "AAAA====",
    "AA\u0000A",
    "AAA-",
    "AAA_",
    "AAéA",
    "AA=\n",
    "AAA\n",
    "AAA\r",
    "AAA\u2028",
    "AAA\u2029",
    "AA==\n",
    "AA==    ",
    " AA=",
    "AA=\t",
  ]) {
    expect(() => decodeGitBytes({ bytesB64, display: "" })).toThrow();
  }
});

it("computes byte lengths for every padding case without decoding text", () => {
  for (const raw of [
    "",
    "a",
    "ab",
    "abc",
    "abcd",
    "\0\xff\x80",
    "x".repeat(393001),
  ]) {
    const bytesB64 = btoa(raw);
    const decoded = decodeGitBlob({
      oid,
      size: raw.length,
      truncated: false,
      bytesB64,
    });
    expect(decoded.bytesB64).toBe(bytesB64);
    const page = decodeGitBlobPage({
      snapshot: "blob",
      nextCursor: null,
      metadata: { oid, size: raw.length },
      entries: raw ? [{ offset: 0, bytesB64 }] : [],
    });
    expect(page.entries[0]?.byteLength ?? 0).toBe(raw.length);
    expect(() =>
      decodeGitBlob({ oid, size: raw.length + 1, truncated: false, bytesB64 }),
    ).toThrow();
    if (raw)
      expect(() =>
        decodeGitBlobPage({
          snapshot: "blob",
          nextCursor: null,
          metadata: { oid, size: raw.length + 1 },
          entries: [{ offset: 0, bytesB64 }],
        }),
      ).toThrow();
  }
});

it("validates matching status counts independently of repository totals", () => {
  expect(
    decodeGitStatus({
      ...status,
      metadata: { ...metadata, totalEntries: 20000, matchedEntries: 0 },
    }).metadata.matchedEntries,
  ).toBe(0);
  for (const matchedEntries of [-1, 2, 0.5]) {
    expect(() =>
      decodeGitStatus({
        ...status,
        metadata: { ...metadata, totalEntries: 1, matchedEntries },
      }),
    ).toThrow();
  }
});
