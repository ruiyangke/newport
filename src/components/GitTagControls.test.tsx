// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { notifyManager } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { GitTagControls } from "./GitTagControls";
import { TooltipProvider } from "./ui/tooltip";
import type { GitRepositoryClient } from "../api/gitRepository";
import {
  decodeGitRepository,
  decodeGitTags,
  type GitTags,
} from "../domain/gitResponses";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";

// Query delivers results on a timer by default; act() only flushes microtasks.
notifyManager.setScheduler(queueMicrotask);

it("ignores a late tag page after the dialog has been reopened", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const page = (
    message: string,
    snapshot: string,
    nextCursor: string | null,
  ): GitTags => ({
    snapshot,
    nextCursor,
    metadata: {},
    entries: decodeGitTags({
      snapshot,
      nextCursor: null,
      metadata: {},
      entries: [
        {
          name: { display: message, bytesB64: btoa(message) },
          reference: {
            display: `refs/tags/${message}`,
            bytesB64: btoa(`refs/tags/${message}`),
          },
          oid: { algorithm: "sha1", hex: "a".repeat(40) },
          symbolicTarget: null,
          annotated: false,
          detailsOmitted: false,
        },
      ],
    }).entries,
  });
  let resolvePage!: (value: GitTags) => void;
  const tags = vi
    .fn()
    .mockResolvedValueOnce(page("Original tag", "old", "next"))
    .mockImplementationOnce(
      () =>
        new Promise<GitTags>((resolve) => {
          resolvePage = resolve;
        }),
    )
    .mockResolvedValueOnce(page("Fresh tag", "new", null));
  const client = { tags } as unknown as GitRepositoryClient;
  seedGitClient(client);
  const queryClient = createTestQueryClient();
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
        <GitTestProviders queryClient={queryClient}>
          <TooltipProvider>
            <GitTagControls
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
    await click("Tags");
    await click("Next tags");
    await click("Close dialog");
    await click("Tags");
    expect(document.body.textContent).toContain("Fresh tag");
    await act(async () => resolvePage(page("Late stale tag", "old", null)));
    expect(document.body.textContent).toContain("Fresh tag");
    expect(document.body.textContent).not.toContain("Late stale tag");
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});
