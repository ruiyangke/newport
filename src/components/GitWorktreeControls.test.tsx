// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { notifyManager } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { GitWorktreeControls } from "./GitWorktreeControls";
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
  type GitWorktrees,
} from "../domain/gitResponses";

// Query delivers results on a timer by default; act() only flushes microtasks.
notifyManager.setScheduler(queueMicrotask);

it("ignores a late worktree page after the dialog has been reopened", async () => {
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
    await click("Next worktrees");
    await click("Close dialog");
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
