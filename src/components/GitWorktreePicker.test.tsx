// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { notifyManager } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GitWorktreePicker } from "./GitWorktreePicker";
import type { GitRepositoryClient } from "../api/gitRepository";
import type { GitProjects } from "../api/gitProjects";
import {
  decodeGitRepository,
  decodeGitStatus,
  decodeGitWorktrees,
} from "../domain/gitResponses";
import { gitPath, type GitPath } from "../domain/git";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";
import { gitResources } from "../git/registry";

notifyManager.setScheduler(queueMicrotask);
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= () => {};
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

const oid = { algorithm: "sha1", hex: "a".repeat(40) };
const head = (branch: string) => ({
  name: gitPath(`refs/heads/${branch}`),
  oid,
  unborn: false,
  detached: false,
});
function wt(
  name: string | null,
  path: string,
  branch: string,
  extra: Record<string, unknown> = {},
) {
  return {
    name: name === null ? null : gitPath(name),
    kind: name === null ? "main" : "linked",
    state: "available",
    path: gitPath(path),
    gitDir: gitPath(
      name === null ? "/app/.git" : `/app/.git/worktrees/${name}`,
    ),
    current: false,
    head: head(branch),
    locked: false,
    lockReason: null,
    prunable: false,
    ...extra,
  };
}
const listing = decodeGitWorktrees({
  snapshot: "worktrees",
  nextCursor: null,
  metadata: { listToken: "token" },
  entries: [
    wt(null, "/app", "main", { current: true }),
    wt("agent-fix", "/app-agent-fix", "agent/fix"),
    wt("gone", "/tmp/gone", "spike", { state: "missing", prunable: true }),
  ],
});
const repository = decodeGitRepository({
  repoId: "repo:/app",
  commonRepoId: "common",
  root: gitPath("/app"),
  bare: false,
  objectFormat: "sha1",
  head: head("main"),
  operationState: "Clean",
  integration: null,
  capabilities: { readOnly: false, workingTree: true },
});
const project = {
  id: "project",
  serverId: "git-test-server",
  name: "App",
  path: gitPath("/app"),
};
function statusOf(count: number) {
  return decodeGitStatus({
    snapshot: `s${count}`,
    nextCursor: null,
    entries: Array.from({ length: count }, (_, i) => ({
      entryId: `e${i}`,
      path: gitPath(`f${i}`),
      oldPath: null,
      flags: 256,
      staged: false,
      unstaged: true,
      untracked: false,
      conflicted: false,
      conflict: null,
    })),
    metadata: {
      totalEntries: count,
      head: head("main"),
      operationState: "Clean",
      integration: null,
      ahead: count ? 2 : 0,
      behind: 0,
      basis: "stored_refs",
      upstreamRef: null,
    },
  });
}

let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  document.body.innerHTML = "";
});

function setup(
  worktrees: (
    repoId: string,
    cursor?: string,
    options?: { filter?: string },
  ) => Promise<unknown>,
) {
  const opened: string[] = [];
  const client = {
    worktrees: vi.fn(worktrees),
    statusSummary: vi.fn(async (path: GitPath) => {
      opened.push(path.display);
      return statusOf(path.display === "/app-agent-fix" ? 3 : 0).metadata;
    }),
  } as unknown as GitRepositoryClient;
  const scope = seedGitClient(client);
  // The summary read opens each worktree by path, as GitProjects does.
  Object.assign(gitResources.get(scope.connection) as GitProjects, {
    open: async (target: { path: GitPath }) => {
      opened.push(target.path.display);
      return { ...repository, repoId: `repo:${target.path.display}` };
    },
  });
  const onOpen = vi.fn();
  const onNew = vi.fn();
  const onManage = vi.fn();
  const render = () =>
    act(async () =>
      root.render(
        <GitTestProviders queryClient={createTestQueryClient()}>
          <GitWorktreePicker
            project={project}
            repository={repository}
            busy={false}
            onOpen={onOpen}
            onNew={onNew}
            onManage={onManage}
            onFiles={vi.fn()}
          />
        </GitTestProviders>,
      ),
    );
  return { client, opened, onOpen, onNew, onManage, render };
}
const button = (name: string) =>
  [...document.querySelectorAll("button")].find(
    (b) => b.textContent === name || b.getAttribute("aria-label") === name,
  )!;
