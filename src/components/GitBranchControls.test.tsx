// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { notifyManager } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { toast } from "sonner";
import { GitBranchControls } from "./GitBranchControls";
import { TooltipProvider } from "./ui/tooltip";
import type { GitRepositoryClient } from "../api/gitRepository";
import { decodeGitBranches, decodeGitRepository } from "../domain/gitResponses";
import { gitPath } from "../domain/git";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";

// Query delivers results on a timer by default; act() only flushes microtasks.
notifyManager.setScheduler(queueMicrotask);
// cmdk measures its list and scrolls the highlighted row into view; jsdom has
// neither API, and neither is what these tests are about.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= () => {};
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
/** The branch row whose name is `name`. */
const row = (name: string) =>
  [...document.querySelectorAll<HTMLElement>('[role="option"]')].find(
    (row) => row.querySelector("strong")?.textContent === name,
  )!;
/** Right-clicks a branch row, as the pointer asks for its context menu. */
const openRowMenu = (name: string) =>
  act(async () => {
    row(name).dispatchEvent(
      new MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        button: 2,
      }),
    );
  });

it.each(["save", "stale", "inherited"])(
  "upstream %s respects configuration and paging guards",
  async (mode) => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
    const oid = { algorithm: "sha1", hex: "a".repeat(40) };
    const branch = {
      name: gitPath("main"),
      reference: gitPath("refs/heads/main"),
      oid,
      current: true,
      remote: false,
      upstream: gitPath("refs/remotes/origin/main"),
      tracking: {
        token: "tracking-original",
        editable: mode !== "inherited",
        configuration: { remote: [], merge: [] },
      },
    };
    const branches = vi
      .fn()
      .mockResolvedValueOnce(
        decodeGitBranches({
          snapshot: "branches",
          nextCursor: "next",
          metadata: {},
          entries: [branch],
        }),
      )
      .mockResolvedValue(
        decodeGitBranches({
          snapshot: "branches",
          nextCursor: null,
          metadata: {},
          entries: [
            {
              ...branch,
              name: gitPath("origin/main"),
              reference: gitPath("refs/remotes/origin/main"),
              remote: true,
              current: false,
              upstream: null,
              tracking: null,
            },
          ],
        }),
      );
    const repository = decodeGitRepository({
      repoId: "repo",
      commonRepoId: "common",
      root: gitPath("/repo"),
      bare: false,
      objectFormat: "sha1",
      head: {
        name: gitPath("refs/heads/main"),
        oid,
        detached: false,
        unborn: false,
      },
      operationState: "Clean",
      integration: null,
      capabilities: { readOnly: false, workingTree: true },
    });
    const client = { branches } as unknown as GitRepositoryClient;
    seedGitClient(client);
    const queryClient = createTestQueryClient();
    const onAction = vi.fn().mockResolvedValue(true);
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    const render = (snapshot: string) =>
      act(async () =>
        root.render(
          <GitTestProviders queryClient={queryClient}>
            <TooltipProvider>
              <GitBranchControls
                repository={repository}
                snapshot={snapshot}
                busy={false}
                writable
                error=""
                onAction={onAction}
              />
            </TooltipProvider>
          </GitTestProviders>,
        ),
      );
    const button = (name: string) =>
      [...document.querySelectorAll("button")].find(
        (b) => b.textContent === name || b.getAttribute("aria-label") === name,
      )!;
    try {
      await render("status");
      await act(async () => button("Branches: main").click());
      await openRowMenu("main");
      const item = [
        ...document.querySelectorAll<HTMLElement>('[role="menuitem"]'),
      ].find(
        (item) =>
          item.textContent?.includes("upstream") ||
          item.textContent?.startsWith("Upstream"),
      )!;
      if (mode === "inherited") {
        expect(item.getAttribute("aria-disabled")).toBe("true");
        return;
      }
      await act(async () => item.click());
      expect(button("Save upstream").disabled).toBe(true);
      await act(async () => button("Load more upstream branches").click());
      expect(branches).toHaveBeenLastCalledWith("repo", "next");
      expect(button("Save upstream").disabled).toBe(false);
      if (mode === "stale") {
        await render("changed");
        expect(button("Save upstream").disabled).toBe(true);
        expect(document.body.textContent).toContain("The repository changed");
        expect(button("Back").disabled).toBe(false);
        expect(onAction).not.toHaveBeenCalled();
      } else {
        await act(async () => button("Save upstream").click());
        expect(onAction).toHaveBeenCalledExactlyOnceWith({
          kind: "branch.set_upstream",
          name: "main",
          expectedOid: "a".repeat(40),
          expectedToken: "tracking-original",
          upstream: "refs/remotes/origin/main",
        });
      }
    } finally {
      await act(async () => root.unmount());
      host.remove();
    }
  },
);

