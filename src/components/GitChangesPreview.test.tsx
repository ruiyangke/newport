// @vitest-environment jsdom
import { act, type ComponentProps, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { notifyManager, type QueryClient } from "@tanstack/react-query";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import { EditorView } from "@codemirror/view";
import {
  GitDiffView as GitDiffViewBare,
  GitChangesPreview,
} from "./GitChangesPreview";
import { lineAt } from "./diff/diffExtensions";
import { GitChangesSplit } from "./diff/GitChangesSplit";
import { decodeGitDiff } from "../domain/gitResponses";
import { gitPath } from "../domain/git";
import { GitRepositoryClient } from "../api/gitRepository";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";

// Query delivers results on a timer by default; act() only flushes microtasks.
notifyManager.setScheduler(queueMicrotask);

let root: Root;
let host: HTMLDivElement;
let queryClient: QueryClient;
/** Renders inside a server scope whose Git session is the test's client. */
/*
 * GitDiffView reads the page's Unified | Split choice from the Git store, which
 * is keyed by the enclosing server; in the app it always sits inside a server's
 * page. These tests render it on its own, so they give it the same providers
 * every other converted test uses. Nothing a test asserts is changed by this.
 */
function GitDiffView(props: ComponentProps<typeof GitDiffViewBare>) {
  return (
    <GitTestProviders queryClient={queryClient}>
      <GitDiffViewBare {...props} />
    </GitTestProviders>
  );
}
function inGit(client: GitRepositoryClient, node: ReactNode) {
  seedGitClient(client);
  return <GitTestProviders queryClient={queryClient}>{node}</GitTestProviders>;
}
// The editor is a lazy chunk; load it once so a render settles inside act().
beforeAll(async () => {
  await import("./diff/GitDiffEditor");
});
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
function wire(text: string, lines = 1, binary = false) {
  return {
    snapshot: "s",
    diff: {
      truncated: true,
      readOnly: true,
      files: [
        {
          oldPath: gitPath("old"),
          newPath: gitPath("new"),
          oldOid: null,
          newOid: null,
          oldMode: 33188,
          newMode: 33261,
          status: "Renamed",
          binary,
          additions: lines,
          deletions: 0,
          hunks: [
            {
              oldStart: 0,
              oldLines: 0,
              newStart: 1,
              newLines: lines,
              lines: Array.from({ length: lines }, (_, n) => ({
                origin: "+",
                oldLine: null,
                newLine: n + 1,
                content: gitPath(`${text} ${n + 1}\n`),
              })),
            },
          ],
        },
      ],
    },
  };
}
/** The diff's editor. CodeMirror in jsdom draws only its first screenful, so
    whole-document facts are read from the editor state. */
function editor() {
  const dom = host.querySelector<HTMLElement>(".cm-editor");
  return dom ? EditorView.findFromDOM(dom) : null;
}
it("keeps every line in a bounded DOM, with rename, mode and truncation information", async () => {
  await act(async () =>
    root.render(
      <GitDiffView
        diff={decodeGitDiff(wire("<script>alert(1)</script>", 602))}
      />,
    ),
  );
  // Every one of the agent's lines is in the document, in order...
  const view = editor()!;
  expect(view.state.doc.lines).toBe(602);
  expect(view.state.doc.line(1).text).toBe("<script>alert(1)</script> 1");
  expect(view.state.doc.line(602).text).toBe("<script>alert(1)</script> 602");
  // ...while only the lines on screen are drawn, and never as markup.
  const drawn = host.querySelectorAll(".cm-line").length;
  expect(drawn).toBeGreaterThan(0);
  expect(drawn).toBeLessThan(602);
  expect(host.querySelector("script")).toBeNull();
  expect(host.textContent).toContain("Some changes are not shown");
  expect(host.textContent).toContain("Renamed from old");
  expect(host.textContent).toContain("Mode 100644 → 100755");
});
it("does not render binary bytes as a text patch", async () => {
  await act(async () =>
    root.render(<GitDiffView diff={decodeGitDiff(wire("binary", 1, true))} />),
  );
  expect(host.textContent).toContain("Binary file changed");
  expect(host.querySelector(".cm-editor")).toBeNull();
});
it("ignores a previous selection's late response", async () => {
  const callbacks: ((value: unknown) => void)[] = [];
  const session = {
    request: vi.fn(() => new Promise((resolve) => callbacks.push(resolve))),
    forget: vi.fn(async () => {}),
  };
  const client = new GitRepositoryClient(session);
  const entry = {
    entryId: "one",
    path: gitPath("one"),
    oldPath: null,
    flags: 256,
    staged: false,
    unstaged: true,
    untracked: false,
    conflicted: false,
    conflict: null,
  };
  await act(async () =>
    root.render(
      inGit(
        client,
        <GitChangesPreview
          key="one"
          repoId="repo"
          snapshot="s"
          entry={entry}
        />,
      ),
    ),
  );
  await act(async () =>
    root.render(
      inGit(
        client,
        <GitChangesPreview
          key="two"
          repoId="repo"
          snapshot="s"
          entry={{ ...entry, entryId: "two" }}
        />,
      ),
    ),
  );
  await act(async () => callbacks[1](wire("current")));
  await act(async () => callbacks[0](wire("stale")));
  expect(host.textContent).toContain("current 1");
  expect(host.textContent).not.toContain("stale 1");
});

const changedEntry = {
  entryId: "one",
  path: gitPath("file"),
  oldPath: null,
  flags: 256,
  staged: false,
  unstaged: true,
  untracked: false,
  conflicted: false,
  conflict: null,
};
function editable(lines = 1) {
  const value = wire("change", lines);
  value.diff.truncated = false;
  const file = value.diff.files[0];
  file.oldPath = file.newPath = gitPath("file");
  file.oldMode = file.newMode = 33188;
  file.status = "Modified";
  return {
    ...value,
    diff: {
      ...value.diff,
      files: [
        {
          ...file,
          hunks: file.hunks.map((h) => ({ ...h, id: "a".repeat(64) })),
        },
      ],
    },
  };
}
function button(prefix: string) {
  return [...host.querySelectorAll("button")].find(
    (b) => b.textContent === prefix || b.getAttribute("aria-label") === prefix,
  )!;
}
it.each([false, true])(
  "sends exact snapshot, hunk and comparison context (unstage=%s)",
  async (unstage) => {
    const request = vi.fn(async () => editable());
    const client = new GitRepositoryClient({ request, forget: vi.fn() });
    let resolve!: (value: boolean) => void;
    const onAction = vi.fn(
      () =>
        new Promise<boolean>((r) => {
          resolve = r;
        }),
    );
    await act(async () =>
      root.render(
        inGit(
          client,
          <GitChangesPreview
            repoId="repo"
            snapshot="s"
            entry={{ ...changedEntry, staged: unstage, unstaged: !unstage }}
            onAction={onAction}
          />,
        ),
      ),
    );
    const label = unstage ? "Unstage hunk" : "Stage hunk";
    await act(async () => {
      button(label).click();
      button(label).click();
    });
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onAction).toHaveBeenCalledWith(
      {
        kind: unstage ? "unstage" : "stage",
        entryIds: ["one"],
        hunks: { ids: ["a".repeat(64)], contextLines: 3 },
      },
      "s",
    );
    expect(button(label).disabled).toBe(true);
    expect(request).toHaveBeenCalledWith(
      expect.objectContaining({
        params: expect.objectContaining({
          side: unstage ? "head_to_index" : "index_to_worktree",
          contextLines: 3,
        }),
      }),
    );
    await act(async () => resolve(true));
    expect(button(label)).toBeUndefined();
  },
);
it("removes obsolete hunk actions immediately when the snapshot changes", async () => {
  const request = vi
    .fn()
    .mockResolvedValueOnce(editable())
    .mockImplementation(() => new Promise(() => {}));
  const client = new GitRepositoryClient({ request, forget: vi.fn() });
  const onAction = vi.fn(async () => true);
  const render = (snapshot: string) =>
    inGit(
      client,
      <GitChangesPreview
        repoId="repo"
        snapshot={snapshot}
        entry={changedEntry}
        onAction={onAction}
      />,
    );
  await act(async () => root.render(render("s")));
  expect(button("Stage hunk")).toBeDefined();
  await act(async () => root.render(render("new-snapshot")));
  expect(button("Stage hunk")).toBeUndefined();
  expect(host.textContent).toContain("Loading diff");
  expect(onAction).not.toHaveBeenCalled();
});
it.each(["truncated", "binary", "mode", "rename", "missing-id", "history"])(
  "does not offer hunk actions for %s",
  async (variant) => {
    const value = editable();
    if (variant === "truncated") value.diff.truncated = true;
    if (variant === "binary") value.diff.files[0].binary = true;
    if (variant === "mode") value.diff.files[0].newMode = 33261;
    if (variant === "rename") value.diff.files[0].status = "Renamed";
    if (variant === "missing-id")
      (value.diff.files[0].hunks[0] as { id: string | null }).id = null;
    const onApply = vi.fn();
    await act(async () =>
      root.render(
        <GitDiffView
          diff={decodeGitDiff(value)}
          hunkAction={
            variant === "history"
              ? undefined
              : {
                  label: "Stage hunk",
                  lineLabel: "Stage",
                  disabled: false,
                  onApply,
                  onApplyLines: vi.fn(async () => {}),
                }
          }
        />,
      ),
    );
    expect(button("Stage hunk")).toBeUndefined();
  },
);
it("disables writes for recovery and does not offer them on conflicts", async () => {
  const client = new GitRepositoryClient({
    request: vi.fn(async () => editable()),
    forget: vi.fn(),
  });
  const onAction = vi.fn(async () => true);
  await act(async () =>
    root.render(
      inGit(
        client,
        <GitChangesPreview
          repoId="repo"
          snapshot="s"
          entry={changedEntry}
          blockedReason="Check the saved outcome first."
          onAction={onAction}
        />,
      ),
    ),
  );
  expect(button("Stage hunk").disabled).toBe(true);
  expect(host.textContent).toContain("Check the saved outcome first");
  await act(async () =>
    root.render(
      inGit(
        client,
        <GitChangesPreview
          repoId="repo"
          snapshot="s"
          entry={{ ...changedEntry, conflicted: true }}
          onAction={onAction}
        />,
      ),
    ),
  );
  expect(button("Stage hunk")).toBeUndefined();
});
it("keeps a long hunk actionable and every line tied to it", async () => {
  const onApply = vi.fn(async () => {});
  await act(async () =>
    root.render(
      <GitDiffView
        diff={decodeGitDiff(editable(602))}
        hunkAction={{
          label: "Stage hunk",
          lineLabel: "Stage",
          disabled: false,
          onApply,
          onApplyLines: vi.fn(async () => {}),
        }}
      />,
    ),
  );
  // The whole hunk is one document now, not pages; its last line still
  // belongs to the hunk whose header carries the action.
  const view = editor()!;
  expect(view.state.doc.lines).toBe(602);
  expect(lineAt(view.state, 602)?.hunkId).toBe("a".repeat(64));
  await act(async () => button("Stage hunk").click());
  expect(onApply).toHaveBeenCalledWith("a".repeat(64));
});

