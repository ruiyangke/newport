import { expect, it, vi } from "vitest";
const invoke = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
import { desktop, collect } from "./desktop";
it("preserves the backend wire names and argument shapes", async () => {
  await desktop("terminal_resize", { session: "session", cols: 80, rows: 24 });
  expect(invoke).toHaveBeenLastCalledWith("terminal_resize", {
    session: "session",
    cols: 80,
    rows: 24,
  });
  await collect("server", "overview");
  expect(invoke).toHaveBeenLastCalledWith("cockpit_collect", {
    id: "server",
    section: "overview",
    refresh: false,
  });
  await desktop("container_logs", {
    id: "server",
    container: null,
    project: "atlas",
    cursor: null,
    lines: 300,
  });
  expect(invoke).toHaveBeenLastCalledWith("container_logs", {
    id: "server",
    container: null,
    project: "atlas",
    cursor: null,
    lines: 300,
  });
  await collect("server", "overview", true);
  expect(invoke).toHaveBeenLastCalledWith("cockpit_collect", {
    id: "server",
    section: "overview",
    refresh: true,
  });
});
// Compiled by tsc, never executed: invalid IPC requests must remain type errors.
function invalidRequests() {
  // @ts-expect-error a width string is not a terminal column count
  void desktop("terminal_resize", { session: "s", cols: "80", rows: 24 });
  // @ts-expect-error command names are closed
  void desktop("unknown_command");
  // @ts-expect-error containers moved to their own commands
  void collect("s", "containers");
  // @ts-expect-error a container action takes a verb, not a project action
  void desktop("container_action", { id: "s", container: "c", verb: "up" });
}
void invalidRequests;

it("scopes project bookmarks to a server and preserves opaque path bytes", async () => {
  const project = {
    id: "project",
    serverId: "server",
    name: "Example",
    path: { display: "/home/�", bytesB64: "L2hvbWUv/w==" },
  };
  await desktop("git_projects_save", { project });
  expect(invoke).toHaveBeenLastCalledWith("git_projects_save", { project });
  await desktop("git_projects_list", { serverId: "server" });
  expect(invoke).toHaveBeenLastCalledWith("git_projects_list", {
    serverId: "server",
  });
  await desktop("git_projects_remove", {
    serverId: "server",
    projectId: "project",
  });
  expect(invoke).toHaveBeenLastCalledWith("git_projects_remove", {
    serverId: "server",
    projectId: "project",
  });
});
