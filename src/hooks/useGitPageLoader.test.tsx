// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import {
  notifyManager,
  QueryClientProvider,
  useQuery,
} from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { createQueryClient } from "../query/client";
import { type GitPage } from "../domain/gitResponses";
import { useGitPageLoader } from "./useGitPageLoader";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
notifyManager.setScheduler(queueMicrotask);
type Page = GitPage<string, Record<string, never>>;
const first: Page = {
  snapshot: "s",
  entries: ["a"],
  metadata: {},
  nextCursor: "next",
};
const last: Page = { ...first, entries: ["a", "b"], nextCursor: null };
function deferred() {
  let resolve!: (page: Page) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<Page>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}
async function harness(
  read: (cursor: string, signal: AbortSignal) => Promise<Page>,
  prefetch = false,
  entryKey = (entry: string) => entry,
) {
  const client = createQueryClient();
  client.setQueryData(["one"], first);
  client.setQueryData(["two"], first);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  let loader!: ReturnType<
    typeof useGitPageLoader<string, Record<string, never>>
  >;
  function View({ repo, enabled }: { repo: string; enabled: boolean }) {
    const query = useQuery({
      queryKey: [repo],
      queryFn: async () => first,
      enabled: false,
    });
    loader = useGitPageLoader({
      queryKey: [repo],
      page: query.data ?? null,
      enabled,
      read,
      entryKey,
      prefetch,
    });
    return null;
  }
  async function render(repo = "one", enabled = true) {
    await act(async () =>
      root.render(
        <QueryClientProvider client={client}>
          <View repo={repo} enabled={enabled} />
        </QueryClientProvider>,
      ),
    );
  }
  await render();
  return {
    client,
    render,
    loader: () => loader,
    close: async () => {
      await act(async () => root.unmount());
      container.remove();
      client.clear();
    },
  };
}

it.each([false, true])(
  "aborts the read signal on invalidation (prefetch=%s)",
  async (prefetch) => {
    const next = deferred();
    let signal!: AbortSignal;
    const read = vi.fn((_cursor: string, incoming: AbortSignal) => {
      signal = incoming;
      return next.promise;
    });
    const h = await harness(read, prefetch);
    try {
      let loading: Promise<void> | undefined;
      if (prefetch)
        await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
      else
        await act(async () => {
          loading = h.loader().load();
        });
      expect(signal.aborted).toBe(false);
      await act(async () => {
        await h.client.invalidateQueries({
          queryKey: ["one"],
          refetchType: "none",
        });
      });
      expect(signal.aborted).toBe(true);
      await act(async () => {
        next.resolve(last);
        await loading;
      });
      expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual(["a"]);
    } finally {
      await h.close();
    }
  },
);

it("cancels a consumed prefetch when its view closes", async () => {
  const next = deferred();
  let signal!: AbortSignal;
  const read = vi.fn((_cursor: string, incoming: AbortSignal) => {
    signal = incoming;
    return next.promise;
  });
  const h = await harness(read, true);
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  let loading!: Promise<void>;
  await act(async () => {
    loading = h.loader().load();
  });
  await h.close();
  expect(signal.aborted).toBe(true);
  next.resolve(last);
  await loading;
  expect(read).toHaveBeenCalledTimes(1);
});

it("prefetches only one upcoming page and consumes it without a second request", async () => {
  const read = vi.fn().mockResolvedValue(last);
  const h = await harness(read, true);
  try {
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual(["a"]);
    expect(h.loader().loading).toBe(false);
    await act(async () => h.loader().load());
    expect(read).toHaveBeenCalledTimes(1);
    expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual(["a", "b"]);
  } finally {
    await h.close();
  }
});

it("keeps speculative errors quiet and retries on demand", async () => {
  const read = vi
    .fn()
    .mockRejectedValueOnce(new Error("offline"))
    .mockResolvedValue(last);
  const h = await harness(read, true);
  try {
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    expect(h.loader().error).toBe("");
    await act(async () => h.loader().load());
    expect(read).toHaveBeenCalledTimes(2);
    expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual(["a", "b"]);
  } finally {
    await h.close();
  }
});

