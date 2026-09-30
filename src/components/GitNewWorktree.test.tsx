// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { notifyManager } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GitNewWorktree } from "./GitNewWorktree";
import type { GitRepositoryClient } from "../api/gitRepository";
import {
  decodeGitBranches,
  decodeGitRepository,
  decodeGitWorktrees,
} from "../domain/gitResponses";
import { gitPath } from "../domain/git";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";

notifyManager.setScheduler(queueMicrotask);
Element.prototype.scrollIntoView ??= () => {};
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const main = { algorithm: "sha1", hex: "a".repeat(40) };
const other = { algorithm: "sha1", hex: "b".repeat(40) };
const head = (branch: string, oid = main) => ({
  name: gitPath(`refs/heads/${branch}`),
  oid,
  unborn: false,
  detached: false,
});
const repository = decodeGitRepository({
  repoId: "repo",
  commonRepoId: "common",
  root: gitPath("/srv/app"),
  bare: false,
  objectFormat: "sha1",
  head: head("main"),
  operationState: "Clean",
  integration: null,
  capabilities: { readOnly: false, workingTree: true },
});
const worktrees = decodeGitWorktrees({
  snapshot: "worktrees-token",
  nextCursor: null,
  metadata: { listToken: "token" },
  entries: [
    {
      name: null,
      kind: "main",
      state: "available",
      path: gitPath("/srv/app"),
      gitDir: gitPath("/srv/app/.git"),
      current: true,
      head: head("main"),
      locked: false,
      lockReason: null,
      prunable: false,
    },
    {
      name: gitPath("taken"),
      kind: "linked",
      state: "available",
      path: gitPath("/srv/app-taken"),
      gitDir: gitPath("/srv/app/.git/worktrees/taken"),
      current: false,
      head: head("feature", other),
      locked: false,
      lockReason: null,
      prunable: false,
    },
  ],
});
const branch = (name: string, oid: typeof main, current = false) => ({
  name: gitPath(name),
  reference: gitPath(`refs/heads/${name}`),
  oid,
  remote: false,
  current,
  upstream: null,
  tracking: null,
});
const branches = decodeGitBranches({
  snapshot: "branches",
  nextCursor: null,
  metadata: {},
  entries: [branch("main", main, true), branch("feature", other)],
});

let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  seedGitClient({
    worktrees: vi.fn(async () => worktrees),
    branches: vi.fn(async () => branches),
  } as unknown as GitRepositoryClient);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  document.body.innerHTML = "";
});

async function render() {
  const onCreate = vi.fn(async () => true);
  await act(async () =>
    root.render(
      <GitTestProviders queryClient={createTestQueryClient()}>
        <GitNewWorktree
          repository={repository}
          serverId="git-test-server"
          busy={false}
          error=""
          onClose={vi.fn()}
          onCreate={onCreate}
        />
      </GitTestProviders>,
    ),
  );
  return onCreate;
}
/** Types into a React-controlled field. */
async function type(label: string, value: string) {
  const input = [...document.querySelectorAll("label")]
    .find((element) => element.textContent?.startsWith(label))!
    .querySelector("input")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  if (label !== "Find a branch")
    await act(async () => new Promise((resolve) => setTimeout(resolve, 220)));
  return input;
}
const field = (label: string) => {
  const element = [...document.querySelectorAll("label")].find((element) =>
    element.textContent?.startsWith(label),
  )!;
  return (element.querySelector("input") ??
    document.getElementById(element.htmlFor)) as HTMLInputElement;
};
const create = () =>
  [...document.querySelectorAll("button")].find(
    (b) => b.textContent === "Create worktree",
  )!;

it("creates a worktree on a new branch from the current one, named and placed by the branch", async () => {
  const onCreate = await render();
  expect(create().disabled).toBe(true);
  await type("Branch name", "agent/fix-login");
  // The name and the location follow the branch until they are edited.
  expect(field("Worktree name").value).toBe("agent-fix-login");
  expect(field("Location on server").value).toBe("/srv/app-agent-fix-login");
  expect(create().disabled).toBe(false);
  await act(async () => create().click());
  expect(onCreate).toHaveBeenCalledWith(
    {
      kind: "worktree.add",
      name: "agent-fix-login",
      path: gitPath("/srv/app-agent-fix-login"),
      branch: "agent/fix-login",
      // Starts at the current branch's commit, and says it is a new branch.
      expectedOid: main.hex,
      locked: false,
      newBranch: true,
    },
    // Guarded by the worktree listing's snapshot, not a status snapshot.
    "worktrees-token",
    true,
  );
});

