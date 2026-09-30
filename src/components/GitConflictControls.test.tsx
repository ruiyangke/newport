// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { notifyManager } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GitConflictControls } from "./GitConflictControls";
import { GitRepositoryClient } from "../api/gitRepository";
import { gitPath } from "../domain/git";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";

// Query delivers results on a timer by default; act() only flushes microtasks.
notifyManager.setScheduler(queueMicrotask);

let root: Root;
let host: HTMLDivElement;
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});
const oid = (character: string) => ({
  algorithm: "sha1" as const,
  hex: character.repeat(40),
});
const side = (character: string) => ({
  oid: oid(character),
  path: gitPath("file"),
  mode: 33188,
});
function entry(conflict: Record<string, unknown> | null = null) {
  return {
    entryId: "one",
    path: gitPath("file"),
    oldPath: null,
    flags: 32768,
    staged: false,
    unstaged: false,
    untracked: false,
    conflicted: true,
    conflict: conflict ?? {
      base: side("a"),
      ours: side("b"),
      theirs: side("c"),
    },
  } as unknown as Parameters<typeof GitConflictControls>[0]["entry"];
}
function button(label: string) {
  return [...document.querySelectorAll("button")].find(
    (b) => b.textContent === label || b.getAttribute("aria-label") === label,
  )!;
}
function render(
  overrides: Partial<Parameters<typeof GitConflictControls>[0]> = {},
) {
  const request = vi.fn(async () => ({
    oid: oid("c"),
    size: 8,
    truncated: false,
    bytesB64: btoa("theirs\n\n"),
  }));
  const client = new GitRepositoryClient({ request, forget: vi.fn() });
  seedGitClient(client);
  const onAction = vi.fn(async () => true);
  return {
    request,
    onAction,
    node: (
      <GitTestProviders queryClient={createTestQueryClient()}>
        <GitConflictControls
          repoId="repo"
          snapshot="s"
          entry={entry()}
          onAction={onAction}
          {...overrides}
        />
      </GitTestProviders>
    ),
  };
}

it("shows all three sides and resolves to the chosen one after confirmation", async () => {
  const { onAction, node } = render();
  await act(async () => root.render(node));
  expect(host.textContent).toContain("Ours");
  expect(host.textContent).toContain("Theirs");
  expect(host.textContent).toContain("Base");
  expect(host.textContent).toContain("the common ancestor");
  await act(async () => button("Use Theirs").click());
  // Destructive resolution is confirmed before anything is dispatched.
  expect(onAction).not.toHaveBeenCalled();
  expect(document.body.textContent).toContain("The other side is discarded");
  await act(async () =>
    [...document.querySelectorAll("button")]
      .find((b) => b.textContent === "Cancel")!
      .click(),
  );
  expect(onAction).not.toHaveBeenCalled();
  await act(async () => button("Use Theirs").click());
  await act(async () =>
    [...document.querySelectorAll("button")]
      .filter((b) => b.textContent === "Use Theirs")
      .at(-1)!
      .click(),
  );
  expect(onAction).toHaveBeenCalledWith(
    {
      kind: "conflict.resolve",
      entryIds: ["one"],
      side: "theirs",
      expectedOid: "c".repeat(40),
    },
    "s",
  );
});

it("reads a side's content on demand and binds it to the requested blob", async () => {
  const { request, node } = render();
  await act(async () => root.render(node));
  await act(async () => button("View Theirs").click());
  expect(request).toHaveBeenCalledWith(
    expect.objectContaining({
      method: "repo.blob",
      params: { repoId: "repo", oid: "c".repeat(40) },
    }),
  );
  expect(host.textContent).toContain("theirs");
  await act(async () => button("Hide").click());
  expect(host.querySelector("pre")).toBeNull();
});

it("offers deletion for a side that removed the file and sends a null blob", async () => {
  const { onAction, node } = render({
    entry: entry({ base: side("a"), ours: side("b"), theirs: null }),
  });
  await act(async () => root.render(node));
  expect(host.textContent).toContain("Absent — this side deleted the file.");
  // An absent side has no content to view.
  expect(button("View Theirs")).toBeUndefined();
  await act(async () => button("Use Theirs").click());
  expect(document.body.textContent).toContain("stages the deletion");
  await act(async () =>
    [...document.querySelectorAll("button")]
      .find((b) => b.textContent === "Delete")!
      .click(),
  );
  expect(onAction).toHaveBeenCalledWith(
    {
      kind: "conflict.resolve",
      entryIds: ["one"],
      side: "theirs",
      expectedOid: null,
    },
    "s",
  );
});

it("disables resolution while writes are blocked", async () => {
  const { onAction, node } = render({ blockedReason: "Check the outcome." });
  await act(async () => root.render(node));
  expect(button("Use Ours").disabled).toBe(true);
  expect(onAction).not.toHaveBeenCalled();
});
