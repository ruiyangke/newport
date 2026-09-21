import { beforeEach, expect, it, vi } from "vitest";
const api = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn(() => true) }));
vi.mock("@tauri-apps/api/core", () => api);
import { loadSidebarWidth, saveSidebarWidth } from "./preferences";
let values: Map<string, string>;
beforeEach(() => {
  values = new Map();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  });
  api.invoke.mockReset();
});
it("migrates an old preference only after successfully persisting it", async () => {
  values.set("sidebar-width", "240");
  api.invoke
    .mockResolvedValueOnce(null)
    .mockRejectedValueOnce(new Error("disk full"));
  await expect(loadSidebarWidth()).rejects.toThrow("disk full");
  expect(values.get("sidebar-width")).toBe("240");
  api.invoke.mockResolvedValueOnce(null).mockResolvedValueOnce(undefined);
  expect(await loadSidebarWidth()).toBe(240);
  expect(values.has("sidebar-width")).toBe(false);
});
it("keeps a user resize after a slow migration", async () => {
  let finish!: (value: null) => void;
  api.invoke.mockImplementation((command: string) =>
    command === "get_sidebar_width"
      ? new Promise((resolve) => {
          finish = resolve;
        })
      : Promise.resolve(),
  );
  const loading = loadSidebarWidth();
  const saving = saveSidebarWidth(280);
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  finish(null);
  await Promise.all([loading, saving]);
  expect(
    api.invoke.mock.calls
      .filter(([command]) => command === "set_sidebar_width")
      .map(([, args]) => args.width),
  ).toEqual([204, 280]);
});
it("retains the existing Store value without migrating over it", async () => {
  values.set("sidebar-width", "240");
  api.invoke.mockResolvedValueOnce(260);
  expect(await loadSidebarWidth()).toBe(260);
  expect(api.invoke).toHaveBeenCalledTimes(1);
});
