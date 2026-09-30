import { QueryClient } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { gitKeys, gitQueries, invalidateRepository } from "./git";
import { gitPath } from "../domain/git";
import { decodeGitStatus } from "../domain/gitResponses";

const projects = vi.hoisted(() => ({
  open: vi.fn(),
  repositories: {
    statusSummary: vi.fn(),
    status: vi.fn(),
    worktrees: vi.fn(),
    withSignal: vi.fn(),
  },
}));
vi.mock("../git/registry", () => ({ gitProjectsFor: () => projects }));
beforeEach(() => {
  vi.clearAllMocks();
  projects.repositories.withSignal.mockReturnValue(projects.repositories);
  projects.repositories.statusSummary.mockImplementation(
    async () => (await projects.repositories.status()).metadata,
  );
  projects.open.mockResolvedValue({ repoId: "repo" });
});
it("separates branch searches, kinds, sizes and cursors under one invalidation prefix", () => {
  const scope = {
    id: "server",
    connection: "server:0:",
    destination: "server",
  };
  const options = [
    { filter: "feature" },
    { filter: "release" },
    { branchKind: "remote" as const },
    { pageSize: 50 },
  ];
  const keys = options.flatMap((option) =>
    [undefined, "next"].map((cursor) =>
      gitKeys.branches(scope, "repo", cursor, option),
    ),
  );
  expect(new Set(keys.map((key) => JSON.stringify(key))).size).toBe(8);
  for (const key of keys)
    expect(key.slice(0, gitKeys.branchRoot(scope, "repo").length)).toEqual(
      gitKeys.branchRoot(scope, "repo"),
    );
});
it.each([
  { total: 320, next: "next", truncated: false, expected: 320 },
  { total: 32000, next: "next", truncated: true, expected: 32000 },
  { total: 0, next: null, truncated: false, expected: 0 },
])(
  "does not present a page length as the total: %j",
  async ({ total, next, truncated, expected }) => {
    projects.repositories.status.mockResolvedValue(
      decodeGitStatus({
        snapshot: "snapshot",
        nextCursor: next,
        entries: [
          {
            entryId: "entry",
            path: gitPath("file"),
            oldPath: null,
            flags: 256,
            staged: false,
            unstaged: true,
            untracked: false,
            conflicted: false,
            conflict: null,
          },
        ],
        metadata: {
          head: {
            name: gitPath("refs/heads/main"),
            oid: null,
            unborn: true,
            detached: false,
          },
          operationState: "Clean",
          integration: null,
          ahead: null,
          behind: null,
          upstreamRef: null,
          basis: "stored_refs",
          totalEntries: total,
          truncated,
        },
      }),
    );
    const queryClient = new QueryClient();
    try {
      const result = await queryClient.fetchQuery(
        gitQueries.checkout(
          { id: "server", connection: "server:0:", destination: "server" },
          {
            id: "project",
            serverId: "server",
            name: "Application",
            path: gitPath("/repo"),
          },
        ),
      );
      expect(result.changes).toBe(expected);
      expect(projects.repositories.statusSummary).toHaveBeenCalledWith(
        gitPath("/repo"),
      );
      expect(projects.repositories.status).toHaveBeenCalledTimes(1);
    } finally {
      queryClient.clear();
    }
  },
);

it("keeps content-addressed commit messages after repository writes", async () => {
  const client = new QueryClient();
  const scope = {
    id: "server",
    connection: "server:0:",
    destination: "server",
  };
  const key = gitKeys.commit(scope, "repo", "A".repeat(40));
  client.setQueryData(key, { message: "immutable" });
  client.setQueryData(gitKeys.history(scope, "repo"), { entries: [] });
  await invalidateRepository(client, scope, "repo");
  expect(client.getQueryState(key)?.isInvalidated).toBe(false);
  expect(
    client.getQueryState(gitKeys.history(scope, "repo"))?.isInvalidated,
  ).toBe(true);
  expect(key).toEqual(gitKeys.commit(scope, "repo", "a".repeat(40)));
  client.clear();
});

