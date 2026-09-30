import { beforeEach, expect, it, vi } from "vitest";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { GitSession } from "./gitSession";
import { gitPath } from "../domain/git";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const connection = {
  serverId: "server",
  info: { capabilities: { methods: ["repo.open", "repo.status"] } },
};
const request = { method: "repo.status" as const, params: { repoId: "repo" } };
beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (command: string) =>
    command === "git_connect" ? connection : undefined,
  );
});

it("requires advertised server-side branch filtering instead of silently searching a partial list", async () => {
  invoke.mockResolvedValue({
    serverId: "server",
    info: { capabilities: { methods: ["repo.branches"], features: [] } },
  });
  const session = new GitSession("server");
  await expect(
    session.request({
      method: "repo.branches",
      params: { repoId: "repo", filter: "late-branch" },
    }),
  ).rejects.toThrow("Update the server agent");
  expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
    false,
  );
  await session.dispose();
});

it("overlaps reads and isolates a repository error without retrying it", async () => {
  const first = deferred<unknown>();
  let requests = 0;
  invoke.mockImplementation((command: string) => {
    if (command === "git_connect") return Promise.resolve(connection);
    if (command === "git_request")
      return ++requests === 1 ? first.promise : Promise.resolve("second");
    return Promise.resolve();
  });
  const session = new GitSession("server");
  const a = session.request(request);
  const rejected = expect(a).rejects.toEqual({ code: "STALE_SNAPSHOT" });
  const b = session.request(request);
  await vi.waitFor(() =>
    expect(
      invoke.mock.calls.filter(([name]) => name === "git_request"),
    ).toHaveLength(2),
  );
  await expect(b).resolves.toBe("second");
  first.reject({ code: "STALE_SNAPSHOT" });
  await rejected;
  expect(
    invoke.mock.calls.filter(([name]) => name === "git_request"),
  ).toHaveLength(2);
  await session.dispose();
});

it("disconnects exactly once and never dispatches queued reads", async () => {
  const opening = deferred<typeof connection>();
  invoke.mockImplementation((command: string) =>
    command === "git_connect" ? opening.promise : Promise.resolve(),
  );
  const session = new GitSession("server");
  const pending = session.request(request);
  const rejected = expect(pending).rejects.toThrow("closed");
  const disposal = session.dispose();
  expect(session.dispose()).toBe(disposal);
  opening.resolve(connection);
  await disposal;
  await rejected;
  // Disposed before the queued read ran, so it neither connected nor sent.
  expect(invoke.mock.calls.map(([name]) => name)).toEqual(["git_disconnect"]);
});

it("disconnects without waiting for an active read and suppresses its late result", async () => {
  const active = deferred<unknown>();
  invoke.mockImplementation((command: string) =>
    command === "git_connect"
      ? Promise.resolve(connection)
      : command === "git_request"
        ? active.promise
        : Promise.resolve(),
  );
  const session = new GitSession("server");
  const pending = session.request(request);
  const rejected = expect(pending).rejects.toThrow("closed");
  await vi.waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("git_request", expect.anything()),
  );
  await session.dispose();
  active.resolve("stale server data");
  await rejected;
});

it("a broken transport fails only its own request; the next goes out on a fresh connection", async () => {
  // The agent is stateless and the native side reconnects before sending, so
  // a lost connection no longer ends the session -- but the request it broke
  // is reported, never re-sent.
  let sent = 0;
  invoke.mockImplementation(async (command: string) => {
    if (command === "git_connect") return connection;
    if (command === "git_request") {
      sent += 1;
      if (sent === 1) throw { code: "TRANSPORT_ERROR", message: "lost" };
      return "fresh";
    }
  });
  const session = new GitSession("server");
  const first = session.request(request);
  await expect(first).rejects.toMatchObject({ code: "TRANSPORT_ERROR" });
  const second = session.request(request);
  await expect(second).resolves.toBe("fresh");
  const names = invoke.mock.calls.map(([name]) => name);
  // Let go of the broken connection, handshook again, then sent the next one.
  expect(names.filter((n) => n === "git_disconnect")).toHaveLength(1);
  expect(names.filter((n) => n === "git_connect")).toHaveLength(2);
  expect(names.filter((n) => n === "git_request")).toHaveLength(2);
  await session.dispose();
});