it("shows a failed hunk request without replaying it and lets the user reread the diff", async () => {
  const request = vi.fn(async () => editable());
  const client = new GitRepositoryClient({ request, forget: vi.fn() });
  const onAction = vi.fn(async () => {
    throw new Error("Refresh changes: this hunk is stale.");
  });
  await act(async () =>
    root.render(
      inGit(
        client,
        <GitChangesPreview
          repoId="repo"
          snapshot="s"
          entry={changedEntry}
          onAction={onAction}
        />,
      ),
    ),
  );
  await act(async () => button("Stage hunk").click());
  expect(host.querySelector('[role="alert"]')?.textContent).toContain(
    "this hunk is stale",
  );
  expect(onAction).toHaveBeenCalledTimes(1);
  await act(async () => button("Retry diff").click());
  expect(request).toHaveBeenCalledTimes(2);
  expect(onAction).toHaveBeenCalledTimes(1);
});

const lineId = (n: number) => n.toString(16).padStart(64, "0");
/** `editable` plus addressable line identifiers, optionally as a +/- pair. */
function selectable(pair = false) {
  const value = editable(2);
  const file = value.diff.files[0];
  const lines = file.hunks[0].lines.map((line, index) => ({
    ...line,
    id: lineId(index + 1),
    ...(pair && index === 0
      ? { origin: "-", oldLine: 1, newLine: null }
      : null),
  }));
  return {
    ...value,
    diff: {
      ...value.diff,
      files: [{ ...file, hunks: [{ ...file.hunks[0], lines }] }],
    },
  };
}
function dialogButton(label: string) {
  return [...document.querySelectorAll("button")].find(
    (b) => b.textContent === label,
  )!;
}
/** A line's checkbox: its gutter cell in the diff editor. */
function checkbox(index: number) {
  return [
    ...host.querySelectorAll<HTMLElement>('[role="checkbox"][data-diff-line]'),
  ][index];
}

