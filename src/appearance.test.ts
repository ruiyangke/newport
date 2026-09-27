import { beforeEach, expect, it, vi } from "vitest";
import { getAppearance } from "./appearance";
let values: Map<string, string>;
beforeEach(() => {
  values = new Map();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  });
});
it("adopts the Porthop theme and preserves the old preference", () => {
  values.set("porthop-appearance", "dark");
  expect(getAppearance()).toBe("dark");
  expect(values.get("newport-appearance")).toBe("dark");
  expect(values.get("porthop-appearance")).toBe("dark");
  values.set("newport-appearance", "light");
  expect(getAppearance()).toBe("light");
});
it("keeps the legacy theme when migration cannot write", () => {
  values.set("porthop-appearance", "dark");
  localStorage.setItem = () => {
    throw new Error("Unavailable");
  };
  expect(getAppearance()).toBe("dark");
});
