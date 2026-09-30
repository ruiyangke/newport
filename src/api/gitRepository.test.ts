import { expect, it, vi } from "vitest";
import { GitRepositoryClient } from "./gitRepository";
import { gitPath } from "../domain/git";

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
    },
  });
  expect(session.forget).not.toHaveBeenCalled();
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
