// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { GitChangeList } from "./GitChangeList";
import { GitCommitComposer } from "./GitCommitComposer";
import { decodeGitStatus } from "../domain/gitResponses";
import { gitPath } from "../domain/git";
import { TooltipProvider } from "./ui/tooltip";

const entry = (id: string, staged = false) => ({
  entryId: id,
  path: gitPath(`${id}.txt`),
  oldPath: null,
  flags: 257,
  staged,
  unstaged: true,
  untracked: false,
  conflicted: false,
  conflict: null,
});
function status(groupCounts?: Record<string, number>) {
  return decodeGitStatus({
    snapshot: "s",
    nextCursor: "next",
    entries: [entry("one", true), entry("two")],
    metadata: {
      head: {
        name: gitPath("refs/heads/main"),
        oid: null,
        unborn: true,
        detached: false,
      },
      operationState: "Clean",
      integration: null,
      ahead: null,
      behind: null,
      upstreamRef: null,
      basis: "stored_refs",
      totalEntries: 12000,
      truncated: false,
      groupCounts,
    },
  });
}

it("distinguishes loaded totals and filters, bulk stages only the named loaded rows", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const stage = vi.fn();
  const more = vi.fn();
  const render = (
    filter = "",
    pageError = "",
    pageLoading = false,
    complete = false,
  ) =>
    act(async () =>
      root.render(
        <TooltipProvider>
          <GitChangeList
            status={{
              ...status(),
              nextCursor: complete ? null : "next",
              metadata: {
                ...status().metadata,
                totalEntries: complete ? 2 : 12000,
              },
            }}
            filter={filter}
            onFilter={vi.fn()}
            group="all"
            onGroup={vi.fn()}
            selectedEntry={null}
            selectedSide="unstaged"
            onSelect={vi.fn()}
            writable
            busy={false}
            onStage={stage}
            onUnstage={vi.fn()}
            onLoadMore={more}
            pageError={pageError}
            pageLoading={pageLoading}
          />
        </TooltipProvider>,
      ),
    );
  try {
    await render();
    expect(host.textContent).toContain("2 of 12,000 changed files loaded");
    const bulk = host.querySelector<HTMLButtonElement>(
      '[aria-label="Stage loaded 2 unstaged"]',
    )!;
    await act(async () => bulk.click());
    expect(stage).toHaveBeenCalledWith(["one", "two"]);
    await render("one");
    expect(host.textContent).toContain("1 matching loaded file"); // one appears in both groups, counted once
    expect(host.textContent).toContain("Filters apply to loaded files only");
    await render("", "Cursor expired");
    expect(host.querySelector('[role="alert"]')?.textContent).toContain(
      "Cursor expired",
    );
    await act(async () =>
      [...host.querySelectorAll("button")]
        .find((button) => button.textContent === "Retry: load more files")!
        .click(),
    );
    expect(more).toHaveBeenCalledOnce();
    await render("", "", true);
    expect(
      [...host.querySelectorAll("button")].find(
        (button) => button.textContent === "Loading…",
      )?.disabled,
    ).toBe(true);
    await render("", "", false, true);
    expect(host.textContent).toContain("All changes loaded");
    expect(host.textContent).toContain("Stage all");
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});

it("uses full index counts and conflicts without guessing from a partial page", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const onAction = vi.fn(async () => true);
  const render = (counts?: Record<string, number>) =>
    act(async () =>
      root.render(
        <GitCommitComposer
          status={status(counts)}
          branch="main"
          summary="A commit"
          description=""
          disabled={false}
          onSummary={vi.fn()}
          onDescription={vi.fn()}
          onAction={onAction}
        />,
      ),
    );
  try {
    await render();
    expect(host.textContent).toContain("Commit staged changes to main");
    expect(host.textContent).toContain("including files not loaded here");
    await render({ staged: 6000, unstaged: 6000, untracked: 0, conflicted: 0 });
    expect(host.textContent).toContain("Commit 6000 files to main");
    await render({ staged: 0, unstaged: 12000, untracked: 0, conflicted: 0 });
    expect(
      host.querySelector<HTMLButtonElement>('button[type="submit"]')?.disabled,
    ).toBe(true);
    await render({ staged: 6000, unstaged: 5999, untracked: 0, conflicted: 1 });
    expect(host.textContent).toContain("Resolve conflicts before committing");
    expect(
      host.querySelector<HTMLButtonElement>('button[type="submit"]')?.disabled,
    ).toBe(true);
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});

it("keeps search mounted while loading, reports server matches and stages only matching rows", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const stage = vi.fn();
  const render = (loading: boolean) =>
    act(async () =>
      root.render(
        <TooltipProvider>
          <GitChangeList
            status={{
              ...status(),
              entries: [entry("one")],
              nextCursor: null,
              metadata: { ...status().metadata, matchedEntries: 1 },
            }}
            filter="one"
            onFilter={() => {}}
            group="all"
            onGroup={() => {}}
            selectedEntry={null}
            selectedSide="unstaged"
            onSelect={() => {}}
            writable
            busy={false}
            onStage={stage}
            onUnstage={() => {}}
            filterLoading={loading}
          />
        </TooltipProvider>,
      ),
    );
  try {
    await render(false);
    const input = host.querySelector("input");
    expect(host.textContent).toContain("1 of 1 matching files loaded.");
    const button = host.querySelector<HTMLButtonElement>(
      'button[aria-label="Stage shown 1 unstaged"]',
    );
    await act(async () => button!.click());
    expect(stage).toHaveBeenCalledWith(["one"]);
    await render(true);
    expect(host.querySelector("input")).toBe(input);
    expect(host.textContent).toContain("Searching changed files");
    expect(host.textContent).not.toContain("No files match");
    expect(host.querySelector('button[aria-label^="Stage"]')).toBeNull();
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});