it("releases failed setup cleanly and rejects unsupported methods before dispatch", async () => {
  invoke.mockRejectedValueOnce(new Error("Agent unavailable"));
  const failed = new GitSession("server");
  await expect(failed.request(request)).rejects.toThrow("Agent unavailable");
  await expect(failed.dispose()).resolves.toBeUndefined();
  const session = new GitSession("server");
  await expect(
    session.request({ method: "repo.tags", params: { repoId: "repo" } }),
  ).rejects.toThrow("Update the server agent");
  expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
    false,
  );
  await session.dispose();
});

it("captures lossless paths before waiting for a connection", async () => {
  const opening = deferred<typeof connection>();
  invoke.mockImplementation((command: string) =>
    command === "git_connect" ? opening.promise : Promise.resolve(null),
  );
  const session = new GitSession("server");
  const path = { display: "replacement character", bytesB64: "/w==" };
  const pending = session.request({ method: "repo.open", params: { path } });
  path.bytesB64 = "changed";
  opening.resolve(connection);
  await pending;
  expect(invoke).toHaveBeenCalledWith("git_request", {
    serverId: "server",
    request: {
      method: "repo.open",
      params: { path: { display: "replacement character", bytesB64: "/w==" } },
    },
  });
  await session.dispose();
});

it("encodes entered Unicode paths as UTF-8 and rejects invalid path sizes", () => {
  const path = gitPath("/home/工作/app");
  expect(
    new TextDecoder().decode(
      Uint8Array.from(atob(path.bytesB64), (c) => c.charCodeAt(0)),
    ),
  ).toBe(path.display);
  expect(() => gitPath("a\0b")).toThrow();
  expect(() => gitPath("é".repeat(2049))).toThrow();
});

const write = {
  method: "operation.start" as const,
  params: {
    operationId: "operation",
    repoId: "repo",
    expectedSnapshot: "snapshot",
    action: { kind: "stage" as const, entryIds: ["file"] },
  },
};
const writableConnection = {
  ...connection,
  info: {
    capabilities: {
      methods: ["operation.start", "repo.status"],
      actions: ["stage"],
    },
  },
};

it("does not dispatch an aborted read queued behind a write", async () => {
  const writing = deferred<unknown>();
  invoke.mockImplementation((command, args) => {
    if (command === "git_connect") return Promise.resolve(writableConnection);
    if (command === "git_request" && args.request.method === "operation.start")
      return writing.promise;
    return Promise.resolve();
  });
  const session = new GitSession("server");
  const mutation = session.request(write);
  await vi.waitFor(() =>
    expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
      true,
    ),
  );
  const controller = new AbortController();
  const pending = session.request(request, controller.signal);
  const rejected = expect(pending).rejects.toMatchObject({
    name: "AbortError",
  });
  controller.abort();
  writing.resolve("saved");
  await mutation;
  await rejected;
  expect(
    invoke.mock.calls.filter(([name]) => name === "git_request"),
  ).toHaveLength(1);
  await session.dispose();
});

it("does not dispatch an aborted read after connection setup", async () => {
  const opening = deferred<typeof connection>();
  invoke.mockImplementation((command) =>
    command === "git_connect" ? opening.promise : Promise.resolve(),
  );
  const session = new GitSession("server");
  const controller = new AbortController();
  const pending = session.request(request, controller.signal);
  const rejected = expect(pending).rejects.toMatchObject({
    name: "AbortError",
  });
  await vi.waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("git_connect", { serverId: "server" }),
  );
  controller.abort();
  opening.resolve(connection);
  await rejected;
  expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
    false,
  );
  await session.dispose();
});

