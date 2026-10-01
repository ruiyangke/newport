// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { notifyManager } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { GitWorktreeControls } from "./GitWorktreeControls";
import { gitPath } from "../domain/git";
import { gitQueries } from "../query/git";
import { TooltipProvider } from "./ui/tooltip";
import type { GitRepositoryClient } from "../api/gitRepository";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";
import {
  decodeGitRepository,
  decodeGitWorktrees,
  decodeGitBranches,
  type GitWorktrees,
} from "../domain/gitResponses";

// Query delivers results on a timer by default; act() only flushes microtasks.
notifyManager.setScheduler(queueMicrotask);
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

it("ignores a late worktree page after the inspector has been reopened", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const page = (
    message: string,
    snapshot: string,
    nextCursor: string | null,
  ): GitWorktrees => ({
    snapshot,
    nextCursor,
    metadata: { listToken: snapshot },
    entries: decodeGitWorktrees({
      snapshot,
      nextCursor: null,
      metadata: { listToken: snapshot },
      entries: [
        {
          name: { display: message, bytesB64: btoa(message) },
          kind: "linked",
          state: "available",
          path: {
            display: "/repo/" + message,
            bytesB64: btoa("/repo/" + message),
          },
          gitDir: {
            display: "/repo/.git/worktrees/" + message,
            bytesB64: btoa("/repo/.git/worktrees/" + message),
          },
          current: false,
          head: null,
          locked: false,
          lockReason: null,
          prunable: false,
        },
      ],
    }).entries,
  });
  let resolvePage!: (value: GitWorktrees) => void;
  const worktrees = vi
    .fn()
    .mockResolvedValueOnce(page("Original worktree", "old", "next"))
    .mockImplementationOnce(
      () =>
        new Promise<GitWorktrees>((resolve) => {
          resolvePage = resolve;
        }),
    )
    .mockResolvedValueOnce(page("Fresh worktree", "new", null));
  const client = { worktrees } as unknown as GitRepositoryClient;
  seedGitClient(client);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const click = async (text: string) =>
    act(async () => {
      const button = [...document.querySelectorAll("button")].find(
        (button) =>
          button.textContent === text ||
          button.getAttribute("aria-label") === text,
      );
      expect(button).toBeTruthy();
      button!.click();
    });
  try {
    await act(async () =>
      root.render(
        <GitTestProviders queryClient={createTestQueryClient()}>
          <TooltipProvider>
            <GitWorktreeControls
              repository={decodeGitRepository({
                repoId: "repo",
                commonRepoId: "common",
                root: { display: "/repo", bytesB64: btoa("/repo") },
                bare: false,
                objectFormat: "sha1",
                head: { name: null, oid: null, detached: true, unborn: false },
                operationState: "Clean",
                integration: null,
                capabilities: { readOnly: false, workingTree: true },
              })}
              busy={false}
              error=""
              onAction={vi.fn()}
            />
          </TooltipProvider>
        </GitTestProviders>,
      ),
    );
    await click("Worktrees");
    await click("Load more worktrees");
    await click("Close inspector");
    await click("Worktrees");
    expect(document.body.textContent).toContain("Fresh worktree");
    await act(async () =>
      resolvePage(page("Late stale worktree", "old", null)),
    );
    expect(document.body.textContent).toContain("Fresh worktree");
    expect(document.body.textContent).not.toContain("Late stale worktree");
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});

const row = (name: string) => ({
  name: gitPath(name),
  kind: "linked",
  state: "available",
  path: gitPath(`/repo-${name}`),
  gitDir: gitPath(`/repo/.git/worktrees/${name}`),
  current: false,
  head: null,
  locked: false,
  lockReason: null,
  prunable: false,
});
const worktreePage = (names: string[], nextCursor: string | null = null) =>
  decodeGitWorktrees({
    snapshot: "s",
    metadata: { listToken: "s" },
    nextCursor,
    entries: names.map(row),
  });
const branchPage = (names: string[], nextCursor: string | null = null) =>
  decodeGitBranches({
    snapshot: "b",
    metadata: {},
    nextCursor,
    entries: names.map((name) => ({
      name: gitPath(name),
      reference: gitPath(`refs/heads/${name}`),
      oid: { algorithm: "sha1", hex: "a".repeat(40) },
      remote: false,
      current: false,
      upstream: null,
      tracking: null,
    })),
  });
