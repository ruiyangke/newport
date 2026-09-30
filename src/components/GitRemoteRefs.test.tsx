// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { notifyManager } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { GitRemoteRefs } from "./GitRemoteRefs";
import { TooltipProvider } from "./ui/tooltip";
import type { GitRepositoryClient } from "../api/gitRepository";
import { decodeGitRemoteRefs } from "../domain/gitResponses";
import { gitPath } from "../domain/git";
import { invalidateRepository } from "../query/git";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";

// Query delivers results on a timer by default; act() only flushes microtasks.
notifyManager.setScheduler(queueMicrotask);

it.each([
  "branch",
  "tag",
  "stale",
  "blocked",
  "push",
  "push_stale",
  "push_blocked",
  "push_head",
  "push_branch",
  "push_detached",
])("remote mutation %s keeps exact guards", async (mode) => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const kind = mode === "tag" ? "tag" : "branch";
  const pushing = mode.startsWith("push");
  const confirm = pushing ? "Replace remote branch" : `Delete remote ${kind}`;
  const page = decodeGitRemoteRefs({
    snapshot: "remote",
    nextCursor: null,
    metadata: {
      remote: "origin",
      remoteToken: "config",
      forPush: true,
      basis: "remote_advertisement",
      truncated: false,
    },
    entries: [
      {
        reference: gitPath(kind === "tag" ? "refs/tags/v1" : "refs/heads/old"),
        kind,
        oid: { algorithm: "sha1", hex: "e".repeat(40) },
        symbolicTarget: null,
      },
    ],
  });
  const remoteRefs = vi.fn().mockResolvedValue(page);
  const client = { remoteRefs } as unknown as GitRepositoryClient;
  seedGitClient(client);
  const queryClient = createTestQueryClient();
  const onAction = vi.fn().mockResolvedValue(true);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const render = (changed = false) =>
    act(async () =>
      root.render(
        <GitTestProviders queryClient={queryClient}>
          <TooltipProvider>
            <GitRemoteRefs
              repoId="repo"
              remote={{
                name: "origin",
                token: "config",
                url: "git@example.test:repo",
                pushUrl: "git@example.test:push",
              }}
              snapshot={changed && mode.endsWith("stale") ? "new" : "original"}
              disabled={changed && mode.endsWith("blocked")}
              busy={false}
              source={
                changed && mode === "push_detached"
                  ? undefined
                  : {
                      name:
                        changed && mode === "push_branch" ? "other" : "main",
                      oid: (changed && mode === "push_head" ? "b" : "a").repeat(
                        40,
                      ),
                    }
              }
              onAction={onAction}
              onBack={vi.fn()}
            />
          </TooltipProvider>
        </GitTestProviders>,
      ),
    );
  const button = (name: string) =>
    [...document.querySelectorAll("button")].find(
      (b) => b.textContent === name,
    )!;
  try {
    await render();
    expect(remoteRefs).toHaveBeenCalledWith({
      repoId: "repo",
      remote: "origin",
      expectedToken: "config",
      forPush: true,
    });
    await act(async () =>
      button(pushing ? "Push with lease…" : "Delete…").click(),
    );
    if (mode !== "branch" && mode !== "tag" && mode !== "push") {
      await render(true);
      expect(button(confirm).disabled).toBe(true);
      expect(button("Back to remote references").disabled).toBe(false);
      await act(async () => button(confirm).click());
      expect(onAction).not.toHaveBeenCalled();
    } else {
      await act(async () => button(confirm).click());
      if (pushing) {
        expect(onAction).toHaveBeenCalledExactlyOnceWith({
          kind: "push.with_lease",
          remote: "origin",
          expectedToken: "config",
          branch: "main",
          expectedOid: "a".repeat(40),
          destinationBranch: "old",
          expectedRemoteOid: "e".repeat(40),
        });
        return;
      }
      expect(onAction).toHaveBeenCalledExactlyOnceWith({
        kind: kind === "tag" ? "tag.delete_remote" : "branch.delete_remote",
        remote: "origin",
        expectedToken: "config",
        expectedOid: "e".repeat(40),
        ...(kind === "tag" ? { name: "v1" } : { branch: "old" }),
      });
    }
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});