it("does not release the write barrier when an already dispatched read is aborted", async () => {
  const reading = deferred<unknown>();
  invoke.mockImplementation((command, args) => {
    if (command === "git_connect") return Promise.resolve(writableConnection);
    if (command === "git_request" && args.request.method === "repo.status")
      return reading.promise;
    return Promise.resolve("saved");
  });
  const session = new GitSession("server");
  const controller = new AbortController();
  const pending = session.request(request, controller.signal);
  const rejected = expect(pending).rejects.toMatchObject({
    name: "AbortError",
  });
  await vi.waitFor(() =>
    expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
      true,
    ),
  );
  controller.abort();
  const mutation = session.request(write);
  await Promise.resolve();
  expect(
    invoke.mock.calls.filter(([name]) => name === "git_request"),
  ).toHaveLength(1);
  reading.resolve("obsolete");
  await rejected;
  await expect(mutation).resolves.toBe("saved");
  expect(
    invoke.mock.calls.filter(([name]) => name === "git_request"),
  ).toHaveLength(2);
  await session.dispose();
});

it("refuses read cancellation signals on writes before dispatch", async () => {
  const session = new GitSession("server");
  await expect(
    session.request(write, new AbortController().signal),
  ).rejects.toThrow("Write requests");
  expect(invoke).not.toHaveBeenCalled();
  await session.dispose();
});

it("waits for earlier reads before a write and gates later reads until it settles", async () => {
  const read = deferred<unknown>();
  const writing = deferred<unknown>();
  let reads = 0;
  invoke.mockImplementation((command, args) => {
    if (command === "git_connect") return Promise.resolve(writableConnection);
    if (command === "git_request") {
      if (args.request.method === "operation.start") return writing.promise;
      return ++reads === 1 ? read.promise : Promise.resolve("later");
    }
    return Promise.resolve();
  });
  const session = new GitSession("server");
  const first = session.request(request);
  const mutation = session.request(write);
  const later = session.request(request);
  await vi.waitFor(() => expect(reads).toBe(1));
  expect(
    invoke.mock.calls.filter(([name]) => name === "git_request"),
  ).toHaveLength(1);
  read.resolve("earlier");
  await first;
  await vi.waitFor(() =>
    expect(
      invoke.mock.calls.filter(([name]) => name === "git_request"),
    ).toHaveLength(2),
  );
  expect(reads).toBe(1);
  writing.resolve("saved");
  await expect(mutation).resolves.toBe("saved");
  await expect(later).resolves.toBe("later");
  await session.dispose();
});

it("coalesces parallel transport failures and waits for disconnect before reconnecting", async () => {
  const failure = deferred<unknown>();
  const disconnect = deferred<void>();
  let fresh = false;
  invoke.mockImplementation((command) => {
    if (command === "git_connect") return Promise.resolve(connection);
    if (command === "git_disconnect") return disconnect.promise;
    return fresh ? Promise.resolve("fresh") : failure.promise;
  });
  const session = new GitSession("server");
  const a = expect(session.request(request)).rejects.toMatchObject({
    code: "TRANSPORT_ERROR",
  });
  const b = expect(session.request(request)).rejects.toMatchObject({
    code: "TRANSPORT_ERROR",
  });
  await vi.waitFor(() =>
    expect(
      invoke.mock.calls.filter(([name]) => name === "git_request"),
    ).toHaveLength(2),
  );
  failure.reject({ code: "TRANSPORT_ERROR" });
  await Promise.all([a, b]);
  const c = session.request(request);
  await Promise.resolve();
  expect(
    invoke.mock.calls.filter(([name]) => name === "git_disconnect"),
  ).toHaveLength(1);
  expect(
    invoke.mock.calls.filter(([name]) => name === "git_connect"),
  ).toHaveLength(1);
  fresh = true;
  disconnect.resolve();
  await expect(c).resolves.toBe("fresh");
  expect(
    invoke.mock.calls.filter(([name]) => name === "git_connect"),
  ).toHaveLength(2);
  await session.dispose();
});