it.each([false, true])(
  "sends the exact selected lines with their hunks (unstage=%s)",
  async (unstage) => {
    const request = vi.fn(async () => selectable());
    const client = new GitRepositoryClient({ request, forget: vi.fn() });
    const onAction = vi.fn(async () => true);
    await act(async () =>
      root.render(
        inGit(
          client,
          <GitChangesPreview
            repoId="repo"
            snapshot="s"
            entry={{ ...changedEntry, staged: unstage, unstaged: !unstage }}
            onAction={onAction}
          />,
        ),
      ),
    );
    await act(async () => checkbox(1).click());
    const verb = unstage ? "Unstage" : "Stage";
    expect(host.textContent).toContain("1 line selected");
    await act(async () => button(`${verb} 1 line`).click());
    expect(onAction).toHaveBeenCalledWith(
      {
        kind: unstage ? "unstage" : "stage",
        entryIds: ["one"],
        hunks: {
          ids: ["a".repeat(64)],
          lines: [lineId(2)],
          contextLines: 3,
        },
      },
      "s",
    );
  },
);

it("omits lines entirely when a whole hunk is staged", async () => {
  const request = vi.fn(async () => selectable());
  const client = new GitRepositoryClient({ request, forget: vi.fn() });
  const onAction = vi.fn(async () => true);
  await act(async () =>
    root.render(
      inGit(
        client,
        <GitChangesPreview
          repoId="repo"
          snapshot="s"
          entry={changedEntry}
          onAction={onAction}
        />,
      ),
    ),
  );
  await act(async () => button("Stage hunk").click());
  expect(onAction).toHaveBeenCalledWith(
    {
      kind: "stage",
      entryIds: ["one"],
      hunks: { ids: ["a".repeat(64)], contextLines: 3 },
    },
    "s",
  );
});