const option = (name: string) =>
  [...document.querySelectorAll<HTMLElement>('[role="option"]')].find(
    (row) => row.querySelector("strong")?.textContent === name,
  )!;

it("lists every checkout, and opens the one chosen", async () => {
  const { render, onOpen } = setup(async () => listing);
  await render();
  await act(async () => button("Worktrees: Main worktree").click());
  expect(option("Main worktree").getAttribute("aria-current")).toBe("true");
  expect(option("agent-fix").textContent).toContain("agent/fix");
  expect(option("agent-fix").textContent).toContain("/app-agent-fix");
  expect(option("gone").textContent).toContain("missing");
  await act(async () => option("agent-fix").click());
  expect(onOpen).toHaveBeenCalledTimes(1);
  expect(onOpen.mock.calls[0][0].name.display).toBe("agent-fix");
});

it("offers nothing to open for the current or a missing checkout", async () => {
  const { render, onOpen } = setup(async () => listing);
  await render();
  await act(async () => button("Worktrees: Main worktree").click());
  await act(async () => option("Main worktree").click());
  await act(async () => option("gone").click());
  expect(onOpen).not.toHaveBeenCalled();
});

it("reads each worktree's status only when asked, and never a missing one", async () => {
  const { render, client, opened } = setup(async () => listing);
  await render();
  await act(async () => button("Worktrees: Main worktree").click());
  // Listing is not reading: no status was asked for on sight.
  expect(client.statusSummary).not.toHaveBeenCalled();
  expect(option("agent-fix").textContent).not.toMatch(/change|Clean/);
  await act(async () => button("Check loaded").click());
  expect(opened).toEqual(["/app", "/app-agent-fix"]);
  expect(option("agent-fix").textContent).toContain("3 changes");
  expect(option("Main worktree").textContent).toContain("Clean");
  expect(option("gone").textContent).not.toMatch(/change|Clean/);
});

it("keeps a retryable error visible for an agent that cannot list worktrees", async () => {
  const { render } = setup(async () => {
    throw new Error("Update the server agent to use this Git feature.");
  });
  await render();
  await act(async () => button("Worktrees: Main worktree").click());
  expect(document.body.textContent).toContain("Update the server agent");
  expect(button("Refresh worktrees")).toBeTruthy();
});

it("loads only the first page while closed, preserves current metadata and deduplicates continuation retries", async () => {
  let fail = true;
  const current = wt("current-late", "/late", "late", { current: true });
  const page = decodeGitWorktrees({
    snapshot: "paged",
    nextCursor: "next",
    entries: [wt("first", "/first", "first")],
    metadata: {
      listToken: "token",
      totalEntries: 1001,
      matchingEntries: 1001,
      current,
      main: wt(null, "/app", "main"),
    },
  });
  const { render, client, onOpen } = setup(async (_repo, cursor) => {
    if (!cursor) return page;
    if (fail) throw { code: "IO_ERROR", message: "Page unavailable" };
    return {
      ...page,
      nextCursor: null,
      entries: [
        ...page.entries,
        decodeGitWorktrees({
          snapshot: "paged",
          nextCursor: null,
          metadata: { listToken: "token" },
          entries: [wt("second", "/second", "second")],
        }).entries[0],
      ],
    };
  });
  await render();
  expect(client.worktrees).toHaveBeenCalledTimes(1);
  expect(button("Worktrees: current-late")).toBeTruthy();
  await act(async () => button("Worktrees: current-late").click());
  expect(document.body.textContent).toContain("1 loaded · 1,001 total");
  await act(async () => button("Load more worktrees").click());
  expect(document.body.textContent).toContain("Page unavailable (IO_ERROR)");
  expect(option("first")).toBeTruthy();
  fail = false;
  await act(async () => button("Retry: load more worktrees").click());
  expect(document.querySelectorAll('[role="option"]')).toHaveLength(2);
  expect(document.body.textContent).toContain("All worktrees loaded");
  await act(async () => option("second").click());
  expect(onOpen.mock.calls[0][0].path.display).toBe("/second");
});

