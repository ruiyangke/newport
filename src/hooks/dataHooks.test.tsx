// @vitest-environment jsdom
import {
  QueryClientProvider,
  notifyManager,
  type QueryClient,
} from "@tanstack/react-query";
import { createQueryClient } from "../query/client";
import { ServerScopeProvider } from "../query/keys";
import type { ReactNode } from "react";
import { act, useRef, useLayoutEffect } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { useSnapshot } from "./useSnapshot";
import { useCollection } from "./useCollection";
import type { Snapshot } from "../types";

const api = vi.hoisted(() => ({ desktop: vi.fn(), collect: vi.fn() }));
vi.mock("../api/desktop", () => api);
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const snapshot = (name: string): Snapshot => ({
  config: {
    servers: [
      {
        id: name,
        name,
        sshHost: "host",
        sshUser: "user",
        sshPort: 22,
        identityFile: null,
        authMethod: "publicKey",
      },
    ],
    tunnels: [],
  },
  runtime: { tunnels: {}, clipboard: {}, health: {} },
  loadError: null,
});
notifyManager.setScheduler(queueMicrotask);
let client: QueryClient;
function render(children: ReactNode, id = "a", host = "host", revision = 0) {
  const server = { ...snapshot(id).config.servers[0], sshHost: host };
  root.render(
    <QueryClientProvider client={client}>
      <ServerScopeProvider server={server} revision={revision}>
        {children}
      </ServerScopeProvider>
    </QueryClientProvider>,
  );
}
let root: Root;
let host: HTMLDivElement;
let hidden = false;
beforeEach(() => {
  client = createQueryClient();
  api.desktop.mockReset();
  api.collect.mockReset();
  vi.useFakeTimers();
  hidden = false;
  Object.defineProperty(document, "hidden", {
    configurable: true,
    get: () => hidden,
  });
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  client.clear();
  host.remove();
  vi.useRealTimers();
});

it("ignores an older poll finishing after a post-action refresh", async () => {
  const old = deferred<Snapshot>();
  const fresh = deferred<Snapshot>();
  api.desktop
    .mockReturnValueOnce(old.promise)
    .mockReturnValueOnce(fresh.promise);
  let state!: ReturnType<typeof useSnapshot>;
  function Probe() {
    state = useSnapshot(true);
    return null;
  }
  await act(async () => render(<Probe />));
  let refresh!: Promise<void>;
  await act(async () => {
    refresh = state.refresh();
  });
  await act(async () => {
    fresh.resolve(snapshot("new"));
    await refresh;
  });
  await act(async () => {
    old.resolve(snapshot("old"));
  });
  expect(state.data.config.servers[0].name).toBe("new");
});
it("clears snapshot errors on recovery and ignores late failures", async () => {
  api.desktop.mockRejectedValueOnce(new Error("offline"));
  let state!: ReturnType<typeof useSnapshot>;
  function Probe() {
    state = useSnapshot(true);
    return null;
  }
  await act(async () => render(<Probe />));
  expect(state.error).toContain("offline");
  const old = deferred<Snapshot>();
  api.desktop
    .mockReturnValueOnce(old.promise)
    .mockResolvedValueOnce(snapshot("recovered"));
  await act(async () => {
    void state.refresh();
  });
  await act(async () => {
    await state.refresh();
  });
  await act(async () => {
    old.reject(new Error("obsolete error"));
  });
  expect(state.error).toBe("");
  expect(state.data.config.servers[0].name).toBe("recovered");
});
it("retries a hidden initial load for a non-polling collection on visibility", async () => {
  hidden = true;
  api.collect.mockResolvedValue([]);
  let state!: ReturnType<typeof useCollection<"services">>;
  function Probe() {
    const scope = useRef<HTMLDivElement>(null);
    state = useCollection("a", "services", false, scope);
    return <div ref={scope} />;
  }
  await act(async () => render(<Probe />));
  expect(api.collect).not.toHaveBeenCalled();
  await act(async () => {
    hidden = false;
    document.dispatchEvent(new Event("visibilitychange"));
  });
  expect(api.collect).toHaveBeenCalledTimes(1);
  expect(state.data).toEqual([]);
  await act(async () => {
    document.dispatchEvent(new Event("visibilitychange"));
  });
  expect(api.collect).toHaveBeenCalledTimes(1);
});
it("discards collection responses from an obsolete server", async () => {
  const old = deferred<[]>();
  api.collect.mockReturnValueOnce(old.promise).mockResolvedValueOnce([]);
  let state!: ReturnType<typeof useCollection<"services">>;
  function Probe({ id }: { id: string }) {
    state = useCollection(id, "services");
    return null;
  }
  await act(async () => render(<Probe id="old" />, "old"));
  await act(async () => render(<Probe id="new" />, "new"));
  await act(async () => {
    old.reject(new Error("old server"));
  });
  expect(state.error).toBe("");
  expect(state.data).toEqual([]);
});

it("does not commit unchanged snapshot polls and preserves config on runtime updates", async () => {
  const initial = snapshot("a");
  api.desktop.mockImplementation(async () => structuredClone(initial));
  let state!: ReturnType<typeof useSnapshot>;
  let commits = 0;
  function Probe() {
    state = useSnapshot(true);
    useLayoutEffect(() => {
      commits++;
    });
    return null;
  }
  await act(async () => render(<Probe />));
  const before = commits;
  const config = state.data.config;
  for (let i = 0; i < 3; i++)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
  expect(commits - before).toBe(0);
  initial.runtime.health.a = "healthy";
  await act(async () => {
    await state.refresh();
  });
  expect(state.data.runtime.health.a).toBe("healthy");
  expect(state.data.config).toBe(config);
});

