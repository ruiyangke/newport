import { afterEach, expect, it, vi } from "vitest";
import { onlineManager } from "@tanstack/react-query";
import { createQueryClient, readIPC, refreshQuery } from "./client";
import { keys, serverScope } from "./keys";
import { fileListingOptions } from "./files";
import type { Server } from "../types";
const api = vi.hoisted(() => ({ desktop: vi.fn() }));
vi.mock("../api/desktop", () => api);
const server: Server = {
  id: "a",
  name: "Name",
  sshHost: "host",
  sshUser: "user",
  sshPort: 22,
  authMethod: "publicKey",
  identityFile: null,
};
afterEach(() => {
  onlineManager.setOnline(true);
  api.desktop.mockReset();
});
it("keeps metadata identity, isolates endpoints, and invalidates credentials without secrets", () => {
  const initial = serverScope(server);
  expect(
    keys.collection(
      serverScope({ ...server, name: "New", clipboardEnabled: true }),
      "services",
    ),
  ).toEqual(keys.collection(initial, "services"));
  expect(
    keys.history(serverScope({ ...server, sshHost: "other" })),
  ).not.toEqual(keys.history(initial));
  expect(keys.ports(serverScope(server, 1))).not.toEqual(keys.ports(initial));
  expect(keys.history(serverScope(server, 1))).toEqual(keys.history(initial));
});
it("executes local IPC while the browser reports offline and never retries failures", async () => {
  const client = createQueryClient();
  onlineManager.setOnline(false);
  try {
    expect(
      await client.fetchQuery({ queryKey: ["local"], queryFn: async () => 42 }),
    ).toBe(42);
    const read = vi.fn().mockRejectedValue(new Error("Authentication failed"));
    await expect(
      client.fetchQuery({ queryKey: ["error"], queryFn: read }),
    ).rejects.toThrow("Authentication failed");
    expect(read).toHaveBeenCalledTimes(1);
  } finally {
    client.clear();
  }
});
it("a late initial reply cannot overwrite an explicit refresh", async () => {
  const client = createQueryClient();
  let resolve!: (value: string) => void;
  const old = new Promise<string>((yes) => {
    resolve = yes;
  });
  const read = vi.fn().mockReturnValueOnce(old).mockResolvedValueOnce("fresh");
  const options = {
    queryKey: ["read"],
    queryFn: ({ signal }: { signal: AbortSignal }) => readIPC(signal, read),
  };
  try {
    const first = client.fetchQuery(options).catch(() => undefined);
    expect(await refreshQuery(client, options)).toBe("fresh");
    resolve("old");
    await first;
    expect(client.getQueryData(["read"])).toBe("fresh");
  } finally {
    client.clear();
  }
});
it("forwards file-list cancellation to Rust and discards the late listing", async () => {
  const client = createQueryClient();
  let resolve!: (value: unknown) => void;
  api.desktop.mockImplementation((command) =>
    command === "files_list"
      ? new Promise((yes) => {
          resolve = yes;
        })
      : Promise.resolve(),
  );
  const options = fileListingOptions(serverScope(server), "/home");
  try {
    const pending = client.fetchQuery(options).catch(() => undefined);
    const operation = api.desktop.mock.calls[0][1].operation;
    await client.cancelQueries({ queryKey: options.queryKey });
    expect(api.desktop).toHaveBeenCalledWith("files_cancel", { operation });
    resolve({ path: "/home", entries: [] });
    await pending;
    expect(client.getQueryData(options.queryKey)).toBeUndefined();
  } finally {
    client.clear();
  }
});