it("warns that unselected deletions survive, and clears the selection", async () => {
  const onApplyLines = vi.fn(async () => {});
  await act(async () =>
    root.render(
      <GitDiffView
        diff={decodeGitDiff(selectable(true))}
        hunkAction={{
          label: "Stage hunk",
          lineLabel: "Stage",
          disabled: false,
          onApply: vi.fn(async () => {}),
          onApplyLines,
        }}
      />,
    ),
  );
  // The addition is selected while its paired deletion is not.
  await act(async () => checkbox(1).click());
  expect(host.textContent).toContain(
    "Deletions you did not select stay in the file",
  );
  await act(async () => checkbox(0).click());
  expect(host.textContent).not.toContain(
    "Deletions you did not select stay in the file",
  );
  await act(async () => button("Clear").click());
  expect(host.textContent).not.toContain("selected");
  expect(onApplyLines).not.toHaveBeenCalled();
});

it.each(["Added", "Untracked", "Deleted"])(
  "offers hunk and line controls for a %s file",
  async (status) => {
    const value = selectable();
    const file = value.diff.files[0];
    file.status = status;
    if (status === "Deleted") file.newMode = 0;
    else file.oldMode = 0;
    await act(async () =>
      root.render(
        <GitDiffView
          diff={decodeGitDiff(value)}
          hunkAction={{
            label: "Stage hunk",
            lineLabel: "Stage",
            disabled: false,
            onApply: vi.fn(async () => {}),
            onApplyLines: vi.fn(async () => {}),
          }}
        />,
      ),
    );
    expect(button("Stage hunk")).toBeDefined();
    expect(checkbox(0)).toBeDefined();
    expect(host.textContent).not.toContain("Individual hunks are unavailable");
  },
);

