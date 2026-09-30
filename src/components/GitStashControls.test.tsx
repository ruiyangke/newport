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

it("ignores a late stash page after the dialog has been reopened", async () => {
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
    await click("Close dialog");
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