it("refuses unsupported write actions before dispatch", async () => {
  invoke.mockResolvedValue({
    ...writableConnection,
    info: { capabilities: { methods: ["operation.start"], actions: [] } },
  });
  const session = new GitSession("server");
  await expect(session.request(write)).rejects.toThrow("does not support");
  expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
    false,
  );
  await session.dispose();
});

it("keeps the operation ID when a dispatched write loses its transport and never retries", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "git_connect") return writableConnection;
    if (command === "git_request")
      throw { code: "TRANSPORT_ERROR", message: "Connection lost" };
  });
  const session = new GitSession("server");
  await expect(session.request(write)).rejects.toMatchObject({
    operationId: "operation",
    code: "OUTCOME_UNKNOWN",
  });
  // Never re-sent on its own: one dispatch, and the connection let go.
  expect(
    invoke.mock.calls.filter(([name]) => name === "git_request"),
  ).toHaveLength(1);
  expect(
    invoke.mock.calls.filter(([name]) => name === "git_disconnect"),
  ).toHaveLength(1);
  await session.dispose();
});

it("marks a write interrupted by disposal as uncertain even if its reply arrives later", async () => {
  const pending = deferred<unknown>();
  invoke.mockImplementation((command: string) =>
    command === "git_connect"
      ? Promise.resolve(writableConnection)
      : command === "git_request"
        ? pending.promise
        : Promise.resolve(),
  );
  const session = new GitSession("server");
  const result = session.request(write);
  const rejected = expect(result).rejects.toMatchObject({
    operationId: "operation",
    code: "OUTCOME_UNKNOWN",
  });
  await vi.waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("git_request", expect.anything()),
  );
  await session.dispose();
  pending.resolve({ state: "succeeded" });
  await rejected;
});

it.each(["repo.init", "repo.clone"] as const)(
  "retains the %s ID after a lost reply without retrying",
  async (method) => {
    invoke.mockImplementation(async (command) => {
      if (command === "git_connect")
        return { ...connection, info: { capabilities: { methods: [method] } } };
      if (command === "git_request")
        throw { code: "TRANSPORT_ERROR", message: "Lost reply" };
    });
    const session = new GitSession("server");
    const params = { operationId: "bootstrap-id", path: gitPath("/srv/new") };
    const request =
      method === "repo.init"
        ? { method, params: { ...params, initialBranch: "main" } }
        : { method, params: { ...params, url: "ssh://host/repo" } };
    await expect(session.request(request)).rejects.toMatchObject({
      operationId: "bootstrap-id",
      code: "OUTCOME_UNKNOWN",
    });
    // Never re-sent on its own.
    expect(
      invoke.mock.calls.filter(([cmd]) => cmd === "git_request"),
    ).toHaveLength(1);
  },
);

it.each([false, true])(
  "exact stash selection requires advertised support: %s",
  async (supported) => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "git_connect")
        return {
          ...connection,
          info: {
            capabilities: {
              methods: ["operation.start"],
              actions: ["stash.drop"],
              features: supported ? ["stash.entry_index"] : [],
            },
          },
        };
      if (command === "git_request") return { state: "succeeded" };
    });
    const session = new GitSession("server");
    const request = {
      ...write,
      params: {
        ...write.params,
        action: {
          kind: "stash.drop" as const,
          oid: "a".repeat(40),
          expectedToken: "list",
          index: 1,
        },
      },
    };
    if (supported)
      await expect(session.request(request)).resolves.toEqual({
        state: "succeeded",
      });
    else
      await expect(session.request(request)).rejects.toThrow(
        "Update the server agent",
      );
    expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
      supported,
    );
    await session.dispose();
  },
);

