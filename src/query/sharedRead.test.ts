import { expect, it, vi } from "vitest";
import { createQueryClient, readSharedQuery } from "./client";

it("shares one request and retains it when only one consumer cancels", async () => {
  const client = createQueryClient();
  let finish!: (value: string) => void;
  let transport!: AbortSignal;
  const queryFn = vi.fn(({ signal }: { signal: AbortSignal }) => {
    transport = signal;
    return new Promise<string>((resolve) => {
      finish = resolve;
    });
  });
  const options = { queryKey: ["shared"], queryFn, staleTime: Infinity };
  const a = new AbortController();
  const b = new AbortController();
  const first = readSharedQuery(client, options, a.signal);
  const second = readSharedQuery(client, options, b.signal);
  const rejected = expect(first).rejects.toMatchObject({ name: "AbortError" });
  a.abort();
  await rejected;
  expect(queryFn).toHaveBeenCalledTimes(1);
  expect(transport.aborted).toBe(false);
  finish("page");
  await expect(second).resolves.toBe("page");
  expect(
    client
      .getQueryCache()
      .find({ queryKey: options.queryKey })
      ?.getObserversCount(),
  ).toBe(0);
  await expect(
    readSharedQuery(client, options, new AbortController().signal),
  ).resolves.toBe("page");
  expect(queryFn).toHaveBeenCalledTimes(1);
  client.clear();
});

it("aborts the query when its last page consumer cancels", async () => {
  const client = createQueryClient();
  let transport!: AbortSignal;
  const controller = new AbortController();
  const queryKey = ["abandoned"];
  const pending = readSharedQuery(
    client,
    {
      queryKey,
      queryFn: ({ signal }) => {
        transport = signal;
        return new Promise<string>(() => {});
      },
    },
    controller.signal,
  );
  const rejected = expect(pending).rejects.toMatchObject({
    name: "AbortError",
  });
  controller.abort();
  await rejected;
  expect(transport.aborted).toBe(true);
  expect(client.getQueryState(queryKey)?.fetchStatus).toBe("idle");
  expect(client.getQueryCache().find({ queryKey })?.getObserversCount()).toBe(
    0,
  );
  client.clear();
});

it("does not start a previously cancelled read", async () => {
  const client = createQueryClient();
  const controller = new AbortController();
  controller.abort();
  const queryFn = vi.fn(async () => "page");
  await expect(
    readSharedQuery(
      client,
      { queryKey: ["cancelled"], queryFn },
      controller.signal,
    ),
  ).rejects.toMatchObject({ name: "AbortError" });
  expect(queryFn).not.toHaveBeenCalled();
  expect(client.getQueryCache().getAll()).toHaveLength(0);
  client.clear();
});
