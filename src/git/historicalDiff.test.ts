import { expect, it, vi } from "vitest";
import { gitPath } from "../domain/git";
import {
  appendGitDiffPage,
  decodeGitCommitDiffPage,
  decodeGitWorkingDiffPage,
  validateInitialGitDiffPage,
} from "../domain/gitResponses";
import {
  historicalDiff,
  workingDiff,
  createDiffRenderer,
} from "./historicalDiff";

const oid = { algorithm: "sha1", hex: "a".repeat(40) };
function wire(
  content: string,
  offset = 0,
  complete = true,
  nextCursor: string | null = null,
  total = 1,
) {
  return {
    snapshot: "snapshot",
    nextCursor,
    metadata: {
      commitOid: oid,
      parentOid: null,
      parentIndex: null,
      parents: [],
      contextLines: 3,
      selectedPath: gitPath("file"),
      readOnly: true,
      hasOmissions: false,
      totalUnits: total,
    },
    entries: [
      {
        fileIndex: 0,
        oldPath: null,
        newPath: gitPath("file"),
        oldOid: null,
        newOid: oid,
        oldMode: 0,
        newMode: 33188,
        status: "Added",
        binary: false,
        omissionReason: null,
        additions: 1,
        deletions: 0,
        hunks: [
          {
            index: 0,
            oldStart: 0,
            oldLines: 0,
            newStart: 1,
            newLines: 1,
            lines: [
              {
                lineIndex: 0,
                byteOffset: offset,
                lineComplete: complete,
                origin: "+",
                oldLine: null,
                newLine: 1,
                contentBytesB64: btoa(content),
              },
            ],
          },
        ],
      },
    ],
  };
}
it("joins split UTF-8 bytes before display and keeps historical fragments read-only", () => {
  const bytes = new TextEncoder().encode("x".repeat(4095) + "界\n");
  const raw = Array.from(bytes, (byte) => String.fromCharCode(byte)).join("");
  const first = validateInitialGitDiffPage(
    decodeGitCommitDiffPage(wire(raw.slice(0, 4096), 0, false, "next", 2)),
  );
  expect(historicalDiff(first).files[0].hunks[0].lines).toEqual([]);
  const second = decodeGitCommitDiffPage(
    wire(raw.slice(4096), 4096, true, null, 2),
  );
  const joined = appendGitDiffPage(first, second, "next");
  const rendered = historicalDiff(joined);
  const line = rendered.files[0].hunks[0].lines[0];
  expect(line.content.display).toBe("x".repeat(4095) + "界\n");
  expect(line.content.bytesB64).toBe(btoa(raw));
  expect(line.id).toBeNull();
  expect(rendered.readOnly).toBe(true);
  expect(rendered.files[0].hunks[0].id).toBeNull();
  expect(first.entries[0].hunks[0].lines).toHaveLength(1);
});
it("rejects missing, repeated, reordered, or inconsistent line fragments", () => {
  const first = decodeGitCommitDiffPage(
    wire("x".repeat(4096), 0, false, "next", 2),
  );
  for (const mutate of [
    (p: ReturnType<typeof wire>) => {
      p.entries[0].hunks[0].lines[0].byteOffset++;
    },
    (p: ReturnType<typeof wire>) => {
      p.entries[0].hunks[0].lines[0].byteOffset = 0;
    },
    (p: ReturnType<typeof wire>) => {
      p.entries[0].hunks[0].lines[0].newLine = 2;
    },
    (p: ReturnType<typeof wire>) => {
      p.entries[0].hunks[0].newLines++;
    },
    (p: ReturnType<typeof wire>) => {
      p.metadata.totalUnits++;
    },
    (p: ReturnType<typeof wire>) => {
      p.snapshot = "stale";
    },
    (p: ReturnType<typeof wire>) => {
      p.nextCursor = "next";
    },
  ]) {
    const bad = wire("end\n", 4096, true, null, 2);
    mutate(bad);
    expect(() =>
      appendGitDiffPage(first, decodeGitCommitDiffPage(bad), "next"),
    ).toThrow();
  }
});
it("rejects prematurely finished pages and incomplete initial selections", () => {
  expect(() =>
    validateInitialGitDiffPage(decodeGitCommitDiffPage(wire("end", 4096))),
  ).toThrow();
  expect(() =>
    validateInitialGitDiffPage(
      decodeGitCommitDiffPage(wire("text", 0, true, null, 2)),
    ),
  ).toThrow();
  expect(() =>
    validateInitialGitDiffPage(
      decodeGitCommitDiffPage(wire("x".repeat(4096), 0, false)),
    ),
  ).toThrow();
  expect(() =>
    decodeGitCommitDiffPage(wire("small", 0, false, "next", 2)),
  ).toThrow();
  const writable = wire("text");
  writable.metadata.readOnly = false;
  expect(() => decodeGitCommitDiffPage(writable)).toThrow();
});
it("preserves non-UTF8 bytes even when the display needs replacement characters", () => {
  const page = validateInitialGitDiffPage(
    decodeGitCommitDiffPage(wire("\xff\n")),
  );
  const content = historicalDiff(page).files[0].hunks[0].lines[0].content;
  expect(content.bytesB64).toBe(btoa("\xff\n"));
  expect(content.display).toBe("\ufffd\n");
});

