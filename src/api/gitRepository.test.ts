import { expect, it, vi } from "vitest";
import { GitRepositoryClient } from "./gitRepository";
import { gitPath } from "../domain/git";

it("passes cancellation through a scoped client without changing the shared client", async () => {
  const session = {
    request: vi.fn().mockRejectedValue(new Error("offline")),
    forget: vi.fn(),
  };
  const client = new GitRepositoryClient(session);
  const signal = new AbortController().signal;
  await expect(client.withSignal(signal).status("repo")).rejects.toThrow(
    "offline",
  );
  expect(session.request).toHaveBeenLastCalledWith(
    { method: "repo.status", params: { repoId: "repo", cursor: undefined } },
    signal,
  );
  await expect(client.status("repo")).rejects.toThrow("offline");
  expect(session.request).toHaveBeenLastCalledWith({
    method: "repo.status",
    params: { repoId: "repo", cursor: undefined },
  });
});

it("closes malformed sessions and does not retry the read", async () => {
  const session = {
    request: vi.fn().mockResolvedValue({}),
    forget: vi.fn().mockResolvedValue(undefined),
  };
  const client = new GitRepositoryClient(session);
  await expect(client.status("repo")).rejects.toMatchObject({
    code: "PROTOCOL_ERROR",
  });
  expect(session.forget).toHaveBeenCalledOnce();
  expect(session.request).toHaveBeenCalledOnce();
});

const commit = { algorithm: "sha1", hex: "a".repeat(40) };
const parent = { algorithm: "sha1", hex: "b".repeat(40) };
const comparison = {
  commitOid: commit,
  parents: [parent, commit],
  parentOid: commit,
  parentIndex: 1,
};
const file = {
  oldPath: gitPath("before"),
  newPath: gitPath("after"),
  oldOid: parent,
  newOid: commit,
  oldMode: 33188,
  newMode: 33188,
  status: "Renamed",
  binary: false,
  additions: 0,
  deletions: 0,
  hunks: [],
};
function diff(metadata: unknown = comparison) {
  return {
    snapshot: commit.hex,
    diff: {
      ...(metadata as object),
      files: [file],
      readOnly: true,
      truncated: false,
    },
  };
}
function clientWith(value: unknown) {
  const session = {
    request: vi.fn().mockResolvedValue(value),
    forget: vi.fn().mockResolvedValue(undefined),
  };
  return { client: new GitRepositoryClient(session), session };
}

it("accepts the selected merge parent and either side of a renamed file", async () => {
  const { client, session } = clientWith(diff());
  for (const path of [file.oldPath, file.newPath]) {
    await expect(
      client.commitDiff({
        repoId: "repo",
        commitOid: commit.hex,
        parentIndex: 1,
        path,
      }),
    ).resolves.toMatchObject({ comparison: { parentIndex: 1 } });
  }
  expect(session.forget).not.toHaveBeenCalled();
});

it("rejects a valid but unrelated commit, parent, file or snapshot", async () => {
  const cases = [
    { value: diff(), params: { commitOid: parent.hex, parentIndex: 1 } },
    { value: diff(), params: { commitOid: commit.hex, parentIndex: 0 } },
    {
      value: diff(),
      params: {
        commitOid: commit.hex,
        parentIndex: 1,
        path: gitPath("unrelated"),
      },
    },
    {
      value: { ...diff(), snapshot: parent.hex },
      params: { commitOid: commit.hex, parentIndex: 1 },
    },
    { value: diff({}), params: { commitOid: commit.hex, parentIndex: 1 } },
  ];
  for (const { value, params } of cases) {
    const { client, session } = clientWith(value);
    await expect(
      client.commitDiff({ repoId: "repo", ...params }),
    ).rejects.toMatchObject({ code: "PROTOCOL_ERROR" });
    expect(session.forget).toHaveBeenCalledOnce();
    expect(session.request).toHaveBeenCalledOnce();
  }
});

it("requires parent metadata to match the commit's actual parent list", async () => {
  for (const metadata of [
    { ...comparison, parentOid: parent },
    { ...comparison, parentIndex: 2 },
    { ...comparison, parents: [], parentIndex: 0 },
  ]) {
    const { client } = clientWith(diff(metadata));
    await expect(
      client.commitDiff({
        repoId: "repo",
        commitOid: commit.hex,
        parentIndex: 1,
      }),
    ).rejects.toMatchObject({ code: "PROTOCOL_ERROR" });
  }
  const { client } = clientWith(
    diff({
      commitOid: commit,
      parents: [],
      parentIndex: null,
      parentOid: null,
    }),
  );
  await expect(
    client.commitDiff({ repoId: "repo", commitOid: commit.hex }),
  ).resolves.toMatchObject({ comparison: { parentIndex: null } });
});

