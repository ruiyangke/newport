// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { EditorView } from "@codemirror/view";
import { notifyManager } from "@tanstack/react-query";
import { beforeAll, beforeEach, afterEach, it, expect, vi } from "vitest";
import { GitBlobPreview } from "./GitBlobPreview";
import { GitRepositoryClient } from "../api/gitRepository";
import type { GitRequest } from "../domain/git";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";
notifyManager.setScheduler(queueMicrotask);
beforeAll(async () => {
  await import("./GitBlobEditor");
});
let host: HTMLDivElement;
let root: Root;
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
const oid = "a".repeat(40);
function wire(
  raw: string,
  offset: number,
  size: number,
  nextCursor: string | null = null,
  id = oid,
) {
  return {
    snapshot: id,
    nextCursor,
    metadata: { oid: { algorithm: "sha1", hex: id }, size },
    entries: raw ? [{ offset, bytesB64: btoa(raw) }] : [],
  };
}
function button(name: string) {
  return [...host.querySelectorAll("button")].find(
    (b) => b.textContent === name,
  )!;
}
function editor() {
  return EditorView.findFromDOM(
    host.querySelector<HTMLElement>(".cm-editor")!,
  )!;
}
it("virtualizes long content and retries a continuation without replacing loaded text", async () => {
  const first = Array.from({ length: 1200 }, (_, i) => `line ${i}\n`).join("");
  const last = "last line\n";
  let fail = true;
  const request = vi.fn(async (r: GitRequest) => {
    if (r.method !== "repo.blob_page") throw new Error("unexpected method");
    if (!r.params.cursor)
      return wire(first, 0, first.length + last.length, "next");
    if (fail) throw new Error("Read interrupted");
    return wire(last, first.length, first.length + last.length);
  });
  seedGitClient(new GitRepositoryClient({ request, forget: vi.fn() }));
  await act(async () =>
    root.render(
      <GitTestProviders queryClient={createTestQueryClient()}>
        <GitBlobPreview repoId="repo" oid={oid} label="Ours content" />
      </GitTestProviders>,
    ),
  );
  const view = editor();
  expect(view.state.doc.lines).toBe(1201);
  expect(host.querySelectorAll(".cm-line").length).toBeLessThan(1200);
  await act(async () => button("Load more content").click());
  expect(host.textContent).toContain("Read interrupted");
  expect(editor()).toBe(view);
  fail = false;
  await act(async () => button("Retry: load more content").click());
  expect(editor()).toBe(view);
  expect(view.state.doc.toString()).toBe(first + last);
  expect(host.textContent).toContain("All content loaded");
  expect(request.mock.calls.at(-1)![0]).toMatchObject({
    params: { maxBytes: 524288, cursor: "next" },
  });
});
it("does not show a previous conflict side's late content", async () => {
  const pending: ((value: unknown) => void)[] = [];
  seedGitClient(
    new GitRepositoryClient({
      request: vi.fn(() => new Promise((resolve) => pending.push(resolve))),
      forget: vi.fn(),
    }),
  );
  const client = createTestQueryClient();
  const render = (id: string) => (
    <GitTestProviders queryClient={client}>
      <GitBlobPreview repoId="repo" oid={id} label="Side content" />
    </GitTestProviders>
  );
  await act(async () => root.render(render(oid)));
  await act(async () => root.render(render("b".repeat(40))));
  await act(async () =>
    pending[1](wire("current", 0, 7, null, "b".repeat(40))),
  );
  await act(async () => pending[0](wire("obsolete", 0, 8)));
  expect(editor().state.doc.toString()).toBe("current");
  expect(host.textContent).not.toContain("obsolete");
});
it("stops loading binary content", async () => {
  const request = vi.fn(async () => wire("\0binary", 0, 100, "next"));
  seedGitClient(new GitRepositoryClient({ request, forget: vi.fn() }));
  await act(async () =>
    root.render(
      <GitTestProviders queryClient={createTestQueryClient()}>
        <GitBlobPreview repoId="repo" oid={oid} label="Side content" />
      </GitTestProviders>,
    ),
  );
  expect(host.textContent).toContain("binary data");
  expect(button("Load more content")).toBeUndefined();
  expect(host.querySelector(".cm-editor")).toBeNull();
  expect(request).toHaveBeenCalledOnce();
});
