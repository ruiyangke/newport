// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { GitSyncControl } from "./GitSyncControl";
import { gitPath } from "../domain/git";
import { decodeGitRepository, decodeGitStatus } from "../domain/gitResponses";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";

it("reads only the selected remote on demand and surfaces lookup failures without writing", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const head = {
    name: gitPath("refs/heads/main"),
    oid: { algorithm: "sha1", hex: "a".repeat(40) },
    unborn: false,
    detached: false,
  };
  const repository = decodeGitRepository({
    repoId: "repo",
    commonRepoId: "common",
    root: gitPath("/srv/app"),
    bare: false,
    objectFormat: "sha1",
    head,
    operationState: "Clean",
    integration: null,
    capabilities: { readOnly: false, workingTree: true },
  });
  const status = decodeGitStatus({
    snapshot: "snapshot",
    entries: [],
    nextCursor: null,
    metadata: {
      head,
      operationState: "Clean",
      integration: null,
      ahead: 0,
      behind: 0,
      upstreamRef: gitPath("refs/remotes/origin/main"),
      basis: "stored_refs",
    },
  });
  const remote = vi
    .fn()
    .mockRejectedValue({ code: "REMOTE_NOT_FOUND", message: "Remote missing" });
  const remotes = vi.fn();
  seedGitClient({ remote, remotes });
  const queryClient = createTestQueryClient();
  const onAction = vi.fn().mockResolvedValue(true);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  try {
    await act(async () =>
      root.render(
        <GitTestProviders queryClient={queryClient}>
          <GitSyncControl
            repository={repository}
            status={status}
            busy={false}
            onAction={onAction}
          />
        </GitTestProviders>,
      ),
    );
    expect(remote).not.toHaveBeenCalled();
    await act(async () =>
      host
        .querySelector<HTMLButtonElement>('button[aria-label="Fetch origin"]')!
        .click(),
    );
    expect(remote).toHaveBeenCalledExactlyOnceWith("repo", "origin");
    expect(remotes).not.toHaveBeenCalled();
    expect(onAction).not.toHaveBeenCalled();
    expect(host.querySelector(".git-sync")!.getAttribute("title")).toContain(
      "Remote missing",
    );
    remote.mockResolvedValue({
      name: "origin",
      url: null,
      pushUrl: null,
      token: "current-token",
    });
    await act(async () =>
      host
        .querySelector<HTMLButtonElement>('button[aria-label="Fetch origin"]')!
        .click(),
    );
    expect(onAction).toHaveBeenCalledExactlyOnceWith({
      kind: "fetch",
      remote: "origin",
      expectedToken: "current-token",
    });
    expect(remotes).not.toHaveBeenCalled();
  } finally {
    await act(async () => root.unmount());
    host.remove();
    queryClient.clear();
  }
});