function workingWire(
  content: string,
  offset = 0,
  complete = true,
  next: string | null = null,
  total = 1,
) {
  const original = wire(content, offset, complete, next, total);
  return {
    ...original,
    metadata: {
      sourceSnapshot: "status",
      entryId: "entry",
      side: "index_to_worktree",
      contextLines: 3,
      readOnly: false,
      hasOmissions: false,
      totalFiles: 1,
      totalUnits: total,
    },
    entries: original.entries.map((file) => ({
      ...file,
      hunks: file.hunks.map((hunk) => ({
        ...hunk,
        id: "a".repeat(64),
        totalLines: 1,
        lines: hunk.lines.map((line) => ({
          ...line,
          id: complete ? "b".repeat(64) : null,
        })),
      })),
    })),
  };
}
it("exposes mutation IDs only after a complete working hunk is assembled", () => {
  const first = validateInitialGitDiffPage(
    decodeGitWorkingDiffPage(
      workingWire("x".repeat(4096), 0, false, "next", 2),
    ),
  );
  const partial = workingDiff(first);
  expect(partial.snapshot).toBe("status");
  expect(partial.files[0].hunks[0].id).toBeNull();
  expect(partial.files[0].hunks[0].lines).toHaveLength(0);
  const last = decodeGitWorkingDiffPage(
    workingWire("tail\n", 4096, true, null, 2),
  );
  const complete = workingDiff(appendGitDiffPage(first, last, "next"));
  expect(complete.files[0].hunks[0].id).toBe("a".repeat(64));
  expect(complete.files[0].hunks[0].lines[0].id).toBe("b".repeat(64));
  expect(complete.files[0].hunks[0].lines[0].content.display).toBe(
    "x".repeat(4096) + "tail\n",
  );
});
it("rejects incomplete hunk ends, invalid identifiers, and incomplete directory metadata", () => {
  for (const mutate of [
    (v: ReturnType<typeof workingWire>) => {
      v.entries[0].hunks[0].totalLines = 2;
    },
    (v: ReturnType<typeof workingWire>) => {
      v.entries[0].hunks[0].lines[0].id = null;
    },
    (v: ReturnType<typeof workingWire>) => {
      v.entries[0].hunks[0].id = "invalid";
    },
    (v: ReturnType<typeof workingWire>) => {
      v.metadata.totalFiles = 2;
    },
  ]) {
    const value = workingWire("line\n");
    mutate(value);
    expect(() =>
      validateInitialGitDiffPage(decodeGitWorkingDiffPage(value)),
    ).toThrow();
  }
  const value = workingWire("line\n", 0, true, "next", 2);
  value.metadata.totalFiles = 2;
  const partial = workingDiff(
    validateInitialGitDiffPage(decodeGitWorkingDiffPage(value)),
  );
  expect(partial.files[0].hunks[0].id).toBeNull();
  expect(partial.files[0].hunks[0].lines[0].id).toBeNull();
});