it("binds historical file listings and working diffs to their requested comparison", async () => {
  const { client, session } = clientWith({
    snapshot: "files",
    entries: [],
    nextCursor: null,
    metadata: { ...comparison, totalFiles: 0, truncated: false },
  });
  await expect(
    client.commitFiles({
      repoId: "repo",
      commitOid: commit.hex,
      parentIndex: 0,
    }),
  ).rejects.toMatchObject({ code: "PROTOCOL_ERROR" });
  expect(session.forget).toHaveBeenCalledOnce();
  const working = clientWith({
    snapshot: "old",
    diff: { files: [], truncated: false, readOnly: true },
  });
  await expect(
    working.client.diff({
      repoId: "repo",
      snapshot: "new",
      entryId: "file",
      side: "head_to_index",
    }),
  ).rejects.toMatchObject({ code: "PROTOCOL_ERROR" });
});

it("captures request parameters before an asynchronous response arrives", async () => {
  let finish!: (value: unknown) => void;
  const session = {
    request: vi.fn(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    ),
    forget: vi.fn().mockResolvedValue(undefined),
  };
  const client = new GitRepositoryClient(session);
  const params = {
    repoId: "repo",
    commitOid: commit.hex,
    parentIndex: 1,
    path: { ...file.newPath },
  };
  const response = client.commitDiff(params);
  params.parentIndex = 0;
  params.path.bytesB64 = "changed";
  finish(diff());
  await expect(response).resolves.toMatchObject({
    comparison: { parentIndex: 1 },
  });
  expect(session.request.mock.calls[0]).toEqual([
    {
      method: "repo.commit_diff",
      params: {
        repoId: "repo",
        commitOid: commit.hex,
        parentIndex: 1,
        path: file.newPath,
      },
    },
  ]);
});

it("preserves query and cursor when requesting another history page", async () => {
  const session = {
    request: vi.fn().mockResolvedValue({
      snapshot: "empty",
      entries: [],
      nextCursor: null,
      metadata: {},
    }),
    forget: vi.fn().mockResolvedValue(undefined),
  };
  await new GitRepositoryClient(session).history(
    "repo",
    "refs/heads/main",
    "opaque-cursor",
  );
  expect(session.request).toHaveBeenCalledWith({
    method: "repo.history",
    params: {
      repoId: "repo",
      revision: "refs/heads/main",
      cursor: "opaque-cursor",
      messageBytes: 512,
    },
  });
  expect(session.forget).not.toHaveBeenCalled();
});

it("sends branch filters and page size to the server with the continuation cursor", async () => {
  const session = {
    request: vi.fn().mockResolvedValue({
      snapshot: "s",
      entries: [],
      nextCursor: null,
      metadata: {},
    }),
    forget: vi.fn(),
  };
  await new GitRepositoryClient(session).branches("repo", "next", {
    filter: "feature/",
    branchKind: "remote",
    pageSize: 75,
  });
  expect(session.request).toHaveBeenCalledWith({
    method: "repo.branches",
    params: {
      repoId: "repo",
      cursor: "next",
      filter: "feature/",
      branchKind: "remote",
      pageSize: 75,
    },
  });
});

it("retains the operation ID when a write returns malformed data", async () => {
  const { client, session } = clientWith({
    operationId: "different",
    state: "succeeded",
  });
  await expect(
    client.start({
      operationId: "original",
      repoId: "repo",
      expectedSnapshot: "snapshot",
      action: { kind: "commit", message: "Message" },
    }),
  ).rejects.toMatchObject({ operationId: "original", code: "OUTCOME_UNKNOWN" });
  expect(session.forget).toHaveBeenCalledOnce();
  expect(session.request).toHaveBeenCalledOnce();
});

it("returns a confirmed failed operation for review rather than mistaking RPC success for write success", async () => {
  const record = {
    operationId: "original",
    repository: "repository",
    payloadHash: "hash",
    state: "failed",
    seq: 1,
    result: null,
    error: {
      code: "EMPTY_COMMIT",
      message: "No staged changes",
      retry: "never",
    },
  };
  const { client, session } = clientWith(record);
  await expect(client.operation("original")).resolves.toMatchObject({
    state: "failed",
    error: { code: "EMPTY_COMMIT" },
  });
  expect(session.forget).not.toHaveBeenCalled();
  session.request.mockResolvedValue({
    ...record,
    state: "succeeded",
    error: null,
  });
  await expect(client.operation("original")).rejects.toMatchObject({
    code: "PROTOCOL_ERROR",
  });
});

