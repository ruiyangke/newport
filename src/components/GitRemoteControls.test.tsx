// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { notifyManager } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { GitRemoteControls } from "./GitRemoteControls";
import { decodeGitRepository } from "../domain/gitResponses";
import { gitPath } from "../domain/git";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= () => {};

const repository = decodeGitRepository({
  repoId: "repo",
  commonRepoId: "common",
  root: gitPath("/repo"),
  bare: false,
  objectFormat: "sha1",
  head: {
    name: gitPath("refs/heads/main"),
    oid: { algorithm: "sha1", hex: "a".repeat(40) },
    detached: false,
    unborn: false,
  },
  operationState: "Clean",
  integration: null,
  capabilities: { readOnly: false, workingTree: true },
});

function client(configuration: {
  fetchPrune: boolean;
  userName: string | null;
  userEmail: string | null;
  token: string;
}) {
  return {
    remote: vi.fn(async (_repo: string, name: string) => ({
      name,
      url: `https://example.test/${name}`,
      pushUrl: null,
      token: `token-${name}`,
    })),
    remoteNames: vi.fn(async () => ({
      snapshot: "names",
      entries: [{ name: "origin" }],
      nextCursor: null,
      metadata: { totalEntries: 1 },
    })),
    remotes: vi.fn(async () => ({
      entries: [
        {
          name: "origin",
          url: "https://example.test/app",
          pushUrl: null,
          token: "token-origin",
        },
      ],
      authentication: {
        ssh: "server_agent",
        https: "server_helpers",
      } as const,
      configuration,
    })),
  };
}

async function mount(
  queryClient: ReturnType<typeof createTestQueryClient>,
  onAction: (action: unknown) => Promise<boolean>,
) {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  notifyManager.setScheduler(queueMicrotask);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () =>
    root.render(
      <GitTestProviders queryClient={queryClient}>
        <GitRemoteControls
          repository={repository}
          projectName="App"
          busy={false}
          recoveryAvailable={false}
          open
          onOpenChange={() => {}}
          hideTrigger
          error=""
          onAction={onAction as never}
        />
      </GitTestProviders>,
    ),
  );
  // Let the remote, names, and remotes reads resolve.
  await act(async () => {});
  await act(async () => {});
  return {
    done: async () => {
      await act(async () => root.unmount());
      host.remove();
      queryClient.clear();
    },
  };
}

const heading = (name: string) =>
  expect(
    document.querySelector(`section[aria-label="${name}"]`),
  ).not.toBeNull();

it("shows repository settings with server values", async () => {
  seedGitClient(
    client({
      fetchPrune: false,
      userName: "Ada",
      userEmail: "ada@example.test",
      token: "config-token",
    }),
  );
  const queryClient = createTestQueryClient();
  const onAction = vi.fn(async () => true);
  const ui = await mount(queryClient, onAction);
  try {
    heading("Remotes");
    heading("Fetching");
    heading("Commit identity");
    expect(document.body.textContent).toContain("Prune deleted branches");
    const name = document.querySelector(
      'section[aria-label="Commit identity"] input[placeholder="Your name"]',
    ) as HTMLInputElement;
    const email = document.querySelector(
      'section[aria-label="Commit identity"] input[placeholder="you@example.com"]',
    ) as HTMLInputElement;
    expect(name.value).toBe("Ada");
    expect(email.value).toBe("ada@example.test");
    // Unchanged drafts cannot save.
    const save = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Save identity",
    )!;
    expect(save.disabled).toBe(true);
  } finally {
    await ui.done();
  }
});

it("saves the prune default without closing, then the edited identity", async () => {
  seedGitClient(
    client({
      fetchPrune: false,
      userName: "Ada",
      userEmail: "ada@example.test",
      token: "config-token",
    }),
  );
  const queryClient = createTestQueryClient();
  const onAction = vi.fn(async () => true);
  const ui = await mount(queryClient, onAction);
  try {
    const toggle = document.querySelector(
      'section[aria-label="Fetching"] button[role="checkbox"]',
    ) as HTMLElement;
    await act(async () => toggle.click());
    expect(onAction).toHaveBeenCalledWith({
      kind: "config.set",
      expectedToken: "config-token",
      fetchPrune: true,
    });
    // A settings save keeps the page open, unlike the editor dialogs.
    heading("Fetching");
    const name = document.querySelector(
      'section[aria-label="Commit identity"] input[placeholder="Your name"]',
    ) as HTMLInputElement;
    const inputValue = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!;
    await act(async () => {
      inputValue.call(name, "Grace");
      name.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(name.value).toBe("Grace");
    const save = [...document.querySelectorAll("button")].find(
      (button) => button.textContent === "Save identity",
    )!;
    expect(save.disabled).toBe(false);
    await act(async () => save.click());
    expect(onAction).toHaveBeenCalledWith({
      kind: "config.set",
      expectedToken: "config-token",
      userName: "Grace",
      userEmail: "ada@example.test",
    });
    heading("Commit identity");
  } finally {
    await ui.done();
  }
});
