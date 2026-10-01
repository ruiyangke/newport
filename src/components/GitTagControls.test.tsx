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

it("ignores a late tag page after the inspector has been reopened", async () => {
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
    await click("Load more tags");
    await click("Close inspector");
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

const tagRow = (name: string, truncated = false) => ({
  name: { display: name, bytesB64: btoa(name) },
  reference: {
    display: `refs/tags/${name}`,
    bytesB64: btoa(`refs/tags/${name}`),
  },
  oid: {
    algorithm: "sha1",
    hex: name === "v1" ? "a".repeat(40) : "b".repeat(40),
  },
  symbolicTarget: null,
  annotated: true,
  detailsOmitted: false,
  message: { display: "Preview", bytesB64: btoa("Preview") },
  messageTruncated: truncated,
});
const tagPage = (
  entries: ReturnType<typeof tagRow>[],
  nextCursor: string | null,
) => decodeGitTags({ snapshot: "tags", metadata: {}, entries, nextCursor });
async function mountTags(client: Partial<GitRepositoryClient>) {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  seedGitClient(client);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const queryClient = createTestQueryClient();
  const repository = decodeGitRepository({
    repoId: "repo",
    commonRepoId: "common",
    root: { display: "/repo", bytesB64: btoa("/repo") },
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
        <GitTagControls
          repository={repository}
          open
          snapshot="status"
          busy={false}
          error=""
          onAction={vi.fn(async () => true)}
        />
      </GitTestProviders>,
    ),
  );
  return {
    click: (text: string) =>
      act(async () =>
        [...document.querySelectorAll("button")]
          .find(
            (button) =>
              button.textContent === text ||
              button.querySelector("strong")?.textContent === text,
          )!
          .click(),
      ),
    close: async () => {
      await act(async () => root.unmount());
      queryClient.clear();
      host.remove();
    },
  };
}

it("waits for an explicit load before reading the next tag page", async () => {
  const tags = vi.fn(async (_repo, cursor) =>
    cursor
      ? tagPage([tagRow("v1"), tagRow("v2")], null)
      : tagPage([tagRow("v1")], "next"),
  );
  const ui = await mountTags({ tags });
  try {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 170));
    });
    expect(tags).toHaveBeenCalledTimes(1);
    expect(document.body.textContent).not.toContain("v2");
    await ui.click("Load more tags");
    expect(tags).toHaveBeenCalledTimes(2);
    expect(
      [...document.querySelectorAll(".git-tag-list strong")].map(
        (node) => node.textContent,
      ),
    ).toEqual(["v1", "v2"]);
    expect(document.body.textContent).toContain("All tags loaded");
  } finally {
    await ui.close();
  }
});

it("loads selected annotations without blocking tag actions and caches by object", async () => {
  let resolve!: (
    value: Awaited<ReturnType<GitRepositoryClient["tag"]>>,
  ) => void;
  const tag = vi.fn(
    () =>
      new Promise<Awaited<ReturnType<GitRepositoryClient["tag"]>>>((yes) => {
        resolve = yes;
      }),
  );
  const entries = tagPage([tagRow("v1", true), tagRow("v2")], null);
  const ui = await mountTags({ tags: vi.fn(async () => entries), tag });
  try {
    await ui.click("v1");
    expect(document.body.textContent).toContain("Loading full annotation");
    expect(
      [...document.querySelectorAll("button")].find(
        (button) => button.textContent === "Delete local tag…",
      )?.disabled,
    ).toBe(false);
    await act(async () =>
      resolve({
        ...entries.entries[0],
        oid: { algorithm: "sha1", hex: "a".repeat(40) },
        message: {
          display: "Full annotation",
          bytesB64: btoa("Full annotation"),
        },
        messageTruncated: false,
      }),
    );
    expect(document.body.textContent).toContain("Full annotation");
    await ui.click("v2");
    await ui.click("v1");
    expect(tag).toHaveBeenCalledOnce();
  } finally {
    await ui.close();
  }
});

it("shows annotation and continuation errors without discarding loaded rows", async () => {
  const tag = vi
    .fn()
    .mockRejectedValueOnce({
      code: "IO_ERROR",
      message: "Annotation unavailable",
    })
    .mockResolvedValue({
      ...tagPage([tagRow("v1")], null).entries[0],
      messageTruncated: false,
    });
  const tags = vi.fn(async (_repo, cursor) => {
    if (cursor) throw { code: "STALE_SNAPSHOT", message: "Tag list changed" };
    return tagPage([tagRow("v1", true)], "next");
  });
  const ui = await mountTags({ tags, tag });
  try {
    await ui.click("v1");
    expect(document.body.textContent).toContain(
      "Annotation unavailable (IO_ERROR)",
    );
    await ui.click("Retry annotation");
    expect(document.body.textContent).not.toContain("Annotation unavailable");
    await ui.click("Load more tags");
    expect(document.body.textContent).toContain(
      "Tag list changed (STALE_SNAPSHOT)",
    );
    expect(document.querySelector(".git-tag-list strong")?.textContent).toBe(
      "v1",
    );
    expect(document.body.textContent).not.toContain("[object Object]");
  } finally {
    await ui.close();
  }
});