it("supersedes an initial collection read when a mutation requests refresh", async () => {
  const old = deferred<[]>();
  api.collect.mockReturnValueOnce(old.promise).mockResolvedValueOnce([]);
  let state!: ReturnType<typeof useCollection<"containers">>;
  function Probe() {
    state = useCollection("a", "containers");
    return null;
  }
  await act(async () => render(<Probe />));
  await act(async () => {
    await state.refresh();
  });
  expect(api.collect).toHaveBeenCalledTimes(2);
  expect(state.data).toEqual([]);
  await act(async () => old.reject(new Error("obsolete read")));
  expect(state.error).toBe("");
});

it("retains same-endpoint data on remount but never displays it under a changed host", async () => {
  const service = {
    name: "old-host.service",
    description: "Old",
    load: "loaded",
    active: "active",
    sub: "running",
  };
  api.collect.mockResolvedValueOnce([service]);
  let state!: ReturnType<typeof useCollection<"services">>;
  function Probe() {
    state = useCollection("a", "services");
    return null;
  }
  await act(async () => render(<Probe />));
  expect(state.data).toEqual([service]);
  await act(async () => render(null));
  const pending = deferred<[]>();
  api.collect.mockReturnValue(pending.promise);
  await act(async () => render(<Probe />));
  expect(state.data).toEqual([service]);
  expect(state.busy).toBe(true);
  await act(async () => render(<Probe />, "a", "new-host"));
  expect(state.data).toBeNull();
  await act(async () => pending.resolve([]));
  expect(state.data).toEqual([]);
});

it("pauses snapshot polling while hidden and refreshes on return", async () => {
  api.desktop.mockResolvedValue(snapshot("a"));
  function Probe() {
    useSnapshot(true);
    return null;
  }
  await act(async () => render(<Probe />));
  expect(api.desktop).toHaveBeenCalledTimes(1);
  await act(async () => {
    hidden = true;
    document.dispatchEvent(new Event("visibilitychange"));
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(api.desktop).toHaveBeenCalledTimes(1);
  await act(async () => {
    hidden = false;
    document.dispatchEvent(new Event("visibilitychange"));
  });
  expect(api.desktop).toHaveBeenCalledTimes(2);
});

it("changes the collection cache when only credential revision changes", async () => {
  api.collect.mockResolvedValueOnce([{ name: "old-account.service" }]);
  let state!: ReturnType<typeof useCollection<"services">>;
  function Probe() {
    state = useCollection("a", "services");
    return null;
  }
  await act(async () => render(<Probe />));
  expect(state.data).toHaveLength(1);
  const pending = deferred<[]>();
  api.collect.mockReturnValueOnce(pending.promise);
  await act(async () => render(<Probe />, "a", "host", 1));
  expect(state.data).toBeNull();
  await act(async () => pending.resolve([]));
  expect(state.data).toEqual([]);
});

it("keeps a saved-history failure until a successful saved read, while showing live samples", async () => {
  const { useMetricHistory } = await import("./useMetricHistory");
  const reading = {
    cpu: 25,
    memoryUsed: 4,
    memoryTotal: 8,
    load: [1],
    uptime: 100,
    network: [],
  };
  api.desktop.mockRejectedValueOnce(new Error("Disk unavailable"));
  let state!: ReturnType<typeof useMetricHistory>;
  function Probe({ at }: { at?: number }) {
    state = useMetricHistory("a", at ? reading : null, at, false);
    return null;
  }
  await act(async () => render(<Probe />));
  expect(state.error).toContain("Disk unavailable");
  await act(async () => render(<Probe at={20_000} />));
  expect(state.samples.map((sample) => sample.at)).toEqual([20_000]);
  expect(state.error).toContain("Disk unavailable");
  expect(api.desktop).toHaveBeenCalledTimes(1);
  api.desktop.mockResolvedValueOnce([{ at: 10_000, data: reading }]);
  await act(async () => {
    await state.refresh();
  });
  expect(state.error).toBe("");
  expect(state.samples.map((sample) => sample.at)).toEqual([10_000, 20_000]);
});

it("isolates both saved-history errors and display samples after a destination change", async () => {
  const { useMetricHistory } = await import("./useMetricHistory");
  const reading = {
    cpu: 25,
    memoryUsed: 4,
    memoryTotal: 8,
    load: [1],
    uptime: 100,
    network: [],
  };
  api.desktop.mockRejectedValueOnce(new Error("Old disk error"));
  let state!: ReturnType<typeof useMetricHistory>;
  function Probe({ live = false }: { live?: boolean }) {
    state = useMetricHistory(
      "a",
      live ? reading : null,
      live ? 20_000 : undefined,
      false,
    );
    return null;
  }
  await act(async () => render(<Probe live />));
  expect(state.error).toContain("Old disk error");
  api.desktop.mockResolvedValueOnce([]);
  await act(async () => render(<Probe />, "a", "new-host"));
  expect(state.error).toBe("");
  expect(state.samples).toEqual([]);
});