it("never contacts the remote again because the repository's reads were retired", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const remoteRefs = vi.fn().mockResolvedValue(
    decodeGitRemoteRefs({
      snapshot: "remote",
      nextCursor: null,
      metadata: {
        remote: "origin",
        remoteToken: "config",
        forPush: true,
        basis: "remote_advertisement",
        truncated: false,
      },
      entries: [
        {
          reference: gitPath("refs/heads/old"),
          kind: "branch",
          oid: { algorithm: "sha1", hex: "e".repeat(40) },
          symbolicTarget: null,
        },
      ],
    }),
  );
  const client = { remoteRefs } as unknown as GitRepositoryClient;
  const scope = seedGitClient(client);
  const queryClient = createTestQueryClient();
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const render = (snapshot: string) =>
    act(async () =>
      root.render(
        <GitTestProviders queryClient={queryClient}>
          <TooltipProvider>
            <GitRemoteRefs
              repoId="repo"
              remote={{
                name: "origin",
                token: "config",
                url: "git@example.test:repo",
                pushUrl: null,
              }}
              snapshot={snapshot}
              disabled={false}
              busy={false}
              onAction={vi.fn()}
              onBack={vi.fn()}
            />
          </TooltipProvider>
        </GitTestProviders>,
      ),
    );
  try {
    await render("original");
    expect(remoteRefs).toHaveBeenCalledTimes(1);
    expect(document.body.textContent).toContain("refs/heads/old");
    // A write elsewhere retires every read of the repository, and the page
    // re-reads status. Neither is a request to contact the remote.
    await act(async () => {
      await invalidateRepository(queryClient, scope, "repo");
    });
    await render("changed");
    expect(remoteRefs).toHaveBeenCalledTimes(1);
    expect(document.body.textContent).toContain("refs/heads/old");
    // Asking is what contacts it.
    await act(async () =>
      [...document.querySelectorAll("button")]
        .find((b) => b.textContent === "Refresh remote references")!
        .click(),
    );
    expect(remoteRefs).toHaveBeenCalledTimes(2);
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});

it("appends references while writes are blocked, retries page errors, and discards an old page after refresh", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const page = (
    names: string[],
    nextCursor: string | null,
    snapshot = "listing",
  ) =>
    decodeGitRemoteRefs({
      snapshot,
      nextCursor,
      metadata: {
        remote: "origin",
        remoteToken: "config",
        forPush: true,
        basis: "remote_advertisement",
        truncated: false,
      },
      entries: names.map((name) => ({
        reference: gitPath(`refs/heads/${name}`),
        kind: "branch",
        oid: { algorithm: "sha1", hex: "a".repeat(40) },
        symbolicTarget: null,
      })),
    });
  let resolveOld: ((value: ReturnType<typeof page>) => void) | undefined;
  const remoteRefs = vi
    .fn()
    .mockResolvedValueOnce(page(["first"], "second"))
    .mockRejectedValueOnce({
      code: "CONNECTION_FAILED",
      message: "Connection interrupted",
    })
    .mockResolvedValueOnce(page(["first", "second"], "third"))
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveOld = resolve;
        }),
    )
    .mockResolvedValueOnce(page(["fresh"], null, "fresh-listing"));
  seedGitClient({ remoteRefs } as unknown as GitRepositoryClient);
  const queryClient = createTestQueryClient();
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const button = (name: string) =>
    [...host.querySelectorAll("button")].find((b) => b.textContent === name)!;
  try {
    await act(async () =>
      root.render(
        <GitTestProviders queryClient={queryClient}>
          <TooltipProvider>
            <GitRemoteRefs
              repoId="repo"
              remote={{
                name: "origin",
                token: "config",
                url: "git@example.test:repo",
                pushUrl: null,
              }}
              disabled
              busy={false}
              snapshot="status"
              onAction={vi.fn()}
              onBack={vi.fn()}
            />
          </TooltipProvider>
        </GitTestProviders>,
      ),
    );
    expect(button("Delete…").disabled).toBe(true);
    expect(button("Load more references").disabled).toBe(false);
    await act(async () => button("Load more references").click());
    expect(host.textContent).toContain("Connection interrupted");
    expect(host.textContent).not.toContain("[object Object]");
    await act(async () => button("Retry: load more references").click());
    expect(
      [...host.querySelectorAll("strong")]
        .map((n) => n.textContent)
        .filter((n) => n?.startsWith("refs/")),
    ).toEqual(["refs/heads/first", "refs/heads/second"]);
    await act(async () => button("Load more references").click());
    expect(button("Refresh remote references").disabled).toBe(false);
    await act(async () => button("Refresh remote references").click());
    await act(async () => resolveOld!(page(["obsolete"], null)));
    expect(host.textContent).toContain("refs/heads/fresh");
    expect(host.textContent).not.toContain("refs/heads/obsolete");
    expect(host.textContent).toContain("All remote references loaded");
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});

