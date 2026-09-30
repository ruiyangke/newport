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

it("serializes reads and continues after a repository error without retrying it", async () => {
  const first = deferred<unknown>();
  invoke.mockImplementation((command: string) => {
    if (command === "git_connect") return Promise.resolve(connection);
    if (command === "git_request") return first.promise;
    return Promise.resolve();
  });
  const session = new GitSession("server");
  const a = session.request(request);
  const rejected = expect(a).rejects.toEqual({ code: "STALE_SNAPSHOT" });
  const b = session.request(request);
  await vi.waitFor(() =>
    expect(
      invoke.mock.calls.filter(([name]) => name === "git_request"),
    ).toHaveLength(1),
  );
  invoke.mockImplementation(async () => "second");
  first.reject({ code: "STALE_SNAPSHOT" });
  await rejected;
  await expect(b).resolves.toBe("second");
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
  const second = session.request(request);
  await expect(first).rejects.toMatchObject({ code: "TRANSPORT_ERROR" });
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