it("rejects advertisements from a different remote or push direction", async () => {
  for (const metadata of [
    { remote: "other", remoteToken: "config", forPush: false },
    { remote: "origin", remoteToken: "stale", forPush: false },
    { remote: "origin", remoteToken: "config", forPush: true },
  ]) {
    const session = {
      request: vi.fn().mockResolvedValue({
        snapshot: "s",
        entries: [],
        nextCursor: null,
        metadata: {
          ...metadata,
          basis: "remote_advertisement",
          truncated: false,
        },
      }),
      forget: vi.fn().mockResolvedValue(undefined),
    };
    await expect(
      new GitRepositoryClient(session).remoteRefs({
        repoId: "repo",
        remote: "origin",
        expectedToken: "config",
      }),
    ).rejects.toMatchObject({ code: "PROTOCOL_ERROR" });
    expect(session.forget).toHaveBeenCalledOnce();
    expect(session.request).toHaveBeenCalledOnce();
  }
});
it("rejects valid blob bytes belonging to a different object", async () => {
  const session = {
    request: vi.fn().mockResolvedValue({
      oid: commit,
      size: 0,
      bytesB64: "",
      truncated: false,
    }),
    forget: vi.fn().mockResolvedValue(undefined),
  };
  await expect(
    new GitRepositoryClient(session).blob("repo", parent.hex),
  ).rejects.toMatchObject({ code: "PROTOCOL_ERROR" });
  expect(session.forget).toHaveBeenCalledOnce();
});

it.each(["repo.init", "repo.clone"] as const)(
  "preserves uncertain %s identity when decoding fails",
  async (method) => {
    const session = {
      request: vi.fn().mockResolvedValue({}),
      forget: vi.fn().mockResolvedValue(undefined),
    };
    const params = { operationId: "bootstrap-id", path: gitPath("/srv/new") };
    const request =
      method === "repo.init"
        ? { method, params: { ...params, initialBranch: "main" } }
        : { method, params: { ...params, url: "ssh://host/repo" } };
    await expect(
      new GitRepositoryClient(session).bootstrap(request),
    ).rejects.toMatchObject({
      operationId: "bootstrap-id",
      code: "OUTCOME_UNKNOWN",
    });
    expect(session.request).toHaveBeenCalledOnce();
    expect(session.forget).toHaveBeenCalledOnce();
  },
);

it("validates the identity of a directly loaded commit", async () => {
  const entry = {
    oid: commit,
    parents: [parent],
    message: gitPath("Complete message"),
    messageTruncated: false,
    author: { name: "Author", email: "a@example.test" },
    time: 0,
    offsetMinutes: 0,
  };
  const { client, session } = clientWith(entry);
  await expect(client.commit("repo", commit.hex)).resolves.toMatchObject({
    message: gitPath("Complete message"),
  });
  expect(session.request).toHaveBeenCalledWith({
    method: "repo.commit",
    params: { repoId: "repo", commitOid: commit.hex },
  });
  await expect(client.commit("repo", parent.hex)).rejects.toMatchObject({
    code: "PROTOCOL_ERROR",
  });
  expect(session.forget).toHaveBeenCalledOnce();
});

it("requests compact annotations and validates directly loaded tag objects", async () => {
  const { client, session } = clientWith({
    snapshot: "tags",
    entries: [],
    nextCursor: null,
    metadata: {},
  });
  await client.tags("repo", "next");
  expect(session.request).toHaveBeenCalledWith({
    method: "repo.tags",
    params: { repoId: "repo", cursor: "next", messageBytes: 512 },
  });
  session.request.mockResolvedValue({
    oid: commit,
    annotated: true,
    detailsOmitted: false,
    message: gitPath("Full annotation"),
    messageTruncated: false,
  });
  await expect(client.tag("repo", commit.hex)).resolves.toMatchObject({
    oid: commit,
    message: gitPath("Full annotation"),
  });
  await expect(client.tag("repo", parent.hex)).rejects.toMatchObject({
    code: "PROTOCOL_ERROR",
  });
  expect(session.forget).toHaveBeenCalledOnce();
});

it("preserves worktree filters, cursor and page size at the wire boundary", async () => {
  const { client, session } = clientWith({
    snapshot: "snapshot",
    nextCursor: null,
    entries: [],
    metadata: {
      listToken: "token",
      totalEntries: 1001,
      matchingEntries: 0,
      current: null,
      main: null,
    },
  });
  const options = {
    filter: "late",
    branch: "refs/heads/late",
    name: "late",
    pageSize: 20,
  };
  await expect(
    client.worktrees("repo", "cursor", options),
  ).resolves.toMatchObject({
    metadata: { totalEntries: 1001, matchingEntries: 0 },
  });
  expect(session.request).toHaveBeenCalledWith({
    method: "repo.worktrees",
    params: { repoId: "repo", cursor: "cursor", ...options },
  });
});

