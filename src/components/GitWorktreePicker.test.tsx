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

function setup(worktrees: () => Promise<unknown>) {
  const opened: string[] = [];
  const client = {
    worktrees: vi.fn(worktrees),
    status: vi.fn(async (repoId: string) =>
      statusOf(repoId === "repo:/app-agent-fix" ? 3 : 0),
    ),
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
  expect(client.status).not.toHaveBeenCalled();
  expect(option("agent-fix").textContent).not.toMatch(/change|Clean/);
  await act(async () => button("Check status").click());
  expect(opened).toEqual(["/app", "/app-agent-fix"]);
  expect(option("agent-fix").textContent).toContain("3 changes");
  expect(option("Main worktree").textContent).toContain("Clean");
  expect(option("gone").textContent).not.toMatch(/change|Clean/);
});

it("steps aside for an agent that cannot list worktrees", async () => {
  const { render } = setup(async () => {
    throw new Error("Update the server agent to use this Git feature.");
  });
  await render();
  expect(button("Worktrees: Main worktree")).toBeUndefined();
});