it("keeps loaded status pages on unchanged polling but discards them on snapshot or prefix changes", async () => {
  const scope = {
    id: "server",
    connection: "server:0:",
    destination: "server",
  };
  const client = new QueryClient();
  const first = decodeGitStatus({
    snapshot: "same",
    nextCursor: "page2",
    entries: [],
    metadata: {
      head: { name: null, oid: null, unborn: true, detached: false },
      operationState: "Clean",
      integration: null,
      ahead: null,
      behind: null,
      upstreamRef: null,
      basis: "stored_refs",
      totalEntries: 12000,
      truncated: false,
    },
  });
  const row = {
    entryId: "later",
    path: gitPath("later.txt"),
    oldPath: null,
    flags: 256,
    staged: false,
    unstaged: true,
    untracked: false,
    conflicted: false,
    conflict: null,
  };
  const loaded = { ...first, entries: [row], nextCursor: "page3" };
  const query = gitQueries.status(scope, "repo");
  try {
    projects.repositories.status.mockResolvedValue(first);
    await client.fetchQuery({ ...query, staleTime: 0 });
    client.setQueryData(query.queryKey, loaded);
    await client.fetchQuery({ ...query, staleTime: 0 });
    expect(client.getQueryData(query.queryKey)).toEqual(loaded);
    projects.repositories.status.mockResolvedValue({
      ...first,
      snapshot: "changed",
    });
    await client.fetchQuery({ ...query, staleTime: 0 });
    expect(client.getQueryData(query.queryKey)).toEqual({
      ...first,
      snapshot: "changed",
    });
    client.setQueryData(query.queryKey, {
      ...loaded,
      entries: [row, { ...row, entryId: "second" }],
    });
    projects.repositories.status.mockResolvedValue({
      ...first,
      entries: [{ ...row, flags: 1 }],
    });
    await client.fetchQuery({ ...query, staleTime: 0 });
    expect(client.getQueryData(query.queryKey)).toEqual({
      ...first,
      entries: [{ ...row, flags: 1 }],
    });
  } finally {
    client.clear();
  }
});

it("publishes project status without waiting for worktree discovery", async () => {
  projects.repositories.status.mockResolvedValue({
    metadata: {
      head: { name: gitPath("refs/heads/main"), detached: false },
      totalEntries: 4,
      ahead: 2,
    },
    entries: [],
    nextCursor: null,
  });
  projects.repositories.worktrees.mockImplementation(
    () => new Promise(() => {}),
  );
  const client = new QueryClient();
  try {
    const result = await client.fetchQuery(
      gitQueries.summary(
        { id: "server", connection: "server:0:", destination: "server" },
        { id: "p", serverId: "server", name: "App", path: gitPath("/repo") },
      ),
    );
    expect(result).toMatchObject({ branch: "main", changes: 4, outgoing: 2 });
    expect(projects.repositories.worktrees).not.toHaveBeenCalled();
  } finally {
    client.clear();
  }
});

it("cancels project summary reads without opening a repository", async () => {
  let finish!: (value: unknown) => void;
  let signal!: AbortSignal;
  projects.repositories.withSignal.mockImplementation((incoming) => {
    signal = incoming;
    return projects.repositories;
  });
  projects.repositories.statusSummary.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const client = new QueryClient();
  const query = gitQueries.summary(
    { id: "server", connection: "server:0:", destination: "server" },
    { id: "p", serverId: "server", name: "App", path: gitPath("/repo") },
  );
  const pending = client.fetchQuery(query).catch((error) => error);
  await client.cancelQueries({ queryKey: query.queryKey });
  expect(signal.aborted).toBe(true);
  finish({ head: { detached: true }, totalEntries: 0, ahead: 0 });
  await pending;
  expect(projects.open).not.toHaveBeenCalled();
  expect(client.getQueryData(query.queryKey)).toBeUndefined();
  client.clear();
});

it("separates status searches and invalidates other filters after a guarded write", async () => {
  const scope = {
    id: "server",
    connection: "server:0:",
    destination: "server",
  };
  const all = gitKeys.status(scope, "repo");
  const current = gitKeys.status(scope, "repo", {
    text: " KEEP ",
    group: "untracked",
  });
  expect(current).toEqual(
    gitKeys.status(scope, "repo", { text: "keep", group: "untracked" }),
  );
  expect(gitKeys.status(scope, "repo", { text: "  ", group: "all" })).toEqual(
    all,
  );
  const other = gitKeys.status(scope, "repo", { text: "skip" });
  const client = new QueryClient();
  for (const key of [all, current, other])
    client.setQueryData(key, { snapshot: "s" });
  await invalidateRepository(client, scope, "repo", [current]);
  expect(client.getQueryState(current)?.isInvalidated).toBe(false);
  expect(client.getQueryState(all)?.isInvalidated).toBe(true);
  expect(client.getQueryState(other)?.isInvalidated).toBe(true);
  client.clear();
});