it("uses a snapshot only to begin a worktree search, then follows its cursor", async () => {
  const { client, session } = clientWith({
    snapshot: "snapshot",
    nextCursor: null,
    entries: [],
    metadata: { listToken: "token" },
  });
  const options = { filter: "late", atSnapshot: "capture" };
  await client.worktrees("repo", undefined, options);
  await client.worktrees("repo", "filtered-next", options);
  expect(session.request.mock.calls[0][0]).toEqual({
    method: "repo.worktrees",
    params: {
      repoId: "repo",
      cursor: undefined,
      filter: "late",
      atSnapshot: "capture",
    },
  });
  expect(session.request.mock.calls[1][0]).toEqual({
    method: "repo.worktrees",
    params: { repoId: "repo", cursor: "filtered-next", filter: "late" },
  });
});

it("reads only the named remote and rejects mismatched response identity", async () => {
  const remote = {
    name: "origin",
    url: "https://example.test/repo",
    pushUrl: null,
    token: "token",
  };
  const session = {
    request: vi.fn().mockResolvedValue(remote),
    forget: vi.fn().mockResolvedValue(undefined),
  };
  const client = new GitRepositoryClient(session);
  await expect(client.remote("repo", "origin")).resolves.toEqual(remote);
  expect(session.request).toHaveBeenCalledExactlyOnceWith({
    method: "repo.remote",
    params: { repoId: "repo", name: "origin" },
  });
  session.request.mockResolvedValue({ ...remote, name: "other" });
  await expect(client.remote("repo", "origin")).rejects.toMatchObject({
    code: "PROTOCOL_ERROR",
  });
  expect(session.forget).toHaveBeenCalledOnce();
});

it("validates remote picker page size, totals, duplicates, and completion", async () => {
  const params = { repoId: "repo", pageSize: 2, filter: "o" };
  const response = {
    snapshot: "snapshot",
    entries: [{ name: "origin" }],
    nextCursor: null,
    metadata: { totalEntries: 1 },
  };
  const session = {
    request: vi.fn().mockResolvedValue(response),
    forget: vi.fn().mockResolvedValue(undefined),
  };
  const client = new GitRepositoryClient(session);
  await expect(client.remoteNames(params)).resolves.toEqual(response);
  expect(session.request).toHaveBeenCalledExactlyOnceWith({
    method: "repo.remote_names",
    params,
  });
  for (const malformed of [
    { ...response, metadata: { totalEntries: 2 } },
    { ...response, nextCursor: "next" },
    {
      ...response,
      entries: [],
      nextCursor: "next",
      metadata: { totalEntries: 2 },
    },
    {
      ...response,
      entries: [{ name: "same" }, { name: "same" }],
      metadata: { totalEntries: 2 },
    },
    {
      ...response,
      entries: [{ name: "a" }, { name: "b" }, { name: "c" }],
      metadata: { totalEntries: 3 },
    },
  ]) {
    session.request.mockResolvedValue(malformed);
    await expect(client.remoteNames(params)).rejects.toMatchObject({
      code: "PROTOCOL_ERROR",
    });
  }
  session.request.mockResolvedValue({
    ...response,
    metadata: { totalEntries: 3 },
  });
  await expect(
    client.remoteNames({ ...params, cursor: "last" }),
  ).resolves.toMatchObject({ entries: [{ name: "origin" }] });
});

it("requests only status metadata and rejects incomplete summary counts", async () => {
  const metadata = {
    head: {
      name: gitPath("refs/heads/main"),
      oid: null,
      unborn: true,
      detached: false,
    },
    operationState: "Clean",
    integration: null,
    ahead: 2,
    behind: 1,
    upstreamRef: null,
    basis: "stored_refs",
    totalEntries: 25000,
    truncated: false,
  };
  const session = {
    request: vi.fn().mockResolvedValue(metadata),
    forget: vi.fn(),
  };
  const client = new GitRepositoryClient(session);
  const signal = new AbortController().signal;
  expect(
    (await client.withSignal(signal).statusSummary("repo")).totalEntries,
  ).toBe(25000);
  expect(session.request).toHaveBeenCalledWith(
    { method: "repo.status_summary", params: { repoId: "repo" } },
    signal,
  );
  session.request.mockResolvedValue({ ...metadata, totalEntries: undefined });
  await expect(client.statusSummary("repo")).rejects.toMatchObject({
    code: "PROTOCOL_ERROR",
  });
  session.request.mockResolvedValue({ ...metadata, truncated: true });
  await expect(client.statusSummary("repo")).rejects.toMatchObject({
    code: "PROTOCOL_ERROR",
  });
});
