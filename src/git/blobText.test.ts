import { expect, it, vi } from "vitest";
import {
  appendGitBlobPage,
  decodeGitBlobPage,
  validateInitialGitBlobPage,
} from "../domain/gitResponses";
import { GitRepositoryClient } from "../api/gitRepository";
import { blobText, createBlobTextReader } from "./blobText";
const oid = { algorithm: "sha1", hex: "a".repeat(40) };
function wire(
  raw: string,
  offset: number,
  size: number,
  nextCursor: string | null = null,
) {
  return {
    snapshot: "blob-snapshot",
    metadata: { oid, size },
    nextCursor,
    entries: raw.length ? [{ offset, bytesB64: btoa(raw) }] : [],
  };
}
it("assembles split UTF-8 and preserves an incomplete tail until the next page", () => {
  const raw = String.fromCharCode(...new TextEncoder().encode("A界B"));
  const first = validateInitialGitBlobPage(
    decodeGitBlobPage(wire(raw.slice(0, 2), 0, raw.length, "next")),
  );
  expect(blobText(first)).toBe("A");
  const last = decodeGitBlobPage(wire(raw.slice(2), 2, raw.length));
  expect(blobText(appendGitBlobPage(first, last, "next"))).toBe("A界B");
  expect(first.entries).toHaveLength(1);
  expect(blobText(decodeGitBlobPage(wire("", 0, 0)))).toBe("");
  expect(blobText(decodeGitBlobPage(wire("\0binary", 0, 7)))).toBeNull();
  expect(blobText(decodeGitBlobPage(wire("\xff", 0, 1)))).toBe("�");
});
it("rejects gaps, duplicates, changed identities, repeated cursors, and wrong sizes", () => {
  const first = decodeGitBlobPage(wire("abc", 0, 6, "next"));
  for (const mutate of [
    (v: ReturnType<typeof wire>) => {
      v.entries[0].offset = 2;
    },
    (v: ReturnType<typeof wire>) => {
      v.entries[0].offset = 4;
    },
    (v: ReturnType<typeof wire>) => {
      v.snapshot = "other";
    },
    (v: ReturnType<typeof wire>) => {
      v.metadata = { ...v.metadata, oid: { ...oid, hex: "b".repeat(40) } };
    },
    (v: ReturnType<typeof wire>) => {
      v.nextCursor = "next";
    },
    (v: ReturnType<typeof wire>) => {
      v.metadata.size = 7;
    },
  ]) {
    const value = wire("def", 3, 6);
    mutate(value);
    expect(() =>
      appendGitBlobPage(first, decodeGitBlobPage(value), "next"),
    ).toThrow();
  }
  expect(() =>
    validateInitialGitBlobPage(decodeGitBlobPage(wire("def", 3, 6))),
  ).toThrow();
  expect(() => decodeGitBlobPage(wire("", 0, 1, "next"))).toThrow();
  expect(() =>
    decodeGitBlobPage(
      wire("abc", Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER),
    ),
  ).toThrow();
});
it("loads beyond the previous 512 KiB cutoff without losing bytes", () => {
  const size = 600_000;
  let merged = validateInitialGitBlobPage(
    decodeGitBlobPage(wire("a".repeat(300_000), 0, size, "next")),
  );
  merged = appendGitBlobPage(
    merged,
    decodeGitBlobPage(wire("b".repeat(300_000), 300_000, size)),
    "next",
  );
  const text = blobText(merged)!;
  expect(text.length).toBe(size);
  expect(text.slice(299_999, 300_001)).toBe("ab");
});
it("binds the typed client response to the requested blob and initial offset", async () => {
  for (const value of [wire("x", 0, 1), wire("x", 1, 2)]) {
    const session = { request: vi.fn(async () => value), forget: vi.fn() };
    const client = new GitRepositoryClient(session);
    await expect(
      client.blobPage({
        repoId: "repo",
        oid: value.entries[0].offset ? oid.hex : "b".repeat(40),
      }),
    ).rejects.toThrow();
    expect(session.forget).toHaveBeenCalledOnce();
  }
  const session = {
    request: vi.fn(async () => wire("abc", 0, 3)),
    forget: vi.fn(),
  };
  const result = await new GitRepositoryClient(session).blobPage({
    repoId: "repo",
    oid: oid.hex.toUpperCase(),
    maxBytes: 65536,
  });
  expect(blobText(result)).toBe("abc");
  expect(session.forget).not.toHaveBeenCalled();
});

it("decodes only appended chunks and resets correctly for rollback, replacement, and a different blob", () => {
  const reader = createBlobTextReader();
  const raw = String.fromCharCode(...new TextEncoder().encode("A界B"));
  const first = decodeGitBlobPage(wire(raw.slice(0, 2), 0, raw.length, "next"));
  const complete = appendGitBlobPage(
    first,
    decodeGitBlobPage(wire(raw.slice(2), 2, raw.length)),
    "next",
  );
  const spy = vi.spyOn(globalThis, "atob");
  try {
    expect(reader(first)).toBe("A");
    expect(spy).toHaveBeenCalledTimes(1);
    expect(reader(first)).toBe("A");
    expect(spy).toHaveBeenCalledTimes(1);
    expect(reader(complete)).toBe("A界B");
    expect(spy).toHaveBeenCalledTimes(2);
    expect(reader(complete)).toBe("A界B");
    expect(spy).toHaveBeenCalledTimes(2);
    expect(reader(first)).toBe("A");
    expect(spy).toHaveBeenCalledTimes(3);
    expect(reader(complete)).toBe("A界B");
    const replaced = structuredClone(complete);
    expect(reader(replaced)).toBe("A界B");
    const different = decodeGitBlobPage(wire("else", 0, 4));
    different.snapshot = "another";
    expect(reader(different)).toBe("else");
    const binary = decodeGitBlobPage(wire("\0data", 0, 5));
    expect(reader(binary)).toBeNull();
    expect(reader(first)).toBe("A");
  } finally {
    spy.mockRestore();
  }
});
