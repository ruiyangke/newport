// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClientProvider, notifyManager } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createQueryClient } from "../query/client";
import { useSharedMutation } from "./useSharedMutation";

let root: Root;
let client: ReturnType<typeof createQueryClient>;
let mutation: ReturnType<typeof useSharedMutation<string, string>>;
const work = vi.fn<(input: string) => Promise<string>>();
notifyManager.setScheduler(queueMicrotask);
function Probe({ scope }: { scope: string }) {
  mutation = useSharedMutation({ mutationKey: [scope], mutationFn: work });
  return null;
}
async function render(scope: string | null) {
  await act(async () =>
    root.render(
      <QueryClientProvider client={client}>
        {scope && <Probe key={scope} scope={scope} />}
      </QueryClientProvider>,
    ),
  );
}
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  client = createQueryClient();
  root = createRoot(document.createElement("div"));
  work.mockReset();
});
afterEach(async () => {
  await act(() => root.unmount());
  client.clear();
});
it("keeps pending/results across navigation and rejects same-tick duplicate actions", async () => {
  let finish!: (result: string) => void;
  work.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  await render("a");
  await act(async () => {
    mutation.mutate("first");
    mutation.mutate("duplicate");
  });
  expect(work).toHaveBeenCalledTimes(1);
  await render(null);
  await render("b");
  expect(mutation.isPending).toBe(false);
  await render("a");
  expect(mutation.isPending).toBe(true);
  await act(async () => {
    mutation.mutate("duplicate after remount");
  });
  expect(work).toHaveBeenCalledTimes(1);
  await act(async () => finish("done"));
  expect(mutation.data).toBe("done");
  expect(mutation.isSuccess).toBe(true);
});
it("preserves errors arriving while unmounted and supports clearing and retrying", async () => {
  let reject!: (reason: Error) => void;
  work.mockImplementation(
    () =>
      new Promise((_resolve, no) => {
        reject = no;
      }),
  );
  await render("a");
  await act(async () => mutation.mutate("first"));
  await render(null);
  await act(async () => reject(new Error("remote failed")));
  await render("a");
  expect(mutation.error?.message).toBe("remote failed");
  await act(async () => mutation.reset());
  expect(mutation.error).toBeNull();
  work.mockResolvedValue("retried");
  await act(async () => mutation.mutate("second"));
  expect(mutation.data).toBe("retried");
});
