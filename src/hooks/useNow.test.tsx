// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useNow } from "./useNow";

let root: Root;
let seen: number[] = [];
function Probe({ interval }: { interval: number }) {
  seen.push(useNow(interval));
  return null;
}
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  root = createRoot(document.createElement("div"));
  seen = [];
  vi.useFakeTimers();
});
afterEach(async () => {
  await act(() => root.unmount());
  vi.useRealTimers();
});

it("ticks on its interval and on focus", async () => {
  await act(async () => root.render(<Probe interval={5_000} />));
  const first = seen.at(-1)!;
  await act(async () => {
    vi.advanceTimersByTime(5_000);
  });
  expect(seen.at(-1)!).toBeGreaterThan(first);
  const second = seen.at(-1)!;
  await act(async () => {
    window.dispatchEvent(new Event("focus"));
  });
  expect(seen.at(-1)!).toBeGreaterThanOrEqual(second);
});