it.each([false, true])(
  "deletes a branch with force=%s only when explicitly escalated",
  async (forced) => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
    const oid = { algorithm: "sha1", hex: "a".repeat(40) };
    const branches = vi.fn().mockResolvedValue(
      decodeGitBranches({
        snapshot: "branches",
        nextCursor: null,
        metadata: {},
        entries: [
          {
            name: gitPath("main"),
            reference: gitPath("refs/heads/main"),
            oid,
            current: true,
            remote: false,
            upstream: null,
            tracking: null,
          },
          {
            name: gitPath("topic"),
            reference: gitPath("refs/heads/topic"),
            oid: { algorithm: "sha1", hex: "b".repeat(40) },
            current: false,
            remote: false,
            upstream: null,
            tracking: null,
          },
        ],
      }),
    );
    const repository = decodeGitRepository({
      repoId: "repo",
      commonRepoId: "common",
      root: gitPath("/repo"),
      bare: false,
      objectFormat: "sha1",
      head: {
        name: gitPath("refs/heads/main"),
        oid,
        detached: false,
        unborn: false,
      },
      operationState: "Clean",
      integration: null,
      capabilities: { readOnly: false, workingTree: true },
    });
    const client = { branches } as unknown as GitRepositoryClient;
    seedGitClient(client);
    const queryClient = createTestQueryClient();
    const onAction = vi.fn().mockResolvedValue(true);
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    const button = (name: string) =>
      [...document.querySelectorAll("button")].find(
        (b) => b.textContent === name || b.getAttribute("aria-label") === name,
      )!;
    try {
      await act(async () =>
        root.render(
          <GitTestProviders queryClient={queryClient}>
            <TooltipProvider>
              <GitBranchControls
                repository={repository}
                snapshot="status"
                busy={false}
                writable
                error=""
                onAction={onAction}
              />
            </TooltipProvider>
          </GitTestProviders>,
        ),
      );
      await act(async () => button("Branches: main").click());
      await openRowMenu("topic");
      await act(async () =>
        [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')]
          .find((item) => item.textContent?.startsWith("Delete"))!
          .click(),
      );
      const checkbox = [
        ...document.querySelectorAll<HTMLInputElement>("input[type=checkbox]"),
      ][0];
      expect(checkbox.checked).toBe(false);
      if (forced) {
        await act(async () => checkbox.click());
        expect(document.body.textContent).toContain("become unreferenced");
      }
      await act(async () => button("Delete branch").click());
      expect(onAction).toHaveBeenCalledExactlyOnceWith({
        kind: "branch.delete",
        name: "topic",
        expectedOid: "b".repeat(40),
        // Unforced deletions keep the original payload shape.
        ...(forced ? { force: true } : {}),
      });
    } finally {
      await act(async () => root.unmount());
      host.remove();
    }
  },
);

const mainOid = { algorithm: "sha1", hex: "a".repeat(40) };
const localBranch = (name: string, hex: string, current = false) => ({
  name: gitPath(name),
  reference: gitPath(`refs/heads/${name}`),
  oid: { algorithm: "sha1", hex: hex.repeat(40) },
  current,
  remote: false,
  upstream: null,
  tracking: null,
});
type BranchEntry = ReturnType<typeof localBranch>;
/**
 * The branch popover over `entries`, with the helpers the tests below share.
 * Unmount with `done`.
 */