it("does not reuse a prefetched page after a refresh of the same snapshot", async () => {
  const read = vi.fn().mockResolvedValue(last);
  const h = await harness(read, true);
  try {
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    await act(async () =>
      h.client.setQueryData(["one"], { ...first, entries: ["fresh"] }),
    );
    await act(async () => h.loader().load());
    expect(read).toHaveBeenCalledTimes(2);
    expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual([
      "fresh",
      "a",
      "b",
    ]);
  } finally {
    await h.close();
  }
});

it("coalesces concurrent loads and preserves order while deduplicating", async () => {
  const next = deferred();
  const read = vi.fn(() => next.promise);
  const h = await harness(read);
  try {
    let pending!: Promise<void>;
    await act(async () => {
      pending = h.loader().load();
      void h.loader().load();
    });
    expect(read).toHaveBeenCalledTimes(1);
    expect(h.loader().loading).toBe(true);
    await act(async () => {
      next.resolve(last);
      await pending;
    });
    expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual(["a", "b"]);
    expect(h.loader().loading).toBe(false);
    await act(async () => h.loader().load());
    expect(read).toHaveBeenCalledTimes(1);
  } finally {
    await h.close();
  }
});

it.each(["switch", "disable", "refresh", "invalidate"])(
  "ignores a page after %s",
  async (change) => {
    const next = deferred();
    const h = await harness(() => next.promise);
    try {
      let pending!: Promise<void>;
      await act(async () => {
        pending = h.loader().load();
      });
      if (change === "switch") await h.render("two");
      if (change === "disable") await h.render("one", false);
      if (change === "refresh")
        await act(async () => {
          h.client.setQueryData(["one"], {
            ...first,
            entries: ["replacement"],
          });
        });
      if (change === "invalidate")
        await act(async () => {
          await h.client.invalidateQueries({
            queryKey: ["one"],
            refetchType: "none",
          });
        });
      await act(async () => {
        next.resolve(last);
        await pending;
      });
      expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual(
        change === "refresh" ? ["replacement"] : ["a"],
      );
      expect(h.client.getQueryData<Page>(["two"])?.entries).toEqual(["a"]);
      expect(h.loader().loading).toBe(false);
    } finally {
      await h.close();
    }
  },
);

it("a refreshed page can load while its previous read is still pending", async () => {
  const previous = deferred();
  const read = vi
    .fn()
    .mockReturnValueOnce(previous.promise)
    .mockResolvedValue(last);
  const h = await harness(read);
  try {
    let oldLoad!: Promise<void>;
    await act(async () => {
      oldLoad = h.loader().load();
    });
    await act(async () => {
      h.client.setQueryData(["one"], { ...first, entries: ["fresh"] });
    });
    await act(async () => h.loader().load());
    expect(read).toHaveBeenCalledTimes(2);
    expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual([
      "fresh",
      "a",
      "b",
    ]);
    await act(async () => {
      previous.reject(new Error("Old read failed"));
      await oldLoad;
    });
    expect(h.loader().error).toBe("");
  } finally {
    await h.close();
  }
});

it("does not publish a read error after the listing is invalidated", async () => {
  const next = deferred();
  const h = await harness(() => next.promise);
  try {
    let pending!: Promise<void>;
    await act(async () => {
      pending = h.loader().load();
    });
    await act(async () => {
      await h.client.invalidateQueries({
        queryKey: ["one"],
        refetchType: "none",
      });
    });
    await act(async () => {
      next.reject(new Error("Old cursor expired"));
      await pending;
    });
    expect(h.loader().error).toBe("");
    expect(h.loader().loading).toBe(false);
  } finally {
    await h.close();
  }
});

