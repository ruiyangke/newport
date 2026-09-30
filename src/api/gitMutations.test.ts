import { beforeEach, expect, it, vi } from "vitest";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { GitMutations } from "./gitMutations";
import type { GitOperation } from "../domain/gitResponses";

const params = {
  operationId: "operation",
  repoId: "repo",
  expectedSnapshot: "snapshot",
  action: { kind: "stage" as const, entryIds: ["entry"] },
};
const result: GitOperation = {
  operationId: "operation",
  repository: "repository",
  payloadHash: "hash",
  seq: 1,
  state: "succeeded",
  result: {},
  error: null,
};
beforeEach(() => {
  invoke.mockReset().mockResolvedValue([]);
});
function setup() {
  const client = {
    start: vi.fn().mockResolvedValue(result),
    bootstrap: vi.fn().mockResolvedValue(result),
    operation: vi.fn().mockResolvedValue(result),
  };
  const guard = vi.fn();
  return {
    client,
    guard,
    mutations: new GitMutations("server", () => client, guard),
  };
}

it("checks persisted receipts before sending a write and preserves its exact ID", async () => {
  const { client, mutations } = setup();
  await expect(mutations.start(params)).resolves.toBe(result);
  expect(invoke).toHaveBeenCalledWith("git_pending_operations", {
    serverId: "server",
  });
  expect(client.start).toHaveBeenCalledExactlyOnceWith(params);
  expect(
    invoke.mock.calls.some(
      ([command]) => command === "git_acknowledge_operation",
    ),
  ).toBe(false);
});

it.each(["pending", "outcome_unknown"])(
  "lets the agent enforce repository-scoped recovery when a receipt is %s",
  async (state) => {
    invoke.mockResolvedValue([
      { operationId: "older", serverId: "server", action: "commit", state },
    ]);
    const { mutations, client } = setup();
    await mutations.start(params);
    expect(client.start).toHaveBeenCalledExactlyOnceWith(params);
    expect(
      invoke.mock.calls.some(
        ([command]) =>
          command === "git_acknowledge_operation" ||
          command === "git_review_operation",
      ),
    ).toBe(false);
  },
);

it("does not let a storage read failure bypass recovery", async () => {
  invoke.mockRejectedValue(new Error("Recovery log unreadable"));
  const { mutations, client } = setup();
  await expect(mutations.start(params)).rejects.toThrow("unreadable");
  expect(client.start).not.toHaveBeenCalled();
});

it("checks outcomes without replaying a write or dismissing a receipt", async () => {
  const { mutations, client } = setup();
  await expect(mutations.check("older")).resolves.toBe(result);
  expect(client.operation).toHaveBeenCalledExactlyOnceWith("older");
  expect(client.start).not.toHaveBeenCalled();
  expect(invoke).not.toHaveBeenCalled();
  await mutations.acknowledge("older");
  expect(invoke).toHaveBeenCalledWith("git_acknowledge_operation", {
    serverId: "server",
    operationId: "older",
  });
});

it("prevents overlapping writes and captures the request before waiting for storage", async () => {
  let finish!: (receipts: unknown[]) => void;
  invoke.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const { mutations, client } = setup();
  const input = structuredClone(params);
  const first = mutations.start(input);
  input.action.entryIds.push("later edit");
  await expect(mutations.start(params)).rejects.toThrow("still running");
  finish([]);
  await first;
  expect(client.start).toHaveBeenCalledExactlyOnceWith(params);
});

it("rechecks the server scope after an asynchronous storage read", async () => {
  const { mutations, client, guard } = setup();
  let calls = 0;
  guard.mockImplementation(() => {
    if (++calls >= 3) throw new Error("Workspace closed");
  });
  await expect(mutations.start(params)).rejects.toThrow("closed");
  expect(client.start).not.toHaveBeenCalled();
});