it("refuses a new branch that already exists, and a worktree name in use", async () => {
  const onCreate = await render();
  await type("Branch name", "feature");
  expect(document.body.textContent).toContain("That branch exists.");
  expect(create().disabled).toBe(true);
  await type("Branch name", "taken");
  expect(document.body.textContent).toContain(
    "A worktree with that name exists.",
  );
  expect(create().disabled).toBe(true);
  // An edited name is kept even as the branch changes.
  await type("Worktree name", "taken-2");
  await type("Branch name", "agent/other");
  expect(field("Worktree name").value).toBe("taken-2");
  expect(create().disabled).toBe(false);
  expect(onCreate).not.toHaveBeenCalled();
});

it("loads later branch pages from the prefetched cursor", async () => {
  const read = vi.fn(async (_repo, cursor) =>
    decodeGitBranches({
      snapshot: "paged",
      nextCursor: cursor ? null : "next",
      metadata: {},
      entries: cursor
        ? [branch("later-branch", other)]
        : [branch("main", main, true)],
    }),
  );
  seedGitClient({
    worktrees: vi.fn(async () => worktrees),
    branches: read,
  } as unknown as GitRepositoryClient);
  await render();
  expect(read).toHaveBeenCalledTimes(2);
  const more = [...document.querySelectorAll("button")].find(
    (button) => button.textContent === "Load more branches",
  )!;
  await act(async () => more.click());
  expect(read).toHaveBeenCalledTimes(2);
  expect(document.body.textContent).toContain("All matching branches loaded");
  await act(async () => {
    document
      .querySelector('[aria-label="Start from"]')!
      .dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
      );
  });
  expect(document.body.textContent).toContain("later-branch");
});

it("searches on the server while preserving the selected starting commit", async () => {
  const read = vi.fn(async (_repo, _cursor, options) =>
    decodeGitBranches({
      snapshot: options?.filter || "all",
      nextCursor: null,
      metadata: {},
      entries: options?.filter
        ? [branch("unloaded-match", other)]
        : [branch("main", main, true)],
    }),
  );
  seedGitClient({
    worktrees: vi.fn(async () => worktrees),
    branches: read,
  } as unknown as GitRepositoryClient);
  const onCreate = await render();
  await type("Branch name", "new-feature");
  await type("Find a branch", "unloaded");
  expect(create().disabled).toBe(true);
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 220));
  });
  expect(read).toHaveBeenLastCalledWith("repo", undefined, {
    filter: "unloaded",
  });
  expect(create().disabled).toBe(false);
  await act(async () => create().click());
  expect(onCreate).toHaveBeenCalledWith(
    expect.objectContaining({ expectedOid: main.hex }),
    "worktrees-token",
    true,
  );
});

it("shows structured branch read failures instead of opaque object text", async () => {
  seedGitClient({
    worktrees: vi.fn(async () => worktrees),
    branches: vi.fn(async () => {
      throw {
        code: "INVALID_REQUEST",
        message: "This agent cannot read filtered branches.",
      };
    }),
  } as unknown as GitRepositoryClient);
  await render();
  expect(document.querySelector('[role="alert"]')?.textContent).toContain(
    "This agent cannot read filtered branches. (INVALID_REQUEST)",
  );
  expect(document.body.textContent).not.toContain("[object Object]");
  expect(create().disabled).toBe(true);
});

it("uses main-worktree metadata and exact name lookup beyond the first page", async () => {
  const lookup = vi.fn(
    async (_repo: string, _cursor?: string, options?: { name?: string }) => ({
      ...worktrees,
      metadata: {
        ...worktrees.metadata,
        main: { ...worktrees.entries[0], path: gitPath("/srv/original") },
      },
      entries:
        options?.name === "hidden"
          ? [{ ...worktrees.entries[1], name: gitPath("hidden") }]
          : [],
      nextCursor: options?.name ? null : "more",
    }),
  );
  seedGitClient({
    worktrees: lookup,
    branches: vi.fn(async () => branches),
  } as unknown as GitRepositoryClient);
  const onCreate = await render();
  await type("Branch name", "hidden");
  expect(field("Location on server").value).toBe("/srv/original-hidden");
  expect(lookup).toHaveBeenCalledWith("repo", undefined, {
    name: "hidden",
    pageSize: 1,
  });
  expect(create().disabled).toBe(true);
  expect(document.body.textContent).toContain(
    "A worktree with that name exists.",
  );
  expect(onCreate).not.toHaveBeenCalled();
  expect(lookup.mock.calls.some((call) => call[1] === "more")).toBe(false);
});