it("releases an obsolete load even when a refresh retains the same page object and timestamp", async () => {
  const old = deferred();
  const read = vi.fn().mockReturnValueOnce(old.promise).mockResolvedValue(last);
  const h = await harness(read);
  try {
    let pending!: Promise<void>;
    await act(async () => {
      pending = h.loader().load();
    });
    const state = h.client.getQueryState(["one"])!;
    const page = h.client.getQueryData(["one"]);
    await act(async () => {
      h.client.setQueryData(["one"], page, { updatedAt: state.dataUpdatedAt });
    });
    expect(h.client.getQueryData(["one"])).toBe(page);
    await act(async () => h.loader().load());
    expect(read).toHaveBeenCalledTimes(2);
    await act(async () => {
      old.reject(new Error("obsolete"));
      await pending;
    });
    expect(h.loader().error).toBe("");
    expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual(["a", "b"]);
  } finally {
    await h.close();
  }
});

it("does not reuse speculative data across an identical refresh", async () => {
  const read = vi.fn().mockResolvedValue(last);
  const h = await harness(read, true);
  try {
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    const state = h.client.getQueryState(["one"])!;
    await act(async () => {
      h.client.setQueryData(["one"], h.client.getQueryData(["one"]), {
        updatedAt: state.dataUpdatedAt,
      });
    });
    await act(async () => h.loader().load());
    expect(read).toHaveBeenCalledTimes(2);
  } finally {
    await h.close();
  }
});

it("keeps existing rows on errors and permits an explicit retry", async () => {
  const read = vi
    .fn()
    .mockRejectedValueOnce(new Error("Connection lost"))
    .mockResolvedValue(last);
  const h = await harness(read);
  try {
    await act(async () => h.loader().load());
    expect(h.loader().error).toBe("Connection lost");
    expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual(["a"]);
    await act(async () => h.loader().load());
    expect(h.loader().error).toBe("");
    expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual(["a", "b"]);
  } finally {
    await h.close();
  }
});

it("rejects a different snapshot instead of mixing rows", async () => {
  const h = await harness(async () => ({ ...last, snapshot: "new" }));
  try {
    await act(async () => h.loader().load());
    expect(h.loader().error).toBeTruthy();
    expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual(["a"]);
  } finally {
    await h.close();
  }
});

it("stops a non-progressing cursor chain", async () => {
  const h = await harness(async () => ({ ...first, nextCursor: "another" }));
  try {
    await act(async () => h.loader().load());
    expect(h.loader().error).toContain("did not add any results");
    expect(h.client.getQueryData<Page>(["one"])?.nextCursor).toBe("next");
  } finally {
    await h.close();
  }
});

it("reuses the key index after React Query structurally shares the appended page", async () => {
  const read = vi
    .fn()
    .mockResolvedValueOnce({ ...first, entries: ["b"], nextCursor: "tail" })
    .mockResolvedValue({ ...first, entries: ["c"], nextCursor: null });
  const key = vi.fn((entry: string) => entry);
  const h = await harness(read, false, key);
  try {
    await act(async () => h.loader().load());
    await act(async () => h.loader().load());
    expect(h.client.getQueryData<Page>(["one"])?.entries).toEqual([
      "a",
      "b",
      "c",
    ]);
    expect(key.mock.calls.map(([entry]) => entry)).toEqual(["a", "b", "c"]);
  } finally {
    await h.close();
  }
});

it.each(["identical refresh", "changed refresh", "invalidation", "disable"])(
  "clears a displayed page error after %s without retrying the obsolete cursor",
  async (change) => {
    const read = vi.fn().mockRejectedValue(new Error("Expired cursor"));
    const h = await harness(read);
    try {
      await act(async () => h.loader().load());
      expect(h.loader().error).toBe("Expired cursor");
      await act(async () => {
        h.client.setQueryData(["two"], last);
      });
      expect(h.loader().error).toBe("Expired cursor");
      if (change === "disable") await h.render("one", false);
      else
        await act(async () => {
          if (change === "invalidation") {
            await h.client.invalidateQueries({
              queryKey: ["one"],
              refetchType: "none",
            });
          } else {
            const previous = h.client.getQueryState(["one"])!;
            h.client.setQueryData(
              ["one"],
              change === "identical refresh"
                ? h.client.getQueryData(["one"])
                : { ...first, entries: ["fresh"] },
              { updatedAt: previous.dataUpdatedAt },
            );
          }
        });
      expect(h.loader().error).toBe("");
      expect(read).toHaveBeenCalledTimes(1);
    } finally {
      await h.close();
    }
  },
);