it("releases its in-flight guard after failure without automatically retrying", async () => {
  const { mutations, client } = setup();
  client.start.mockRejectedValueOnce(new Error("Uncertain outcome"));
  await expect(mutations.start(params)).rejects.toThrow("Uncertain outcome");
  expect(client.start).toHaveBeenCalledOnce();
  await expect(mutations.check("operation")).resolves.toBe(result);
});

it("allows an unrelated repository creation while preserving saved outcomes", async () => {
  invoke.mockResolvedValue([
    {
      operationId: "older",
      serverId: "server",
      action: "repo.clone",
      state: "pending",
    },
  ]);
  const { client, mutations } = setup();
  await expect(
    mutations.bootstrap({
      method: "repo.init",
      params: {
        operationId: "new",
        path: { display: "/new", bytesB64: "L25ldw==" },
        initialBranch: "main",
      },
    }),
  ).resolves.toBeDefined();
  expect(client.bootstrap).toHaveBeenCalledTimes(1);
});
it("dispatches cloning once without acknowledging the saved outcome", async () => {
  const { client, mutations } = setup();
  const request = {
    method: "repo.clone" as const,
    params: {
      operationId: "new",
      path: { display: "/new", bytesB64: "L25ldw==" },
      url: "ssh://host/repo",
      branch: "main",
      bare: false,
    },
  };
  await mutations.bootstrap(request);
  expect(client.bootstrap).toHaveBeenCalledExactlyOnceWith(request);
  expect(invoke.mock.calls.map(([command]) => command)).toEqual([
    "git_pending_operations",
  ]);
});

it("permits a corrected creation after rejection without dismissing or replaying the earlier attempt", async () => {
  invoke.mockResolvedValue([
    {
      operationId: "rejected-id",
      serverId: "server",
      action: "repo.init",
      state: "rejected",
    },
  ]);
  const { mutations, client } = setup();
  const request = {
    method: "repo.init" as const,
    params: {
      operationId: "new-id",
      path: { display: "/repo", bytesB64: btoa("/repo") },
      initialBranch: "main",
    },
  };
  await mutations.bootstrap(request);
  expect(client.bootstrap).toHaveBeenCalledExactlyOnceWith(request);
  expect(client.operation).not.toHaveBeenCalled();
  expect(
    invoke.mock.calls.some(
      ([command]) => command === "git_acknowledge_operation",
    ),
  ).toBe(false);
});

it("records explicit review without replaying or dismissing the original operation", async () => {
  const { mutations, client } = setup();
  await mutations.review("interrupted");
  expect(invoke).toHaveBeenCalledWith("git_review_operation", {
    serverId: "server",
    operationId: "interrupted",
  });
  expect(client.start).not.toHaveBeenCalled();
  expect(client.bootstrap).not.toHaveBeenCalled();
  expect(
    invoke.mock.calls.some(
      ([method]) => method === "git_acknowledge_operation",
    ),
  ).toBe(false);
});

it("permits a separately requested write after explicit review", async () => {
  const { mutations, client } = setup();
  invoke.mockResolvedValue([
    {
      operationId: "old",
      serverId: "server",
      action: "pull.fast_forward",
      state: "reviewed_unknown",
    },
  ]);
  await mutations.start(params);
  expect(client.start).toHaveBeenCalledExactlyOnceWith(params);
  expect(client.operation).not.toHaveBeenCalled();
});

it("surfaces repository recovery refusal without reviewing or replaying it", async () => {
  const { mutations, client } = setup();
  client.start.mockRejectedValueOnce(
    new Error("RECOVERY_REQUIRED: inspect interrupted operation"),
  );
  await expect(mutations.start(params)).rejects.toThrow("RECOVERY_REQUIRED");
  expect(client.start).toHaveBeenCalledTimes(1);
  expect(
    invoke.mock.calls.some(
      ([command]) =>
        command === "git_acknowledge_operation" ||
        command === "git_review_operation",
    ),
  ).toBe(false);
});