it("searches beyond loaded references and ignores a superseded search", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const page = (name: string) =>
    decodeGitRemoteRefs({
      snapshot: `remote-${name}`,
      nextCursor: null,
      metadata: {
        remote: "origin",
        remoteToken: "config",
        forPush: true,
        basis: "remote_advertisement",
        truncated: false,
      },
      entries: name
        ? [
            {
              reference: gitPath(`refs/heads/${name}`),
              kind: "branch",
              oid: { algorithm: "sha1", hex: "e".repeat(40) },
              symbolicTarget: null,
            },
          ]
        : [],
    });
  let resolveOld!: (value: ReturnType<typeof page>) => void;
  const old = new Promise<ReturnType<typeof page>>((resolve) => {
    resolveOld = resolve;
  });
  const remoteRefs = vi.fn(({ filter }: { filter?: string }) => {
    if (filter === "old") return old;
    if (filter === "failure")
      return Promise.reject(new Error("Network unavailable"));
    return Promise.resolve(
      page(filter === "missing" ? "" : (filter ?? "initial")),
    );
  });
  seedGitClient({ remoteRefs } as unknown as GitRepositoryClient);
  const queryClient = createTestQueryClient();
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const inputValue = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )!.set!;
  async function search(value: string) {
    await act(async () => {
      const input = host.querySelector("input")!;
      inputValue.call(input, value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(host.querySelector("li")).toBeNull();
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 220));
    });
  }
  try {
    await act(async () =>
      root.render(
        <GitTestProviders queryClient={queryClient}>
          <TooltipProvider>
            <GitRemoteRefs
              repoId="repo"
              remote={{
                name: "origin",
                token: "config",
                url: null,
                pushUrl: null,
              }}
              snapshot="original"
              disabled={false}
              busy={false}
              onAction={vi.fn()}
              onBack={vi.fn()}
            />
          </TooltipProvider>
        </GitTestProviders>,
      ),
    );
    await search("old");
    await search(" LATE ");
    expect(remoteRefs).toHaveBeenLastCalledWith(
      expect.objectContaining({ filter: "late" }),
    );
    expect(host.textContent).toContain("refs/heads/late");
    await act(async () => resolveOld(page("old")));
    expect(host.textContent).not.toContain("refs/heads/old");
    await search("missing");
    expect(host.textContent).toContain("No matching remote references.");
    expect(host.textContent).toContain("All matching references loaded");
    await search("failure");
    expect(host.querySelector('[role="alert"]')?.textContent).toContain(
      "Network unavailable",
    );
    expect(host.querySelector("input")?.value).toBe("failure");
    await search("");
    expect(host.textContent).toContain("refs/heads/initial");
  } finally {
    await act(async () => root.unmount());
    queryClient.clear();
    host.remove();
  }
});
