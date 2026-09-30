// @vitest-environment jsdom
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { notifyManager, type QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GitCommitInspector } from "./GitCommitInspector";
import { GitRepositoryClient } from "../api/gitRepository";
import type { GitRequest } from "../domain/git";
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
    snapshot: char.repeat(40),
    nextCursor: null,
    metadata: {
      commitOid: oid(char),
      parents: [],
      parentIndex: null,
      parentOid: null,
      contextLines: 3,
      selectedPath: gitPath(path),
      readOnly: true,
      hasOmissions: false,
      totalUnits: 1,
    },
    entries: [
      {
        ...page(char, path).entries[0],
        fileIndex: 0,
        binary: false,
        omissionReason: null,
        additions: 1,
        deletions: 0,
        hunks: [],
      },
    ],
  };
}
/** Answers each read with what that method returns. */
function answer(char: string, path: string) {
  return async (request: { method: string }) =>
    request.method === "repo.commit_diff_page"
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

it("loads the full message beside the file read and reuses the immutable commit", async () => {
  let resolve!: (value: unknown) => void;
  const full = {
    ...commit("a"),
    message: gitPath("Summary\n\nComplete body from the server"),
  };
  const session = {
    request: vi.fn((request: { method: string }) =>
      request.method === "repo.commit"
        ? new Promise((yes) => {
            resolve = yes;
          })
        : answer("a", "ready.txt")(request),
    ),
    forget: vi.fn(async () => {}),
  };
  const client = new GitRepositoryClient(session);
  const preview = {
    ...commit("a"),
    message: gitPath("Summary"),
    messageTruncated: true,
  };
  const actions = vi.fn((entry: { messageTruncated: boolean }) => (
    <span>
      {entry.messageTruncated ? "Incomplete message" : "Complete message"}
    </span>
  ));
  const node = () =>
    inGit(
      client,
      <GitCommitInspector repoId="repo" commit={preview} actions={actions} />,
    );
  await act(async () => root.render(node()));
  expect(host.textContent).toContain("ready.txt");
  expect(host.textContent).toContain("Loading full commit message");
  expect(actions).toHaveBeenLastCalledWith(
    expect.objectContaining({ messageTruncated: true }),
  );
  await act(async () => resolve(full));
  expect(host.textContent).toContain("Complete body from the server");
  expect(actions).toHaveBeenLastCalledWith(
    expect.objectContaining({ messageTruncated: false }),
  );
  await act(async () => root.render(null));
  await act(async () => root.render(node()));
  expect(
    session.request.mock.calls.filter(
      ([request]) => request.method === "repo.commit",
    ),
  ).toHaveLength(1);
  expect(host.textContent).toContain("Complete message");
});

it("keeps files and the incomplete preview on message failure, with retry", async () => {
  let attempts = 0;
  const session = {
    request: vi.fn(async (request: { method: string }) => {
      if (request.method !== "repo.commit")
        return answer("a", "ready.txt")(request);
      if (++attempts === 1)
        throw { code: "IO_ERROR", message: "Commit read failed" };
      return { ...commit("a"), message: gitPath("Complete recovered message") };
    }),
    forget: vi.fn(async () => {}),
  };
  await act(async () =>
    root.render(
      inGit(
        new GitRepositoryClient(session),
        <GitCommitInspector
          repoId="repo"
          commit={{ ...commit("a"), messageTruncated: true }}
        />,
      ),
    ),
  );
  expect(host.textContent).toContain("ready.txt");
  expect(host.textContent).toContain("Commit read failed (IO_ERROR)");
  await act(async () =>
    [...host.querySelectorAll("button")]
      .find((button) => button.textContent === "Retry commit message")!
      .click(),
  );
  expect(host.textContent).toContain("Complete recovered message");
  expect(host.textContent).not.toContain("Commit read failed");
});

it("prefetches one file page and preserves the selected diff when files append", async () => {
  let finishNext!: (value: unknown) => void;
  const initial = {
    ...page("a", "first.txt"),
    nextCursor: "next",
    metadata: { ...page("a", "first.txt").metadata, totalFiles: 2 },
  };
  const next = { ...page("a", "second.txt"), metadata: initial.metadata };
  const request = vi.fn(
    (request: import("../domain/git").GitRequest): Promise<unknown> => {
      if (request.method === "repo.commit_diff_page")
        return Promise.resolve(
          diffOf("a", request.params.path?.display ?? "first.txt"),
        );
      if (request.method === "repo.commit_files" && request.params.cursor)
        return new Promise((resolve) => {
          finishNext = resolve;
        });
      return Promise.resolve(initial);
    },
  );
  const client = new GitRepositoryClient({
    request,
    forget: vi.fn(async () => {}),
  });
  await act(async () =>
    root.render(
      inGit(client, <GitCommitInspector repoId="repo" commit={commit("a")} />),
    ),
  );
  expect(
    host.querySelector('[aria-label="Historical file diff"]')?.textContent,
  ).toContain("first.txt");
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 180));
  });
  expect(
    request.mock.calls.filter(([r]) => r.method === "repo.commit_files"),
  ).toHaveLength(2);
  await act(async () =>
    [...host.querySelectorAll("button")]
      .find((b) => b.textContent === "Load more files")!
      .click(),
  );
  await act(async () => finishNext(next));
  expect(
    host.querySelector('[aria-label="Historical file diff"]')?.textContent,
  ).toContain("first.txt");
  expect(
    request.mock.calls.filter(([r]) => r.method === "repo.commit_diff_page"),
  ).toHaveLength(1);
  expect(host.textContent).toContain("All commit files loaded");
  expect(
    [...host.querySelectorAll("button")].filter((b) =>
      b.textContent?.includes("first.txt"),
    ),
  ).toHaveLength(1);
  await act(async () =>
    [...host.querySelectorAll("button")]
      .find((b) => b.textContent?.includes("second.txt"))!
      .click(),
  );
  expect(
    host.querySelector('[aria-label="Historical file diff"]')?.textContent,
  ).toContain("second.txt");
  expect(
    request.mock.calls.filter(([r]) => r.method === "repo.commit_diff_page"),
  ).toHaveLength(2);
});

