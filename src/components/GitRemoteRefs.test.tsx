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