async function mountBranches(
  entries: BranchEntry[] = [
    localBranch("main", "a", true),
    localBranch("topic", "b"),
  ],
) {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const branches = vi.fn().mockResolvedValue(
    decodeGitBranches({
      snapshot: "branches",
      nextCursor: null,
      metadata: {},
      entries,
    }),
  );
  const repository = decodeGitRepository({
    repoId: "repo",
    commonRepoId: "common",
    root: gitPath("/repo"),
    bare: false,
    objectFormat: "sha1",
    head: {
      name: gitPath("refs/heads/main"),
      oid: mainOid,
      detached: false,
      unborn: false,
    },
    operationState: "Clean",
    integration: null,
    capabilities: { readOnly: false, workingTree: true },
  });
  seedGitClient({ branches } as unknown as GitRepositoryClient);
  const queryClient = createTestQueryClient();
  const onAction = vi.fn().mockResolvedValue(true);
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const render = (busy: boolean) =>
    act(async () =>
      root.render(
        <GitTestProviders queryClient={queryClient}>
          <TooltipProvider>
            <GitBranchControls
              repository={repository}
              snapshot="status"
              busy={busy}
              writable
              error=""
              onAction={onAction}
            />
          </TooltipProvider>
        </GitTestProviders>,
      ),
    );
  await render(false);
  const button = (name: string) =>
    [...document.querySelectorAll("button")].find(
      (b) => b.textContent === name || b.getAttribute("aria-label") === name,
    )!;
  // Radix listens for outside presses on the document from the next tick, and
  // returns focus from a closed menu on one.
  const tick = () =>
    act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  const openPopover = async () => {
    await act(async () => button("Branches: main").click());
    await tick();
  };
  return {
    onAction,
    button,
    tick,
    openPopover,
    setBusy: render,
    popover: () =>
      document.querySelector('[role="dialog"][aria-label="Branches"]'),
    menu: () => document.querySelector<HTMLElement>('[role="menu"]'),
    filter: () =>
      document.querySelector<HTMLInputElement>(
        'input[aria-label="Filter branches"]',
      )!,
    menuItem: (label: string) =>
      [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(
        (item) => item.textContent === label,
      )!,
    done: async () => {
      await act(async () => root.unmount());
      host.remove();
    },
  };
}
const checkoutTopic = {
  kind: "checkout",
  target: { kind: "branch", name: "topic", expectedOid: "b".repeat(40) },
};
const key = (target: Element, init: KeyboardEventInit) =>
  act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", {
        bubbles: true,
        cancelable: true,
        ...init,
      }),
    );
  });