it("confirms before discarding a hunk and sends the unstaged source", async () => {
  const request = vi.fn(async () => selectable());
  const client = new GitRepositoryClient({ request, forget: vi.fn() });
  const onAction = vi.fn(async () => true);
  await act(async () =>
    root.render(
      inGit(
        client,
        <GitChangesPreview
          repoId="repo"
          snapshot="s"
          entry={changedEntry}
          onAction={onAction}
        />,
      ),
    ),
  );
  await act(async () => button("Discard hunk").click());
  // Nothing is dispatched until the destructive action is confirmed.
  expect(onAction).not.toHaveBeenCalled();
  expect(document.body.textContent).toContain("This cannot be undone");
  await act(async () => dialogButton("Cancel").click());
  expect(onAction).not.toHaveBeenCalled();
  await act(async () => button("Discard hunk").click());
  await act(async () => dialogButton("Discard").click());
  expect(onAction).toHaveBeenCalledWith(
    {
      kind: "discard",
      entryIds: ["one"],
      source: "index",
      hunks: { ids: ["a".repeat(64)], contextLines: 3 },
    },
    "s",
  );
});

it("discards exactly the selected lines", async () => {
  const request = vi.fn(async () => selectable());
  const client = new GitRepositoryClient({ request, forget: vi.fn() });
  const onAction = vi.fn(async () => true);
  await act(async () =>
    root.render(
      inGit(
        client,
        <GitChangesPreview
          repoId="repo"
          snapshot="s"
          entry={changedEntry}
          onAction={onAction}
        />,
      ),
    ),
  );
  await act(async () => checkbox(0).click());
  await act(async () => button("Discard 1 line").click());
  await act(async () => dialogButton("Discard").click());
  expect(onAction).toHaveBeenCalledWith(
    {
      kind: "discard",
      entryIds: ["one"],
      source: "index",
      hunks: {
        ids: ["a".repeat(64)],
        lines: [lineId(1)],
        contextLines: 3,
      },
    },
    "s",
  );
});

const viewportWidth = window.innerWidth;
function setViewportWidth(width: number) {
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    writable: true,
    value: width,
  });
}
afterEach(() => setViewportWidth(viewportWidth));

