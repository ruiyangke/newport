import { expect, it, vi } from "vitest";
import { createUniquePageAppender } from "./uniquePages";
import type { GitPage } from "../domain/gitResponses";
const page = (
  entries: string[],
  nextCursor: string | null,
  snapshot = "s",
): GitPage<string, Record<string, never>> => ({
  snapshot,
  entries,
  nextCursor,
  metadata: {},
});
it("indexes each newly loaded row once, preserves order, and rebuilds after rollback", () => {
  const key = vi.fn((entry: string) => entry);
  const append = createUniquePageAppender(key);
  const first = page(["a", "a"], "one");
  const second = append(first, page(["a", "b", "b", "c"], "two"), "one");
  expect(second.entries).toEqual(["a", "b", "c"]);
  key.mockClear();
  const last = append(second, page(["c", "d"], null), "two");
  expect(last.entries).toEqual(["a", "b", "c", "d"]);
  expect(key.mock.calls.map(([value]) => value)).toEqual(["c", "d"]);
  expect(first.entries).toEqual(["a", "a"]);
  expect(second.entries).toEqual(["a", "b", "c"]);
  expect(append(first, page(["b", "c"], null), "one").entries).toEqual([
    "a",
    "b",
    "c",
  ]);
});
it("failed appends cannot poison the index used by a retry", () => {
  let fail = false;
  const append = createUniquePageAppender((entry: string) => {
    if (fail && entry === "d") throw Error("invalid key");
    return entry;
  });
  const second = append(page(["a"], "one"), page(["b"], "two"), "one");
  fail = true;
  expect(() => append(second, page(["c", "d"], null), "two")).toThrow(
    "invalid key",
  );
  fail = false;
  expect(() => append(second, page(["c"], "two"), "two")).toThrow();
  expect(() => append(second, page(["a", "b"], "three"), "two")).toThrow(
    "did not add",
  );
  expect(append(second, page(["c", "d"], null), "two").entries).toEqual([
    "a",
    "b",
    "c",
    "d",
  ]);
});
it("rebuilds for a new page object even when its snapshot is unchanged", () => {
  const append = createUniquePageAppender((entry: string) => entry);
  const old = append(page(["old"], "one"), page(["old-next"], "two"), "one");
  const refreshed = page(["fresh"], "two");
  expect(append(refreshed, page(["old"], null), "two").entries).toEqual([
    "fresh",
    "old",
  ]);
  expect(() =>
    append(old, page(["other"], null, "different"), "two"),
  ).toThrow();
});

it("marks only validated appends from the exact predecessor", async () => {
  const { isValidatedPageAppend } = await import("./uniquePages");
  const append = createUniquePageAppender((entry: string) => entry);
  const before = page(["a"], "one");
  const after = append(before, page(["b"], null), "one");
  expect(isValidatedPageAppend(before, after)).toBe(true);
  expect(isValidatedPageAppend({ ...before }, after)).toBe(false);
  expect(isValidatedPageAppend(before, { ...after })).toBe(false);
  expect(isValidatedPageAppend(undefined, after)).toBe(false);
  expect(isValidatedPageAppend(before, null)).toBe(false);
});