it("keeps the branch popover open while a row's context menu is in use, and switches from a row once", async () => {
  const { onAction, button, tick, openPopover, popover, menu, filter, done } =
    await mountBranches();
  // A whole click: Radix settles an outside press on its click, not on the
  // press alone.
  const press = (target: Element) =>
    act(async () => {
      for (const type of [
        "pointerdown",
        "mousedown",
        "pointerup",
        "mouseup",
        "click",
      ])
        target.dispatchEvent(new MouseEvent(type, { bubbles: true }));
    });
  try {
    // The trigger is still the picker styled by the app-wide button rule:
    // becoming the popover's trigger did not take its `data-slot` away.
    expect(button("Branches: main").dataset.slot).toBe("button");
    expect(button("Branches: main").getAttribute("aria-expanded")).toBe(
      "false",
    );
    await openPopover();
    expect(popover()).not.toBeNull();
    // Each row is a single control: nothing inside an option is a control of
    // its own, which assistive technology would not announce. The row keeps
    // the list's `data-slot` though it is also the context menu's trigger.
    for (const option of document.querySelectorAll('[role="option"]')) {
      expect(
        option.querySelectorAll("button, [role=button], [tabindex]"),
      ).toHaveLength(0);
      expect(option.getAttribute("data-slot")).toBe("command-item");
    }
    await openRowMenu("topic");
    await tick();
    // The menu is portalled out of the popover's DOM, and focus moved into it:
    // neither reads as leaving the popover. Asking for it chose nothing.
    expect(menu()?.getAttribute("aria-label")).toBe("Actions for topic");
    expect(popover()).not.toBeNull();
    expect(onAction).not.toHaveBeenCalled();
    // Clicking inside the menu, but on none of its items.
    await press(menu()!);
    await tick();
    expect(menu()).not.toBeNull();
    expect(popover()).not.toBeNull();
    // Escape dismisses the menu alone; the list it came from stays.
    await key(menu()!, { key: "Escape" });
    await tick();
    expect(menu()).toBeNull();
    expect(popover()).not.toBeNull();
    // Nothing in the menu reached the row underneath: no branch was switched.
    expect(onAction).not.toHaveBeenCalled();
    // Choosing the current branch's row does nothing.
    await act(async () => row("main").click());
    expect(onAction).not.toHaveBeenCalled();
    // Choosing another row switches to it, once.
    await act(async () => row("topic").click());
    expect(onAction).toHaveBeenCalledExactlyOnceWith(checkoutTopic);
    expect(popover()).toBeNull();
    onAction.mockClear();
    // Filtering and pressing Enter chooses the highlighted row, as a picker's
    // list does, under the same guards as choosing it with the pointer.
    await openPopover();
    await act(async () => {
      Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )!.set!.call(filter(), "TOP");
      filter().dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(
      [...document.querySelectorAll('[role="option"]')].map(
        (row) => row.querySelector("strong")?.textContent,
      ),
    ).toEqual(["topic"]);
    await key(filter(), { key: "Enter" });
    expect(onAction).toHaveBeenCalledExactlyOnceWith(checkoutTopic);
    onAction.mockClear();
    await openPopover();
    expect(popover()).not.toBeNull();
    // A press anywhere else still dismisses the popover.
    await press(document.body);
    await tick();
    expect(popover()).toBeNull();
    expect(onAction).not.toHaveBeenCalled();
  } finally {
    await done();
  }
});

it("offers each row's actions under the conditions its old actions menu had", async () => {
  const remote = {
    ...localBranch("origin/topic", "c"),
    reference: gitPath("refs/remotes/origin/topic"),
    remote: true,
  };
  const disabled = (label: string) =>
    view.menuItem(label).getAttribute("aria-disabled") === "true";
  const labels = () =>
    [...document.querySelectorAll('[role="menuitem"]')].map(
      (item) => item.textContent,
    );
  const close = async () => {
    await key(view.menu()!, { key: "Escape" });
    await view.tick();
  };
  const view = await mountBranches([
    localBranch("main", "a", true),
    localBranch("topic", "b"),
    remote,
  ]);
  try {
    await view.openPopover();
    await openRowMenu("main");
    // No tracking configuration to edit: the upstream action says why.
    expect(labels()).toEqual([
      "Rename…",
      "Copy Branch Name",
      "Upstream configuration unavailable or inherited",
      "Delete…",
    ]);
    expect(disabled("Rename…")).toBe(false);
    expect(disabled("Copy Branch Name")).toBe(false);
    expect(disabled("Upstream configuration unavailable or inherited")).toBe(
      true,
    );
    // The current branch cannot be deleted.
    expect(disabled("Delete…")).toBe(true);
    await close();
    await openRowMenu("topic");
    expect(disabled("Rename…")).toBe(false);
    expect(disabled("Delete…")).toBe(false);
    await close();
    // A remote-tracking row had no actions; its name can still be copied.
    await openRowMenu("origin/topic");
    expect(disabled("Rename…")).toBe(true);
    expect(disabled("Delete…")).toBe(true);
    expect(disabled("Upstream configuration unavailable or inherited")).toBe(
      true,
    );
    expect(disabled("Copy Branch Name")).toBe(false);
    await close();
    // Choosing a remote-tracking row switches nothing.
    await act(async () => row("origin/topic").click());
    expect(view.onAction).not.toHaveBeenCalled();
    // While the repository is busy nothing that writes is offered; copying,
    // which writes nothing, still is.
    await view.setBusy(true);
    expect(view.popover()).not.toBeNull();
    await openRowMenu("topic");
    expect(disabled("Rename…")).toBe(true);
    expect(disabled("Delete…")).toBe(true);
    expect(disabled("Copy Branch Name")).toBe(false);
  } finally {
    await view.done();
  }
});

it.each([true, false])(
  "copies a branch's name and says whether it could (clipboard works: %s)",
  async (works) => {
    const writeText = works
      ? vi.fn().mockResolvedValue(undefined)
      : vi.fn().mockRejectedValue(new Error("denied"));
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    vi.mocked(toast.success).mockClear();
    vi.mocked(toast.error).mockClear();
    const { onAction, tick, openPopover, popover, menu, menuItem, done } =
      await mountBranches();
    try {
      await openPopover();
      await openRowMenu("topic");
      await act(async () => menuItem("Copy Branch Name").click());
      await tick();
      expect(writeText).toHaveBeenCalledExactlyOnceWith("topic");
      if (works) {
        expect(toast.success).toHaveBeenCalledOnce();
        expect(vi.mocked(toast.success).mock.calls[0][0]).toContain("topic");
        expect(toast.error).not.toHaveBeenCalled();
      } else {
        expect(toast.error).toHaveBeenCalledOnce();
        expect(vi.mocked(toast.error).mock.calls[0][0]).toContain("denied");
        expect(toast.success).not.toHaveBeenCalled();
      }
      // Copying is not choosing: nothing switched, and the list stays.
      expect(menu()).toBeNull();
      expect(popover()).not.toBeNull();
      expect(onAction).not.toHaveBeenCalled();
    } finally {
      await done();
    }
  },
);

it("opens the highlighted row's context menu from the keyboard, anchored to that row", async () => {
  const { onAction, tick, openPopover, popover, menu, filter, done } =
    await mountBranches();
  try {
    await openPopover();
    // The filter keeps focus and tells assistive technology how to reach the
    // highlighted row's actions.
    expect(document.activeElement).toBe(filter());
    const hint = document.getElementById(
      filter().getAttribute("aria-describedby")!,
    );
    expect(hint?.textContent).toContain("Shift+F10");
    // Highlight "topic", the second row.
    await key(filter(), { key: "ArrowDown" });
    expect(filter().getAttribute("aria-activedescendant")).toBe(
      row("topic").id,
    );
    // jsdom lays nothing out; give the row a place to anchor the menu to.
    vi.spyOn(row("topic"), "getBoundingClientRect").mockReturnValue(
      new DOMRect(100, 200, 300, 30),
    );
    const asked: MouseEvent[] = [];
    row("topic").addEventListener("contextmenu", (event) => asked.push(event));
    let handled = false;
    await act(async () => {
      handled = !filter().dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "F10",
          shiftKey: true,
          bubbles: true,
          cancelable: true,
        }),
      );
    });
    await tick();
    // Shift+F10 went to the highlighted row, at its start and just below it,
    // and raised no native menu on the filter.
    expect(handled).toBe(true);
    expect(asked).toHaveLength(1);
    expect([asked[0].clientX, asked[0].clientY]).toEqual([110, 230]);
    expect(menu()?.getAttribute("aria-label")).toBe("Actions for topic");
    expect(popover()).not.toBeNull();
    // Focus is in the menu, on its first action, ready for the arrow keys.
    expect(menu()!.contains(document.activeElement)).toBe(true);
    expect(document.activeElement?.textContent).toBe("Rename…");
    // Moving in the menu does not move the list's highlight under it.
    await key(document.activeElement!, { key: "ArrowDown" });
    await tick();
    expect(document.activeElement?.textContent).toBe("Copy Branch Name");
    expect(filter().getAttribute("aria-activedescendant")).toBe(
      row("topic").id,
    );
    // Escape closes the menu alone and returns focus to the filter.
    await key(document.activeElement!, { key: "Escape" });
    await tick();
    expect(menu()).toBeNull();
    expect(popover()).not.toBeNull();
    expect(document.activeElement).toBe(filter());
    // The ContextMenu key does the same, and Enter on an action takes it
    // without also choosing the row underneath.
    await key(filter(), { key: "ContextMenu" });
    await tick();
    expect(menu()?.getAttribute("aria-label")).toBe("Actions for topic");
    await key(document.activeElement!, { key: "Enter" });
    await tick();
    expect(menu()).toBeNull();
    expect(document.querySelector('[role="dialog"]')?.textContent).toContain(
      "Rename branch",
    );
    expect(onAction).not.toHaveBeenCalled();
  } finally {
    await done();
  }
});

it("asks for no menu from the keyboard when no row is highlighted", async () => {
  const { openPopover, menu, filter, done } = await mountBranches();
  try {
    await openPopover();
    await act(async () => {
      Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )!.set!.call(filter(), "nothing matches");
      filter().dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(document.querySelectorAll('[role="option"]')).toHaveLength(0);
    let handled = true;
    await act(async () => {
      handled = !filter().dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "F10",
          shiftKey: true,
          bubbles: true,
          cancelable: true,
        }),
      );
    });
    expect(handled).toBe(false);
    expect(menu()).toBeNull();
  } finally {
    await done();
  }
});