it("appends historical diff pages and reaches the end without restarting the selection", async () => {
  const request = vi.fn(async (request: GitRequest) => {
    if (request.method !== "repo.commit_diff_page")
      return page("a", "file.txt");
    const next = !!request.params.cursor;
    const base = diffOf("a", "file.txt");
    return {
      ...base,
      nextCursor: next ? null : "more-diff",
      metadata: { ...base.metadata, totalUnits: 2 },
      entries: [
        {
          ...base.entries[0],
          hunks: [
            {
              index: 0,
              oldStart: 0,
              oldLines: 0,
              newStart: 1,
              newLines: 2,
              lines: [
                {
                  lineIndex: next ? 1 : 0,
                  byteOffset: 0,
                  lineComplete: true,
                  origin: "+",
                  oldLine: null,
                  newLine: next ? 2 : 1,
                  contentBytesB64: btoa(
                    next ? "second line\n" : "first line\n",
                  ),
                },
              ],
            },
          ],
        },
      ],
    };
  });
  const client = new GitRepositoryClient({
    request,
    forget: vi.fn(async () => {}),
  });
  await act(async () =>
    root.render(
      inGit(client, <GitCommitInspector repoId="repo" commit={commit("a")} />),
    ),
  );
  const more = () =>
    [...host.querySelectorAll("button")].find(
      (button) => button.textContent === "Load more changes",
    );
  // The editor is a lazy import, and its scroll container owns the sentinel.
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 250));
  });
  expect(more()).toBeDefined();
  await act(async () => more()!.click());
  expect(host.textContent).toContain("All changes loaded");
  expect(
    request.mock.calls.filter(([r]) => r.method === "repo.commit_diff_page"),
  ).toHaveLength(2);
  expect(
    request.mock.calls
      .filter(([r]) => r.method === "repo.commit_diff_page")
      .map(([r]) =>
        r.method === "repo.commit_diff_page" ? r.params.maxBytes : null,
      ),
  ).toEqual([65536, 524288]);
  expect(host.textContent).not.toContain("Retry");
});