it("decodes compact diff rows identically and rejects malformed tuples", () => {
  for (const working of [false, true]) {
    for (const complete of [false, true]) {
      const original = working
        ? workingWire(complete ? "text\n" : "x".repeat(4096), 0, complete)
        : wire(complete ? "text\n" : "x".repeat(4096), 0, complete);
      const decode = working
        ? decodeGitWorkingDiffPage
        : decodeGitCommitDiffPage;
      const compact = {
        ...original,
        entries: original.entries.map((file) => ({
          ...file,
          hunks: file.hunks.map((hunk) => ({
            ...hunk,
            lines: hunk.lines.map((line) => [
              line.lineIndex,
              line.byteOffset,
              line.lineComplete,
              line.origin,
              line.oldLine,
              line.newLine,
              line.contentBytesB64,
              ...("id" in line ? [line.id] : []),
            ]),
          })),
        })),
      };
      expect(decode(compact)).toEqual(decode(original));
      for (const change of [
        (row: unknown[]) => row.push("extra"),
        (row: unknown[]) => row.pop(),
        (row: unknown[]) => {
          row[0] = -1;
        },
        (row: unknown[]) => {
          row[3] = "invalid";
        },
        (row: unknown[]) => {
          row[6] = "not base64!";
        },
        ...(working
          ? [
              (row: unknown[]) => {
                row[7] = complete ? null : "a".repeat(64);
              },
            ]
          : []),
      ]) {
        const bad = structuredClone(compact);
        change(bad.entries[0].hunks[0].lines[0]);
        expect(() => decode(bad)).toThrow();
      }
    }
  }
});

it("reuses decoded lines on append and rollback without mutating prior staging IDs", () => {
  const firstWire = workingWire("first\n", 0, true, "next", 2);
  firstWire.entries[0].hunks[0].totalLines = 2;
  const lastWire = workingWire("last\n", 0, true, null, 2);
  lastWire.entries[0].hunks[0].totalLines = 2;
  lastWire.entries[0].hunks[0].lines[0].lineIndex = 1;
  lastWire.entries[0].hunks[0].lines[0].newLine = 2;
  const first = decodeGitWorkingDiffPage(firstWire);
  const complete = appendGitDiffPage(
    first,
    decodeGitWorkingDiffPage(lastWire),
    "next",
  );
  const render = createDiffRenderer();
  const decode = vi.spyOn(globalThis, "atob");
  try {
    const partial = render(first);
    expect(decode).toHaveBeenCalledTimes(1);
    expect(partial.files[0].hunks[0].lines[0].id).toBeNull();
    const final = render(complete);
    expect(decode).toHaveBeenCalledTimes(2);
    expect(final.files[0].hunks[0].lines[0].content).toBe(
      partial.files[0].hunks[0].lines[0].content,
    );
    expect(final.files[0].hunks[0].lines[0].id).toBe("b".repeat(64));
    expect(partial.files[0].hunks[0].lines[0].id).toBeNull();
    expect(render(first)).toEqual(partial);
    expect(render(complete)).toEqual(final);
    expect(decode).toHaveBeenCalledTimes(2);
    render({ ...complete, snapshot: "another-selection" });
    expect(decode).toHaveBeenCalledTimes(4);
  } finally {
    decode.mockRestore();
  }
});

it("joins split UTF-8 before caching and invalidates replaced prefixes", () => {
  const first = decodeGitCommitDiffPage(
    wire("x".repeat(4095) + "\xe7", 0, false, "next", 2),
  );
  const last = decodeGitCommitDiffPage(wire("\x95\x8c\n", 4096, true, null, 2));
  const complete = appendGitDiffPage(first, last, "next");
  const replaced = {
    ...complete,
    entries: complete.entries.map((file) => ({
      ...file,
      hunks: file.hunks.map((hunk) => ({
        ...hunk,
        lines: hunk.lines.map((piece, index) =>
          index
            ? piece
            : { ...piece, contentBytesB64: btoa("y".repeat(4095) + "\xe7") },
        ),
      })),
    })),
  };
  const render = createDiffRenderer();
  const decode = vi.spyOn(globalThis, "atob");
  try {
    expect(render(first).files[0].hunks[0].lines).toEqual([]);
    expect(decode).not.toHaveBeenCalled();
    expect(render(complete).files[0].hunks[0].lines[0].content.display).toBe(
      "x".repeat(4095) + "界\n",
    );
    expect(decode).toHaveBeenCalledTimes(2);
    render(complete);
    expect(decode).toHaveBeenCalledTimes(2);
    expect(render(replaced).files[0].hunks[0].lines[0].content.display).toBe(
      "y".repeat(4095) + "界\n",
    );
    expect(decode).toHaveBeenCalledTimes(4);
    expect(render(complete).files[0].hunks[0].lines[0].content.display).toBe(
      "x".repeat(4095) + "界\n",
    );
  } finally {
    decode.mockRestore();
  }
});
