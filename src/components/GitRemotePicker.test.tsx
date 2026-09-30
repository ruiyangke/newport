// @vitest-environment jsdom
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { notifyManager } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { GitRemotePicker } from "./GitRemotePicker";
import { useGitRemoteSelection } from "../hooks/useGitRemoteSelection";
import {
  GitTestProviders,
  createTestQueryClient,
  seedGitClient,
} from "../git/testing";
notifyManager.setScheduler(queueMicrotask);
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;
Element.prototype.scrollIntoView ??= () => {};
const page = (
  names: string[],
  nextCursor: string | null = null,
  snapshot = "names",
) => ({
  snapshot,
  entries: names.map((name) => ({ name })),
  nextCursor,
  metadata: { totalEntries: nextCursor ? 2 : names.length },
});
const detail = (name: string) => ({
  name,
  url: `https://example.test/${name}`,
  pushUrl: null,
  token: `token-${name}`,
});
const click = async (name: string) =>
  act(async () => {
    const el = [
      ...document.querySelectorAll<HTMLElement>('button,[role="option"]'),
    ].find(
      (el) => el.textContent === name || el.getAttribute("aria-label") === name,
    );
    expect(el).toBeTruthy();
    el!.click();
  });
function mount(child: React.ReactNode) {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const queryClient = createTestQueryClient();
  return {
    host,
    root,
    queryClient,
    render: () =>
      act(async () =>
        root.render(
          <GitTestProviders queryClient={queryClient}>
            {child}
          </GitTestProviders>,
        ),
      ),
    close: async () => {
      await act(async () => root.unmount());
      host.remove();
      queryClient.clear();
    },
  };
}
it("retains loaded choices on continuation failure and retries without hydrating every remote", async () => {
  let fail = true;
  const remoteNames = vi.fn(async (params: { cursor?: string }) => {
    if (!params.cursor) return page(["origin"], "next");
    if (fail) throw { message: "Page interrupted" };
    return { ...page(["z-last"]), metadata: { totalEntries: 2 } };
  });
  const remote = vi.fn();
  seedGitClient({ remoteNames, remote });
  const selected = vi.fn();
  const view = mount(
    <GitRemotePicker repoId="repo" value="origin" onChange={selected} />,
  );
  try {
    await view.render();
    expect(remoteNames).not.toHaveBeenCalled();
    await click("Remote");
    await click("Load more remotes");
    expect(document.body.textContent).toContain("Page interrupted");
    expect(document.querySelector('[role="option"]')?.textContent).toContain(
      "origin",
    );
    fail = false;
    await click("Retry: load more remotes");
    expect(
      [...document.querySelectorAll('[role="option"]')].map(
        (el) => el.textContent,
      ),
    ).toEqual(["origin", "z-last"]);
    expect(document.body.textContent).toContain("All remotes loaded");
    await click("z-last");
    expect(selected).toHaveBeenCalledWith("z-last");
    expect(remote).not.toHaveBeenCalled();
  } finally {
    await view.close();
  }
});
it("ignores a late configuration response after changing the selected remote", async () => {
  let resolve!: (value: ReturnType<typeof detail>) => void;
  const remote = vi.fn((_repo: string, name: string) =>
    name === "origin"
      ? new Promise<ReturnType<typeof detail>>((r) => {
          resolve = r;
        })
      : Promise.resolve(detail(name)),
  );
  const remoteNames = vi.fn().mockResolvedValue(page(["origin", "z-last"]));
  seedGitClient({ remote, remoteNames });
  function Harness() {
    const [value, setValue] = useState("");
    const s = useGitRemoteSelection("repo", value, setValue);
    return (
      <>
        <button onClick={() => setValue("z-last")}>Switch</button>
        <output>{s.remote?.token ?? (s.loading ? "Loading" : s.error)}</output>
      </>
    );
  }
  const view = mount(<Harness />);
  try {
    await view.render();
    expect(view.host.textContent).toContain("Loading");
    await click("Switch");
    expect(view.host.textContent).toContain("token-z-last");
    await act(async () => resolve(detail("origin")));
    expect(view.host.textContent).toContain("token-z-last");
    expect(view.host.textContent).not.toContain("token-origin");
    expect(remoteNames).not.toHaveBeenCalled();
  } finally {
    await view.close();
  }
});
it("falls back after a deleted selection and treats an empty list as complete", async () => {
  const remote = vi.fn(async (_repo: string, name: string) => {
    if (name === "origin")
      throw { code: "REMOTE_NOT_FOUND", message: "Missing" };
    return detail(name);
  });
  const remoteNames = vi.fn().mockResolvedValue(page(["upstream"]));
  seedGitClient({ remote, remoteNames });
  function Harness() {
    const [value, setValue] = useState("");
    const s = useGitRemoteSelection("repo", value, setValue);
    return (
      <>
        <button onClick={s.refresh}>Refresh</button>
        <output>
          {s.empty
            ? "Empty"
            : (s.remote?.token ?? (s.loading ? "Loading" : s.error))}
        </output>
      </>
    );
  }
  const view = mount(<Harness />);
  try {
    await view.render();
    expect(view.host.textContent).toContain("token-upstream");
    remoteNames.mockResolvedValue(page([]));
    remote.mockRejectedValue({ code: "REMOTE_NOT_FOUND", message: "Missing" });
    await click("Refresh");
    expect(view.host.textContent).toContain("Empty");
    expect(view.host.textContent).not.toContain("token-upstream");
  } finally {
    await view.close();
  }
});

it("searches on the server and ignores a previous search's pending continuation", async () => {
  let resolve!: (value: ReturnType<typeof page>) => void;
  const remoteNames = vi.fn(
    async (params: { cursor?: string; filter?: string }) => {
      if (params.filter === "late")
        return page(["late-remote"], null, "filtered");
      if (params.cursor)
        return new Promise<ReturnType<typeof page>>((r) => {
          resolve = r;
        });
      return page(["origin"], "next");
    },
  );
  seedGitClient({ remoteNames });
  const selected = vi.fn();
  const view = mount(
    <GitRemotePicker repoId="repo" value="origin" onChange={selected} />,
  );
  try {
    await view.render();
    await click("Remote");
    await click("Load more remotes");
    await act(async () => {
      const input = document.querySelector<HTMLInputElement>(
        'input[aria-label="Search remotes"]',
      )!;
      Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )!.set!.call(input, "late");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 230));
    });
    expect(remoteNames).toHaveBeenCalledWith({
      repoId: "repo",
      filter: "late",
      pageSize: 20,
    });
    await act(async () =>
      resolve({ ...page(["old-result"]), metadata: { totalEntries: 2 } }),
    );
    expect(
      [...document.querySelectorAll('[role="option"]')].map(
        (el) => el.textContent,
      ),
    ).toEqual(["late-remote"]);
    await click("late-remote");
    expect(selected).toHaveBeenCalledWith("late-remote");
  } finally {
    await view.close();
  }
});