it.each([false, true])(
  "partial staging requires advertised support: %s",
  async (supported) => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "git_connect")
        return {
          ...connection,
          info: {
            capabilities: {
              methods: ["operation.start"],
              actions: ["stage"],
              features: supported ? ["index.hunks"] : [],
            },
          },
        };
      if (command === "git_request") return { state: "succeeded" };
    });
    const session = new GitSession("server");
    const request = {
      ...write,
      params: {
        ...write.params,
        action: {
          kind: "stage" as const,
          entryIds: ["file"],
          hunks: { ids: ["a".repeat(64)], contextLines: 3 },
        },
      },
    };
    if (supported)
      await expect(session.request(request)).resolves.toEqual({
        state: "succeeded",
      });
    else
      await expect(session.request(request)).rejects.toThrow(
        "Update the server agent",
      );
    expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
      supported,
    );
    await session.dispose();
  },
);

it.each([false, true])(
  "a worktree on a new branch requires advertised support: %s",
  async (supported) => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "git_connect")
        return {
          ...connection,
          info: {
            capabilities: {
              methods: ["operation.start"],
              actions: ["worktree.add"],
              features: supported ? ["worktree.new_branch"] : [],
            },
          },
        };
      if (command === "git_request") return { state: "succeeded" };
    });
    const session = new GitSession("server");
    const request = {
      ...write,
      params: {
        ...write.params,
        action: {
          kind: "worktree.add" as const,
          name: "agent-fix",
          path: {
            display: "/srv/app-agent-fix",
            bytesB64: btoa("/srv/app-agent-fix"),
          },
          branch: "agent/fix",
          expectedOid: "a".repeat(40),
          newBranch: true,
        },
      },
    };
    if (supported)
      await expect(session.request(request)).resolves.toEqual({
        state: "succeeded",
      });
    else
      // An older agent would refuse the unknown field anyway; saying why,
      // before anything is sent, is the difference.
      await expect(session.request(request)).rejects.toThrow(
        "Update the server agent to create a worktree on a new branch.",
      );
    expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
      supported,
    );
    await session.dispose();
  },
);

it.each([true, false])(
  "requests short history messages only when advertised: %s",
  async (supported) => {
    invoke.mockImplementation(async (command: string) =>
      command === "git_connect"
        ? {
            serverId: "server",
            info: {
              capabilities: {
                methods: ["repo.history"],
                features: supported ? ["history.summary"] : [],
              },
            },
          }
        : undefined,
    );
    const session = new GitSession("server");
    const request = {
      method: "repo.history" as const,
      params: { repoId: "repo", cursor: "next", messageBytes: 512 },
    };
    await session.request(request);
    expect(invoke).toHaveBeenCalledWith("git_request", {
      serverId: "server",
      request: {
        method: "repo.history",
        params: {
          repoId: "repo",
          cursor: "next",
          ...(supported ? { messageBytes: 512 } : {}),
        },
      },
    });
    expect(request.params.messageBytes).toBe(512);
    await session.dispose();
  },
);

it.each([true, false])(
  "requests short tag annotations only when advertised: %s",
  async (supported) => {
    invoke.mockImplementation(async (command: string) =>
      command === "git_connect"
        ? {
            serverId: "server",
            info: {
              capabilities: {
                methods: ["repo.tags"],
                features: supported ? ["tags.summary"] : [],
              },
            },
          }
        : undefined,
    );
    const session = new GitSession("server");
    await session.request({
      method: "repo.tags",
      params: { repoId: "repo", cursor: "next", messageBytes: 512 },
    });
    expect(invoke).toHaveBeenCalledWith("git_request", {
      serverId: "server",
      request: {
        method: "repo.tags",
        params: {
          repoId: "repo",
          cursor: "next",
          ...(supported ? { messageBytes: 512 } : {}),
        },
      },
    });
    await session.dispose();
  },
);