it("searches on the server and stops offering stale filtered rows", async () => {
  let resolve!: (value: unknown) => void;
  const read = vi.fn(
    async (_repo: string, _cursor?: string, options?: { filter?: string }) => {
      if (options?.filter === "late")
        return new Promise((done) => {
          resolve = done;
        });
      return listing;
    },
  );
  const { render, onOpen } = setup(read);
  await render();
  await act(async () => button("Worktrees: Main worktree").click());
  const input = document.querySelector<HTMLInputElement>(
    'input[aria-label="Filter worktrees"]',
  )!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(input, "late");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => option("agent-fix").click());
  expect(onOpen).not.toHaveBeenCalled();
  await act(async () => new Promise((done) => setTimeout(done, 220)));
  expect(read).toHaveBeenLastCalledWith("repo:/app", undefined, {
    filter: "late",
    atSnapshot: "worktrees",
  });
  await act(async () =>
    resolve(
      decodeGitWorktrees({
        snapshot: "filtered",
        nextCursor: null,
        metadata: {
          listToken: "token",
          totalEntries: 1001,
          matchingEntries: 1,
          current: listing.entries[0],
        },
        entries: [wt("late-result", "/late-result", "late")],
      }),
    ),
  );
  expect(option("agent-fix")).toBeUndefined();
  expect(option("late-result")).toBeTruthy();
  expect(document.body.textContent).toContain(
    "1 loaded · 1 matching · 1,001 total",
  );
});

it("refreshes an expired captured search before trying that filter again", async () => {
  let refreshed = false;
  const read = vi.fn(
    async (
      _repo: string,
      _cursor?: string,
      options?: { filter?: string; atSnapshot?: string },
    ) => {
      if (options?.filter && options.atSnapshot === "worktrees")
        throw { code: "SNAPSHOT_EXPIRED", message: "Listing changed" };
      return {
        ...listing,
        snapshot: refreshed ? "fresh-listing" : "worktrees",
      };
    },
  );
  const { render } = setup(read);
  await render();
  await act(async () => button("Worktrees: Main worktree").click());
  const type = async () => {
    const input = document.querySelector<HTMLInputElement>(
      'input[aria-label="Filter worktrees"]',
    )!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )!.set!.call(input, "late");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => new Promise((done) => setTimeout(done, 220)));
  };
  await type();
  expect(document.body.textContent).toContain(
    "Listing changed (SNAPSHOT_EXPIRED)",
  );
  refreshed = true;
  await act(async () => button("Refresh worktrees").click());
  expect(
    document.querySelector<HTMLInputElement>(
      'input[aria-label="Filter worktrees"]',
    )!.value,
  ).toBe("");
  await type();
  expect(read).toHaveBeenLastCalledWith("repo:/app", undefined, {
    filter: "late",
    atSnapshot: "fresh-listing",
  });
  expect(document.body.textContent).not.toContain("Listing changed");
});

it("aborts status reads already started by Check loaded", async () => {
  const { client, render } = setup(async () => listing);
  const checks: {
    signal: AbortSignal;
    resolve: (value: ReturnType<typeof statusOf>["metadata"]) => void;
  }[] = [];
  client.withSignal = vi.fn(
    (signal) =>
      ({
        ...client,
        statusSummary: () =>
          new Promise((resolve) => {
            checks.push({ signal, resolve });
          }),
      }) as unknown as GitRepositoryClient,
  );
  await render();
  await act(async () => button("Worktrees: Main worktree").click());
  await act(async () => button("Check loaded").click());
  expect(checks).toHaveLength(2);
  await act(async () => button("Worktrees: Main worktree").click());
  expect(checks.every((check) => check.signal.aborted)).toBe(true);
  await act(async () => {
    for (const check of checks) check.resolve(statusOf(99).metadata);
  });
  await act(async () => button("Worktrees: Main worktree").click());
  expect(document.body.textContent).not.toContain("99 changes");
});
