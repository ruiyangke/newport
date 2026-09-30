import type { GitPage } from "../domain/gitResponses";
import { expect, it } from "vitest";
import { createQueryClient } from "./client";
import { createUniquePageAppender } from "../git/uniquePages";

it("publishes validated appends directly but shares equal refreshes normally", () => {
  const client = createQueryClient();
  const key = ["pages"];
  const first: GitPage<{ id: string }, Record<string, never>> = {
    snapshot: "s",
    entries: [{ id: "a" }],
    metadata: {},
    nextCursor: "next",
  };
  try {
    const stored = client.setQueryData<typeof first>(key, first)!;
    const append = createUniquePageAppender<
      { id: string },
      Record<string, never>
    >((row) => row.id);
    const next = append(
      stored,
      { ...first, entries: [{ id: "b" }], nextCursor: null },
      "next",
    );
    expect(client.setQueryData<typeof first>(key, next)).toBe(next);
    expect(next.entries[0]).toBe(stored.entries[0]);
    const refreshed = {
      ...next,
      entries: next.entries.map((row) => ({ ...row })),
    };
    expect(client.setQueryData<typeof first>(key, refreshed)).toBe(next);
    const changed = { ...next, entries: [{ id: "c" }] };
    expect(client.setQueryData<typeof first>(key, changed)?.entries).toEqual([
      { id: "c" },
    ]);
    // A once-valid append cannot bypass comparison after replacement/rollback.
    expect(client.setQueryData<typeof first>(key, next)?.entries).toEqual([
      { id: "a" },
      { id: "b" },
    ]);
  } finally {
    client.clear();
  }
});
