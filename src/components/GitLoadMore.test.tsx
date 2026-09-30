// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { GitLoadMore } from "./GitLoadMore";

it("observes the list viewport, stops on errors/end, and leaves an explicit retry", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  let notify!: IntersectionObserverCallback;
  const observe = vi.fn();
  const disconnect = vi.fn();
  const options: IntersectionObserverInit[] = [];
  vi.stubGlobal(
    "IntersectionObserver",
    class {
      constructor(
        callback: IntersectionObserverCallback,
        init: IntersectionObserverInit,
      ) {
        notify = callback;
        options.push(init);
      }
      observe = observe;
      disconnect = disconnect;
    },
  );
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const onLoad = vi.fn();
  const props = {
    cursor: "next" as string | null,
    loading: false,
    error: "",
    onLoad,
    label: "Load more commits",
    endLabel: "End of history",
  };
  async function render(next = props) {
    await act(async () =>
      root.render(
        <div style={{ overflowY: "auto" }}>
          <GitLoadMore {...next} />
        </div>,
      ),
    );
  }
  try {
    await render();
    expect(options[0].root).toBe(container.firstElementChild);
    await act(async () =>
      notify(
        [{ isIntersecting: true }] as IntersectionObserverEntry[],
        {} as IntersectionObserver,
      ),
    );
    expect(onLoad).toHaveBeenCalledTimes(1);
    await render({ ...props, loading: true });
    expect(container.querySelector("button")?.disabled).toBe(true);
    expect(disconnect).toHaveBeenCalled();
    await render({ ...props, error: "Disconnected" });
    expect(container.querySelector('[role="alert"]')?.textContent).toBe(
      "Disconnected",
    );
    expect(options).toHaveLength(1);
    await act(async () => container.querySelector("button")!.click());
    expect(onLoad).toHaveBeenCalledTimes(2);
    await render({ ...props, cursor: null });
    expect(container.querySelector("button")).toBeNull();
    expect(container.querySelector('[role="status"]')?.textContent).toBe(
      "End of history",
    );
  } finally {
    await act(async () => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
  }
});
