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
  await collect("server", "services");
  expect(invoke).toHaveBeenLastCalledWith("cockpit_collect", {
    id: "server",
    section: "services",
    refresh: false,
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
  // @ts-expect-error services returns service rows, not an Overview
  const overview: Promise<import("../domain/cockpit").Overview> = collect(
    "s",
    "services",
  );
  return overview;
}
void invalidRequests;