async function mountManager(
  client: Partial<GitRepositoryClient>,
  cached = false,
) {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const scope = seedGitClient(client);
  const queryClient = createTestQueryClient();
  if (cached)
    queryClient.setQueryData(
      gitQueries.worktrees(scope, "repo").queryKey,
      worktreePage(["cached"]),
    );
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const onAction = vi.fn(async () => false);
  const repository = decodeGitRepository({
    repoId: "repo",
    commonRepoId: "common",
    root: gitPath("/repo"),
    bare: false,
    objectFormat: "sha1",
    head: { name: null, oid: null, detached: true, unborn: false },
    operationState: "Clean",
    integration: null,
    capabilities: { readOnly: false, workingTree: true },
  });
  await act(async () =>
    root.render(
      <GitTestProviders queryClient={queryClient}>
        <GitWorktreeControls
          open
          repository={repository}
          busy={false}
          error=""
          onAction={onAction}
        />
      </GitTestProviders>,
    ),
  );
  const button = (name: string) =>
    [...document.querySelectorAll("button")].find(
      (button) => button.textContent === name,
    )!;
  return {
    onAction,
    button,
    click: (name: string) => act(async () => button(name).click()),
    close: async () => {
      await act(async () => root.unmount());
      queryClient.clear();
      host.remove();
    },
  };
}

it("appends worktrees in order with deduplication and leaves row actions usable during continuation", async () => {
  let finish!: (page: GitWorktrees) => void;
  const read = vi.fn(async (_repo, cursor) =>
    cursor
      ? new Promise<GitWorktrees>((resolve) => {
          finish = resolve;
        })
      : worktreePage(["first"], "next"),
  );
  const ui = await mountManager({ worktrees: read });
  try {
    await ui.click("Load more worktrees");
    expect(ui.button("Lock…").disabled).toBe(false);
    expect(ui.button("Add worktree").disabled).toBe(false);
    await act(async () => finish(worktreePage(["first", "second"])));
    expect(
      [...document.querySelectorAll(".git-worktree-list strong")].map(
        (node) => node.textContent,
      ),
    ).toEqual(["first", "second"]);
    expect(document.body.textContent).toContain("All worktrees loaded");
    await ui.click("Lock…");
    await ui.click("Lock worktree");
    expect(ui.onAction).toHaveBeenCalledWith(
      { kind: "worktree.lock", name: "first" },
      "s",
    );
    expect(document.querySelector("form")).not.toBeNull(); // rejected operation keeps the confirmation
  } finally {
    await ui.close();
  }
});

it("retains rows on a continuation error and shows a structured retry", async () => {
  const ui = await mountManager({
    worktrees: vi.fn(async (_repo, cursor) => {
      if (cursor)
        throw { code: "STALE_SNAPSHOT", message: "Worktrees changed" };
      return worktreePage(["first"], "next");
    }),
  });
  try {
    await ui.click("Load more worktrees");
    expect(document.body.textContent).toContain(
      "Worktrees changed (STALE_SNAPSHOT)",
    );
    expect(document.body.textContent).not.toContain("[object Object]");
    expect(ui.button("Retry: load more worktrees")).toBeDefined();
    expect(
      document.querySelector(".git-worktree-list strong")?.textContent,
    ).toBe("first");
    expect(ui.button("Lock…").disabled).toBe(false);
  } finally {
    await ui.close();
  }
});

it("shows cached worktrees but keeps actions inert until the first fresh read", async () => {
  let finish!: (page: GitWorktrees) => void;
  const ui = await mountManager(
    {
      worktrees: vi.fn(
        () =>
          new Promise<GitWorktrees>((resolve) => {
            finish = resolve;
          }),
      ),
    },
    true,
  );
  try {
    expect(document.body.textContent).toContain("cached");
    expect(ui.button("Lock…").disabled).toBe(true);
    await act(async () => finish(worktreePage(["fresh"])));
    expect(document.body.textContent).not.toContain("cached");
    expect(ui.button("Lock…").disabled).toBe(false);
  } finally {
    await ui.close();
  }
});

it("loads one branch page ahead and appends choices instead of replacing them", async () => {
  Element.prototype.scrollIntoView ??= () => {};
  const branches = vi.fn(async (_repo, cursor) =>
    cursor ? branchPage(["first", "second"]) : branchPage(["first"], "next"),
  );
  const ui = await mountManager({
    worktrees: vi.fn(async () => worktreePage(["checkout"])),
    branches,
  });
  try {
    expect(branches).not.toHaveBeenCalled();
    await ui.click("Add worktree");
    await act(async () =>
      document
        .querySelector('[aria-label="Local branch"]')!
        .dispatchEvent(
          new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
        ),
    );
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 170));
    });
    expect(branches).toHaveBeenCalledTimes(2);
    await ui.click("Load more branches");
    expect(branches).toHaveBeenCalledTimes(2);
    expect(
      [...document.querySelectorAll('[role="option"]')].map(
        (node) => node.textContent,
      ),
    ).toEqual(["first", "second"]);
  } finally {
    await ui.close();
  }
});
