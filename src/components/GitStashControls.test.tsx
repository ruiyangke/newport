// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { notifyManager } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { GitStashControls } from "./GitStashControls";
import { TooltipProvider } from "./ui/tooltip";
import type { GitRepositoryClient } from "../api/gitRepository";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";
import type { GitStashes } from "../domain/gitResponses";

// Query delivers results on a timer by default; act() only flushes microtasks.
notifyManager.setScheduler(queueMicrotask);

it("ignores a late stash page after the inspector has been reopened", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const page = (
    message: string,
    snapshot: string,
    nextCursor: string | null,
  ): GitStashes => ({
    snapshot,
    nextCursor,
    metadata: { listToken: snapshot },
    entries: [
      {
        index: 0,
        oid: "a".repeat(40),
        previousOid: "0".repeat(40),
        message,
        messageTruncated: false,
        time: 0,
      },
    ],
  });
  let resolvePage!: (value: GitStashes) => void;
  const stashes = vi
    .fn()
    .mockResolvedValueOnce(page("Original stash", "old", "next"))
    .mockImplementationOnce(
      () =>
        new Promise<GitStashes>((resolve) => {
          resolvePage = resolve;
        }),
    )
    .mockResolvedValueOnce(page("Fresh stash", "new", null));
  const client = { stashes } as unknown as GitRepositoryClient;
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
            <GitStashControls
              repoId="repo"
              projectName="Fixture"
              busy={false}
              error=""
              onAction={vi.fn()}
            />
          </TooltipProvider>
        </GitTestProviders>,
      ),
    );
    await click("Stashes");
    await click("Load more stashes");
    await click("Close inspector");
    await click("Stashes");
    expect(document.body.textContent).toContain("Fresh stash");
    await act(async () => resolvePage(page("Late stale stash", "old", null)));
    expect(document.body.textContent).toContain("Fresh stash");
    expect(document.body.textContent).not.toContain("Late stale stash");
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});

vi.mock("./GitCommitInspector", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./GitCommitInspector")>()),
  GitCommitInspector: ({ commit }: { commit: { oid: { hex: string } } }) => (
    <div data-testid="preview">{commit.oid.hex}</div>
  ),
}));

it("reads only selected stash commits and loads untracked details on demand", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const tracked = "a".repeat(40);
  const untracked = "b".repeat(40);
  const commit = vi.fn().mockImplementation(async (_repo, oid: string) => ({
    oid: { hex: oid, algorithm: "sha1" },
    parents:
      oid === tracked
        ? ["c", "d", "b"].map((c) => ({ hex: c.repeat(40), algorithm: "sha1" }))
        : [],
  }));
  const history = vi.fn();
  const stashes = vi.fn().mockResolvedValue({
    snapshot: "snapshot",
    nextCursor: null,
    metadata: { listToken: "list" },
    entries: [
      {
        index: 0,
        oid: tracked,
        previousOid: "0".repeat(40),
        message: "Saved edits",
        messageTruncated: false,
        time: 0,
      },
    ],
  });
  seedGitClient({ stashes, commit, history } as unknown as GitRepositoryClient);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const click = async (text: string) =>
    act(async () => {
      const button = [...document.querySelectorAll("button")].find((b) =>
        b.textContent?.startsWith(text),
      );
      expect(button).toBeTruthy();
      button!.click();
    });
  try {
    await act(async () =>
      root.render(
        <GitTestProviders queryClient={createTestQueryClient()}>
          <TooltipProvider>
            <GitStashControls
              repoId="repo"
              projectName="Fixture"
              snapshot="status"
              busy={false}
              error=""
              onAction={vi.fn()}
            />
          </TooltipProvider>
        </GitTestProviders>,
      ),
    );
    await click("Stashes");
    await click("Saved edits");
    expect(commit.mock.calls).toEqual([["repo", tracked]]);
    expect(document.querySelector(".git-stash-sidebar")).not.toBeNull();
    expect(document.querySelector(".git-stash-sidebar [aria-pressed=true]")).not.toBeNull();
    expect(document.querySelector('[data-testid="preview"]')?.textContent).toBe(
      tracked,
    );
    await click("Untracked files");
    expect(commit.mock.calls).toEqual([
      ["repo", tracked],
      ["repo", untracked],
    ]);
    expect(document.querySelector('[data-testid="preview"]')?.textContent).toBe(
      untracked,
    );
    await click("Tracked changes");
    await click("Untracked files");
    expect(commit).toHaveBeenCalledTimes(2);
    expect(history).not.toHaveBeenCalled();
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});

it("keeps distinct reflog entries for a repeated object and retries a failed continuation", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const entry = (index: number) => ({
    index,
    oid: "a".repeat(40),
    previousOid: "0".repeat(40),
    message: `Entry ${index}`,
    messageTruncated: false,
    time: 0,
  });
  const page = (
    entries: ReturnType<typeof entry>[],
    nextCursor: string | null,
  ) => ({
    snapshot: "same",
    metadata: { listToken: "token" },
    entries,
    nextCursor,
  });
  const stashes = vi
    .fn()
    .mockResolvedValueOnce(page([entry(0)], "next"))
    .mockRejectedValueOnce({ code: "IO_ERROR", message: "Stash read failed" })
    .mockResolvedValueOnce(page([entry(0), entry(1)], null));
  seedGitClient({ stashes } as unknown as GitRepositoryClient);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const click = async (text: string) =>
    act(async () => {
      const button = [...document.querySelectorAll("button")].find(
        (b) => b.textContent === text,
      );
      expect(button).toBeTruthy();
      button!.click();
    });
  try {
    await act(async () =>
      root.render(
        <GitTestProviders queryClient={createTestQueryClient()}>
          <TooltipProvider>
            <GitStashControls
              repoId="repo"
              projectName="Fixture"
              busy={false}
              error=""
              onAction={vi.fn()}
            />
          </TooltipProvider>
        </GitTestProviders>,
      ),
    );
    await click("Stashes");
    await click("Load more stashes");
    expect(document.body.textContent).toContain("Stash read failed");
    expect(document.body.textContent).not.toContain("[object Object]");
    await click("Retry: load more stashes");
    expect(
      document.querySelectorAll(".git-stash-list > li > button"),
    ).toHaveLength(2);
    expect(document.body.textContent).toContain("Entry 0");
    expect(document.body.textContent).toContain("Entry 1");
    expect(document.body.textContent).toContain("All stashes loaded");
    expect(stashes.mock.calls).toEqual([
      ["repo", undefined],
      ["repo", "next"],
      ["repo", "next"],
    ]);
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});