it("refuses worktree filters unless the agent advertises exact filtering", async () => {
  invoke.mockResolvedValue({
    serverId: "server",
    info: { capabilities: { methods: ["repo.worktrees"], features: [] } },
  });
  const session = new GitSession("server");
  for (const options of [
    { filter: "late" },
    { branch: "refs/heads/late" },
    { name: "late" },
  ]) {
    await expect(
      session.request({
        method: "repo.worktrees",
        params: { repoId: "repo", ...options },
      }),
    ).rejects.toThrow("Update the server agent");
  }
  expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
    false,
  );
  await session.dispose();
});

it("does not silently ignore an unsupported worktree snapshot", async () => {
  invoke.mockResolvedValue({
    serverId: "server",
    info: {
      capabilities: {
        methods: ["repo.worktrees"],
        features: ["worktrees.filter"],
      },
    },
  });
  const session = new GitSession("server");
  await expect(
    session.request({
      method: "repo.worktrees",
      params: { repoId: "repo", filter: "late", atSnapshot: "capture" },
    }),
  ).rejects.toThrow(
    "Update the server agent to search a captured worktree listing",
  );
  expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
    false,
  );
  await session.dispose();
});

it("negotiates compact diff rows without changing requests for older agents", async () => {
  for (const compact of [false, true]) {
    invoke.mockReset();
    invoke.mockImplementation(async (command: string) =>
      command === "git_connect"
        ? {
            serverId: "server",
            info: {
              capabilities: {
                methods: ["repo.diff_page"],
                features: compact ? ["diff.tuple_v1"] : [],
              },
            },
          }
        : null,
    );
    const session = new GitSession("server");
    const request = {
      method: "repo.diff_page" as const,
      params: {
        repoId: "repo",
        snapshot: "status",
        entryId: "entry",
        side: "index_to_worktree" as const,
        lineEncoding: "tuple_v1" as const,
      },
    };
    await session.request(request);
    const sent = invoke.mock.calls.find(([name]) => name === "git_request")![1]
      .request;
    expect(sent.params.lineEncoding).toBe(compact ? "tuple_v1" : undefined);
    expect(request.params.lineEncoding).toBe("tuple_v1");
    await session.dispose();
  }
});

it.each([false, true])(
  "negotiates changed-file filters (supported=%s) without mutating the request",
  async (supported) => {
    invoke.mockImplementation(async (command: string) =>
      command === "git_connect"
        ? {
            ...connection,
            info: {
              capabilities: {
                methods: ["repo.status"],
                features: supported ? ["status.filter"] : [],
              },
            },
          }
        : {},
    );
    const session = new GitSession("server");
    const filtered = {
      method: "repo.status" as const,
      params: {
        repoId: "repo",
        cursor: "next",
        filter: { text: "late-file", group: "untracked" as const },
      },
    };
    await session.request(filtered);
    const sent = invoke.mock.calls.find(([name]) => name === "git_request")![1]
      .request;
    expect(sent.params.filter).toEqual(
      supported ? filtered.params.filter : undefined,
    );
    expect(filtered.params.filter.text).toBe("late-file");
    expect(sent.params.cursor).toBe("next");
    await session.dispose();
  },
);

