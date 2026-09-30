// @vitest-environment jsdom
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { notifyManager, type QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GitCommitInspector } from "./GitCommitInspector";
import { GitRepositoryClient } from "../api/gitRepository";
import { gitPath } from "../domain/git";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";

// Query delivers results on a timer by default; act() only flushes microtasks.
notifyManager.setScheduler(queueMicrotask);
// The file list and diff sit on a resizable split, which watches its panels'
// sizes; jsdom has no ResizeObserver, and measures nothing anyway.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

let host: HTMLDivElement;
let root: Root;
let queryClient: QueryClient;
/** Renders inside a server scope whose Git session is the test's client. */
function inGit(client: GitRepositoryClient, node: ReactNode) {
  seedGitClient(client);
  return <GitTestProviders queryClient={queryClient}>{node}</GitTestProviders>;
}
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  queryClient = createTestQueryClient();
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});
const oid = (char: string) => ({
  algorithm: "sha1" as const,
  hex: char.repeat(40),
});
const commit = (char: string) => ({
  oid: oid(char),
  parents: [],
  message: gitPath("Initial commit"),
  messageTruncated: false,
  author: { name: "Author", email: "a@example.com" },
  time: 0,
  offsetMinutes: 0,
});
function page(char: string, path: string) {
  return {
    snapshot: char,
    nextCursor: null,
    metadata: {
      commitOid: oid(char),
      parentOid: null,
      parentIndex: null,
      parents: [],
      totalFiles: 1,
      truncated: false,
    },
    entries: [
      {
        oldPath: null,
        newPath: gitPath(path),
        oldOid: null,
        newOid: oid(char),
        oldMode: 0,
        newMode: 33188,
        status: "Added",
      },
    ],
  };
}
/** A historical diff of one added file, for the file the inspector opens on. */
function diffOf(char: string, path: string) {
  return {
    // A commit diff is fixed by its commit, so its snapshot is the commit id.
    snapshot: char.repeat(40),
    diff: {
      commitOid: oid(char),
      parents: [],
      parentIndex: null,
      parentOid: null,
      truncated: false,
      readOnly: true,
      files: [
        {
          oldPath: null,
          newPath: gitPath(path),
          oldOid: null,
          newOid: oid(char),
          oldMode: 0,
          newMode: 33188,
          status: "Added",
          binary: false,
          additions: 1,
          deletions: 0,
          hunks: [],
        },
      ],
    },
  };
}
/** Answers each read with what that method returns. */
function answer(char: string, path: string) {
  return async (request: { method: string }) =>
    request.method === "repo.commit_diff"
      ? diffOf(char, path)
      : page(char, path);
}
it("discards late file listings after changing the selected commit", async () => {
  const callbacks: ((value: unknown) => void)[] = [];
  const session = {
    request: vi.fn(() => new Promise((resolve) => callbacks.push(resolve))),
    forget: vi.fn(async () => {}),
  };
  const client = new GitRepositoryClient(session);
  await act(async () =>
    root.render(
      inGit(
        client,
        <GitCommitInspector key="a" repoId="repo" commit={commit("a")} />,
      ),
    ),
  );
  await act(async () =>
    root.render(
      inGit(
        client,
        <GitCommitInspector key="b" repoId="repo" commit={commit("b")} />,
      ),
    ),
  );
  await act(async () => callbacks[1](page("b", "current.txt")));
  await act(async () => callbacks[0](page("a", "stale.txt")));
  expect(host.textContent).toContain("current.txt");
  expect(host.textContent).not.toContain("stale.txt");
  expect(host.textContent).toContain("compared with an empty tree");
});
it("shows file-list failures with recovery instead of an empty commit", async () => {
  const session = {
    request: vi
      .fn()
      .mockRejectedValueOnce({ message: "Snapshot expired" })
      .mockImplementation(answer("a", "recovered.txt")),
    forget: vi.fn(async () => {}),
  };
  const client = new GitRepositoryClient(session);
  await act(async () =>
    root.render(
      inGit(client, <GitCommitInspector repoId="repo" commit={commit("a")} />),
    ),
  );
  expect(host.querySelector('[role="alert"]')?.textContent).toContain(
    "Snapshot expired",
  );
  expect(host.textContent).not.toContain("No changed files");
  const retry = [...host.querySelectorAll("button")].find(
    (button) => button.textContent === "Reload commit files",
  )!;
  await act(async () => retry.click());
  expect(host.textContent).toContain("recovered.txt");
  expect(host.querySelector('[role="alert"]')?.textContent ?? null).toBeNull();
});
it("puts the commit's files and diff on a labelled resizable split", async () => {
  const client = new GitRepositoryClient({
    request: vi.fn(answer("a", "file.txt")),
    forget: vi.fn(async () => {}),
  });
  await act(async () =>
    root.render(
      inGit(client, <GitCommitInspector repoId="repo" commit={commit("a")} />),
    ),
  );
  const handles = host.querySelectorAll('[role="separator"]');
  expect(handles).toHaveLength(1);
  expect(handles[0].getAttribute("aria-label")).toBe(
    "Resize the commit file list",
  );
  expect(handles[0].getAttribute("aria-orientation")).toBe("vertical");
  expect(host.textContent).toContain("file.txt");
  // The pane opens on the first file's diff, not on an instruction to pick one.
  expect(host.textContent).not.toContain(
    "Select a file to inspect this commit.",
  );
  expect(
    host.querySelector('[aria-label="Historical file diff"]')?.textContent,
  ).toContain("file.txt");
});