it("says which comparison the diff shows, next to the selector", async () => {
  const client = new GitRepositoryClient({
    request: vi.fn(async () => editable()),
    forget: vi.fn(),
  });
  const show = (key: string, entry: typeof changedEntry) =>
    root.render(
      inGit(
        client,
        <GitChangesPreview
          key={key}
          repoId="repo"
          snapshot="s"
          entry={entry}
        />,
      ),
    );
  await act(async () =>
    show("staged", { ...changedEntry, staged: true, unstaged: false }),
  );
  expect(host.textContent).toContain(
    "Staged changes: index compared with HEAD",
  );
  // One comparison needs no selector; the sentence names it.
  expect(host.querySelector('[aria-label="Diff comparison"]')).toBeNull();
  await act(async () =>
    show("both", { ...changedEntry, staged: true, unstaged: true }),
  );
  // With both sides the sentence names the one shown, beside the selector
  // that switches it.
  expect(host.textContent).toContain(
    "Staged changes: index compared with HEAD",
  );
  expect(host.querySelector('[aria-label="Diff comparison"]')).not.toBeNull();
  await act(async () => show("unstaged", changedEntry));
  expect(host.textContent).toContain(
    "Unstaged changes: working tree compared with the index",
  );
  await act(async () =>
    show("neither", { ...changedEntry, unstaged: false, untracked: false }),
  );
  expect(host.textContent).toContain(
    "All changes: working tree compared with HEAD",
  );
});

it("offers no discard on the staged comparison", async () => {
  const request = vi.fn(async () => selectable());
  const client = new GitRepositoryClient({ request, forget: vi.fn() });
  await act(async () =>
    root.render(
      inGit(
        client,
        <GitChangesPreview
          repoId="repo"
          snapshot="s"
          entry={{ ...changedEntry, staged: true, unstaged: false }}
          onAction={vi.fn(async () => true)}
        />,
      ),
    ),
  );
  expect(button("Unstage hunk")).toBeDefined();
  // Discarding a staged comparison has no defined meaning here.
  expect(button("Discard hunk")).toBeUndefined();
  await act(async () => checkbox(0).click());
  expect(button("Discard 1 line")).toBeUndefined();
});

it("hands the list width and the narrow flow to a surrounding GitChangesSplit", async () => {
  // The split watches its panels' sizes; jsdom has no ResizeObserver.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  const client = new GitRepositoryClient({
    request: vi.fn(async () => editable()),
    forget: vi.fn(),
  });
  const render = () =>
    root.render(
      inGit(
        client,
        <GitChangesSplit
          list={<p>The file list</p>}
          detailTitle="file"
          detailKey="one"
          detail={
            <GitChangesPreview
              repoId="repo"
              snapshot="s"
              entry={changedEntry}
            />
          }
        />,
      ),
    );
  await act(async () => render());
  // One handle, the shadcn one, and none of the hand-rolled one.
  const handles = host.querySelectorAll('[role="separator"]');
  expect(handles).toHaveLength(1);
  expect(handles[0].getAttribute("aria-label")).toBe("Resize the file list");
  expect(host.querySelector(".git-diff-split")).toBeNull();
  expect(host.textContent).toContain("The file list");
  expect(host.textContent).toContain("change 1");
  // Narrow: one pane at a time, switched from the diff's own toolbar.
  setViewportWidth(480);
  await act(async () => window.dispatchEvent(new Event("resize")));
  expect(host.querySelector('[role="separator"]')).toBeNull();
  const frame = host.querySelector<HTMLElement>("[data-git-changes-pane]")!;
  expect(frame.dataset.gitChangesPane).toBe("diff");
  await act(async () => button("Back to changes").click());
  expect(frame.dataset.gitChangesPane).toBe("list");
  const [list, detail] = [...frame.children] as HTMLElement[];
  expect(list.hidden).toBe(false);
  // The diff stays mounted behind the list, so its choices survive the trip.
  expect(detail.hidden).toBe(true);
  expect(detail.textContent).toContain("change 1");
  await act(async () => button("Show diff").click());
  expect(frame.dataset.gitChangesPane).toBe("diff");
  expect(detail.hidden).toBe(false);
  // Widening restores the split.
  setViewportWidth(1200);
  await act(async () => window.dispatchEvent(new Event("resize")));
  expect(host.querySelectorAll('[role="separator"]')).toHaveLength(1);
  expect(host.textContent).toContain("change 1");
});
