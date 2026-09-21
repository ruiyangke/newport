// @vitest-environment jsdom
import { act, lazy, Suspense } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { ErrorBoundary } from "./ErrorBoundary";

it("contains rendering and lazy-load failures and supports retry and navigation", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  const host = document.createElement("div");
  const root = createRoot(host);
  let broken = true;
  function Panel() {
    if (broken) throw new Error("Fixture render failure");
    return <p>Workspace restored</p>;
  }
  try {
    await act(async () =>
      root.render(
        <>
          <nav>Navigation</nav>
          <ErrorBoundary scope="workspace">
            <Panel />
          </ErrorBoundary>
        </>,
      ),
    );
    expect(host.querySelector("nav")?.textContent).toBe("Navigation");
    expect(host.querySelector('[role="alert"]')).not.toBeNull();
    broken = false;
    await act(async () => host.querySelector("button")!.click());
    expect(host.textContent).toContain("Workspace restored");
    const Rejected = lazy(() => Promise.reject(new Error("Chunk unavailable")));
    await act(async () =>
      root.render(
        <ErrorBoundary key="files" scope="workspace">
          <Suspense fallback="Loading">
            <Rejected />
          </Suspense>
        </ErrorBoundary>,
      ),
    );
    expect(host.textContent).toContain("Reload window");
    await act(async () =>
      root.render(
        <ErrorBoundary key="overview" scope="workspace">
          <p>Overview</p>
        </ErrorBoundary>,
      ),
    );
    expect(host.textContent).toBe("Overview");
  } finally {
    await act(async () => root.unmount());
    log.mockRestore();
  }
});
