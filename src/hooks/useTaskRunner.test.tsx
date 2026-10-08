// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useTaskRunner } from "./useTaskRunner";

let root: Root;
let runner: ReturnType<typeof useTaskRunner>;
const refresh = vi.fn<() => Promise<void>>();
function Probe() {
  runner = useTaskRunner(refresh);
  return null;
}
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  root = createRoot(document.createElement("div"));
  refresh.mockReset();
  refresh.mockResolvedValue(undefined);
});
afterEach(async () => {
  await act(() => root.unmount());
});
async function render() {
  await act(async () => root.render(<Probe />));
}

it("dedupes in-flight keys and tracks pending", async () => {
  let finish!: () => void;
  await render();
  const task = vi.fn(
    () =>
      new Promise<unknown>((resolve) => {
        finish = () => resolve(undefined);
      }),
  );
  let first!: Promise<void>;
  await act(async () => {
    first = runner.act("k", task);
    await runner.act("k", task);
  });
  expect(task).toHaveBeenCalledTimes(1);
  expect(runner.pending.has("k")).toBe(true);
  await act(async () => finish());
  await act(async () => first);
  expect(runner.pending.has("k")).toBe(false);
  expect(refresh).toHaveBeenCalledTimes(1);
});

it("keeps action errors in their own channel", async () => {
  await render();
  await act(async () => {
    await runner.act("fail", () => Promise.reject(new Error("boom")));
  });
  expect(runner.actionError).toBe("Error: boom");
  // A new action clears the previous failure, without touching anything else.
  await act(async () => {
    await runner.act("ok", () => Promise.resolve());
  });
  expect(runner.actionError).toBe("");
  await act(async () => {
    await runner.act("fail", () => Promise.reject(new Error("again")));
  });
  expect(runner.actionError).toBe("Error: again");
  await act(async () => runner.clearActionError());
  expect(runner.actionError).toBe("");
});
