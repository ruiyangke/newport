import { QueryClient } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import {
  gitProjectsFor,
  gitResources,
  resetGitProjects,
  retainGitProjects,
} from "./registry";
import {
  forgetRepositories,
  gitKeys,
  gitQueries,
  invalidateRepository,
} from "../query/git";
import {
  gitStore,
  patchGitState,
  readGitState,
  retainGitState,
  setGitState,
} from "../state/git";

const scope = (id: string, connection = `${id}:0:`) => ({
  id,
  connection,
  destination: id,
});

beforeEach(() => {
  retainGitProjects(new Set());
  gitStore.setState(() => ({}));
  invoke.mockReset();
  invoke.mockResolvedValue(undefined);
});

it("hands every reader of a connection the same session", () => {
  const a = gitProjectsFor(scope("a"));
  expect(gitProjectsFor(scope("a"))).toBe(a);
  // A new connection revision is a new session: its repository ids are not
  // the old one's.
  expect(gitProjectsFor(scope("a", "a:1:"))).not.toBe(a);
});

it("reset releases the old session and starts a fresh one", async () => {
  const before = gitProjectsFor(scope("a"));
  const dispose = vi.spyOn(before, "dispose");
  const after = resetGitProjects(scope("a"));
  expect(dispose).toHaveBeenCalledOnce();
  expect(after).not.toBe(before);
  expect(gitProjectsFor(scope("a"))).toBe(after);
});

it("retains only the sessions of valid connections", () => {
  const keep = gitProjectsFor(scope("a"));
  const drop = gitProjectsFor(scope("b"));
  const dispose = vi.spyOn(drop, "dispose");
  retainGitProjects(new Set(["a:0:"]));
  expect(dispose).toHaveBeenCalledOnce();
  expect([...gitResources.values()]).toEqual([keep]);
});

it("keeps every repository read under the prefix a write invalidates", () => {
  const s = scope("a");
  const prefix = gitKeys.repo(s, "r1");
  for (const key of [
    gitKeys.status(s, "r1"),
    gitKeys.history(s, "r1"),
    gitKeys.branches(s, "r1"),
    gitKeys.remotes(s, "r1"),
    gitKeys.stashes(s, "r1"),
    gitKeys.tags(s, "r1"),
    gitKeys.worktrees(s, "r1"),
    gitKeys.blob(s, "r1", "ABC"),
    gitKeys.diff(s, {
      repoId: "r1",
      snapshot: "s",
      side: "head_to_index",
      path: { display: "a", bytesB64: "YQ==" },
    } as never),
  ])
    expect(key.slice(0, prefix.length)).toEqual([...prefix]);
  // Another repository's reads are not caught by it.
  expect(gitKeys.status(s, "r2").slice(0, prefix.length)).not.toEqual([
    ...prefix,
  ]);
});

it("never refreshes the one read that contacts the remote on its own", () => {
  const options = gitQueries.remoteRefs(scope("a"), {
    repoId: "r1",
    remote: "origin",
    expectedToken: "t",
  });
  expect(options.staleTime).toBe(Infinity);
  expect(options.refetchOnMount).toBe(false);
});

it("forgets a reset session's repository reads but not its bookmarks", () => {
  const client = new QueryClient();
  const s = scope("a");
  client.setQueryData(gitKeys.status(s, "r1"), { snapshot: "x" });
  client.setQueryData(gitKeys.projects(s), []);
  forgetRepositories(client, s);
  expect(client.getQueryData(gitKeys.status(s, "r1"))).toBeUndefined();
  expect(client.getQueryData(gitKeys.projects(s))).toEqual([]);
});

it("keeps page state per connection and prunes it with the session", () => {
  setGitState("a", "fileFilter", "src");
  setGitState("a", "favourites", (current) => new Set([...current, "p1"]));
  patchGitState("b", { tab: "history", selectedCommit: "c" });
  expect(readGitState("a").fileFilter).toBe("src");
  expect([...readGitState("a").favourites]).toEqual(["p1"]);
  expect(readGitState("b").tab).toBe("history");
  // An untouched connection reads the defaults rather than nothing.
  expect(readGitState("z").tab).toBe("changes");
  retainGitState(new Set(["b"]));
  expect(Object.keys(gitStore.state)).toEqual(["b"]);
});

it("a write never re-runs a content-addressed read or the remote one", async () => {
  const client = new QueryClient();
  const s = scope("a");
  const seed = (key: readonly unknown[]) => client.setQueryData(key, 1);
  const status = gitKeys.status(s, "r1");
  const branches = gitKeys.branches(s, "r1");
  const blob = gitKeys.blob(s, "r1", "abc");
  const refs = gitKeys.remoteRefs(s, {
    repoId: "r1",
    remote: "origin",
    expectedToken: "t",
  });
  for (const key of [status, branches, blob, refs]) seed(key);
  await invalidateRepository(client, s, "r1", ["status"]);
  const stale = (key: readonly unknown[]) =>
    client.getQueryState(key)?.isInvalidated;
  expect(stale(branches)).toBe(true);
  // Named as already re-read by the caller.
  expect(stale(status)).toBe(false);
  // Fixed by its key, and the one that would contact the remote.
  expect(stale(blob)).toBe(false);
  expect(stale(refs)).toBe(false);
});