it("cancels a native read and releases the write barrier only after native cleanup", async () => {
  const reading = deferred<unknown>();
  invoke.mockImplementation((command, args) => {
    if (command === "git_connect") return Promise.resolve(writableConnection);
    if (command === "git_register_read") return Promise.resolve("read-token");
    if (command === "git_request" && args.request.method === "repo.status")
      return reading.promise;
    return Promise.resolve("saved");
  });
  const session = new GitSession("server");
  const controller = new AbortController();
  const pending = session.request(request, controller.signal);
  const rejected = expect(pending).rejects.toMatchObject({
    name: "AbortError",
  });
  await vi.waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("git_request", {
      serverId: "server",
      request,
      readId: "read-token",
    }),
  );
  controller.abort();
  expect(invoke).toHaveBeenCalledWith("git_cancel_read", {
    serverId: "server",
    readId: "read-token",
  });
  const mutation = session.request(write);
  await Promise.resolve();
  expect(
    invoke.mock.calls.filter(([name]) => name === "git_request"),
  ).toHaveLength(1);
  reading.reject({ code: "READ_CANCELLED" });
  await rejected;
  await expect(mutation).resolves.toBe("saved");
  expect(invoke.mock.calls.some(([name]) => name === "git_disconnect")).toBe(
    false,
  );
  await session.dispose();
});
it("cancels registration if abort wins before native dispatch", async () => {
  const registering = deferred<string>();
  invoke.mockImplementation((command) =>
    command === "git_connect"
      ? Promise.resolve(connection)
      : command === "git_register_read"
        ? registering.promise
        : Promise.resolve(),
  );
  const session = new GitSession("server");
  const controller = new AbortController();
  const pending = session.request(request, controller.signal);
  const rejected = expect(pending).rejects.toMatchObject({
    name: "AbortError",
  });
  await vi.waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("git_register_read", {
      serverId: "server",
    }),
  );
  controller.abort();
  registering.resolve("early-token");
  await rejected;
  expect(invoke).toHaveBeenCalledWith("git_cancel_read", {
    serverId: "server",
    readId: "early-token",
  });
  expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
    false,
  );
  await session.dispose();
});

it("does not disconnect parallel reads or replay a failed read channel", async () => {
  const healthy = deferred<unknown>();
  let sent = 0;
  invoke.mockImplementation((command) => {
    if (command === "git_connect") return Promise.resolve(connection);
    if (command === "git_request") {
      sent++;
      if (sent === 1)
        return Promise.reject({
          code: "READ_CHANNEL_ERROR",
          message: "Read interrupted",
        });
      return sent === 2 ? healthy.promise : Promise.resolve("fresh channel");
    }
    return Promise.resolve();
  });
  const session = new GitSession("server");
  const failed = expect(session.request(request)).rejects.toMatchObject({
    code: "READ_CHANNEL_ERROR",
  });
  const other = session.request(request);
  await failed;
  expect(invoke.mock.calls.some(([name]) => name === "git_disconnect")).toBe(
    false,
  );
  healthy.resolve("uninterrupted");
  await expect(other).resolves.toBe("uninterrupted");
  await expect(session.request(request)).resolves.toBe("fresh channel");
  expect(sent).toBe(3);
  expect(
    invoke.mock.calls.filter(([name]) => name === "git_connect"),
  ).toHaveLength(1);
  await session.dispose();
});

it("requires remote search support but preserves unfiltered older agents", async () => {
  invoke.mockImplementation(async (command: string) =>
    command === "git_connect"
      ? {
          serverId: "server",
          info: {
            capabilities: { methods: ["repo.remote_refs"], features: [] },
          },
        }
      : undefined,
  );
  const session = new GitSession("server");
  const params = { repoId: "repo", remote: "origin", expectedToken: "token" };
  await expect(
    session.request({
      method: "repo.remote_refs",
      params: { ...params, filter: "late" },
    }),
  ).rejects.toThrow("Update the server agent");
  expect(invoke.mock.calls.some(([name]) => name === "git_request")).toBe(
    false,
  );
  await session.request({
    method: "repo.remote_refs",
    params: { ...params, filter: "" },
  });
  expect(invoke).toHaveBeenCalledWith("git_request", {
    serverId: "server",
    request: { method: "repo.remote_refs", params },
  });
  await session.dispose();
});

it("requires path-summary capability before sending a path to an older agent", async () => {
  invoke.mockResolvedValue({
    serverId: "server",
    info: { capabilities: { methods: ["repo.status_summary"], features: [] } },
  });
  const session = new GitSession("server");
  await expect(
    session.request({
      method: "repo.status_summary",
      params: { path: gitPath("/repo") },
    }),
  ).rejects.toThrow("Update the server agent");
  expect(invoke.mock.calls.some(([command]) => command === "git_request")).toBe(
    false,
  );
});
