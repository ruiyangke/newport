import { expect, it } from "vitest";
import { gitPath } from "../../domain/git";
import {
  buildSplit,
  buildUnified,
  selectionPayload,
  toggleSelection,
  visualRows,
  type DiffFile,
} from "./diffModel";

const H1 = "a".repeat(64);
const H2 = "b".repeat(64);
const id = (n: number) => n.toString(16).padStart(64, "0");
type Line = DiffFile["hunks"][number]["lines"][number];
const line = (
  origin: string,
  text: string,
  oldLine: number | null,
  newLine: number | null,
  lineId: string | null = null,
): Line => ({
  id: lineId,
  origin,
  oldLine,
  newLine,
  content: gitPath(`${text}\n`),
});
/**
 * Two hunks: the first replaces two lines with three (unequal on purpose), the
 * second only deletes. Every changed line has an id except one addition, which
 * stands in for a line the agent did not make addressable.
 */
function file(): DiffFile {
  return {
    oldPath: gitPath("src/app.ts"),
    newPath: gitPath("src/app.ts"),
    oldOid: null,
    newOid: null,
    oldMode: 0o100644,
    newMode: 0o100644,
    status: "Modified",
    binary: false,
    additions: 3,
    deletions: 3,
    hunks: [
      {
        id: H1,
        oldStart: 1,
        oldLines: 4,
        newStart: 1,
        newLines: 5,
        lines: [
          line(" ", "keep 1", 1, 1),
          line("-", "old 2", 2, null, id(1)),
          line("-", "old 3", 3, null, id(2)),
          line("+", "new 2", null, 2, id(3)),
          line("+", "new 3", null, 3, null),
          line("+", "new 4", null, 4, id(4)),
          line(" ", "keep 4", 4, 5),
        ],
      },
      {
        id: H2,
        oldStart: 20,
        oldLines: 2,
        newStart: 21,
        newLines: 1,
        lines: [
          line(" ", "keep 20", 20, 21),
          line("-", "gone 21", 21, null, id(5)),
        ],
      },
    ],
  } as DiffFile;
}

it("builds the unified document from the agent's lines, each carrying its ids", () => {
  const model = buildUnified(file(), true);
  expect(model.doc.split("\n")).toEqual([
    "keep 1",
    "old 2",
    "old 3",
    "new 2",
    "new 3",
    "new 4",
    "keep 4",
    "keep 20",
    "gone 21",
  ]);
  // Document line n is agent line n: same ids, same hunk, same numbers.
  expect(model.lines.map((l) => [l.lineId, l.hunkId])).toEqual([
    [null, H1],
    [id(1), H1],
    [id(2), H1],
    [id(3), H1],
    [null, H1],
    [id(4), H1],
    [null, H1],
    [null, H2],
    [id(5), H2],
  ]);
  expect(model.lines.map((l) => l.mark).join("")).toBe(" --+++  -");
  expect(model.lines[3]).toMatchObject({ oldLine: null, newLine: 2 });
  // One header above each hunk's first line.
  expect(model.blocks).toEqual([
    { beforeLine: 1, parts: [{ kind: "header", hunk: 0 }] },
    { beforeLine: 8, parts: [{ kind: "header", hunk: 1 }] },
  ]);
  expect(model.hunks[0].header).toBe("@@ −1,4 +1,5 @@");
});

it("never lets a line without an id be selected, alone or through its run", () => {
  const model = buildUnified(file(), true);
  const unaddressed = model.lines[4];
  expect(unaddressed.lineId).toBeNull();
  expect(unaddressed.selectable).toBe(false);
  expect(unaddressed.run).toBeNull();
  // The unaddressed line splits the run around it, so no strip covers it.
  expect(model.runs.map((run) => run.lineIds)).toEqual([
    [id(1), id(2), id(3)],
    [id(4)],
    [id(5)],
  ]);
  expect(model.runs.flatMap((run) => run.lineIds)).not.toContain(null);
  // Context is never selectable, and nothing is where staging is not offered.
  expect(model.lines[0].selectable).toBe(false);
  expect(buildUnified(file(), false).lines.some((l) => l.selectable)).toBe(
    false,
  );
  expect(buildUnified(file(), false).runs).toEqual([]);
  // A hunk without an id makes none of its lines selectable.
  const orphan = file();
  orphan.hunks[1] = { ...orphan.hunks[1], id: null };
  expect(
    buildUnified(orphan, true).lines.filter(
      (l) => l.hunkIndex === 1 && l.selectable,
    ),
  ).toEqual([]);
});

it("sends exactly the selected ids, in the order they were picked", () => {
  let selected: ReadonlySet<string> = new Set();
  selected = toggleSelection(selected, [id(4)], true);
  selected = toggleSelection(selected, [id(5), id(1)], true);
  selected = toggleSelection(selected, [id(1)], false);
  // An id the diff does not contain is never sent.
  selected = toggleSelection(selected, ["f".repeat(64)], true);
  expect(selectionPayload(file(), selected)).toEqual({
    lines: [id(4), id(5)],
    hunks: [H1, H2],
  });
  // A whole run, as the strip selects it, and then cleared by the same gesture.
  const run = buildUnified(file(), true).runs[0].lineIds;
  selected = toggleSelection(new Set(), run, true);
  expect(selectionPayload(file(), selected)).toEqual({
    lines: [id(1), id(2), id(3)],
    hunks: [H1],
  });
  expect(
    selectionPayload(file(), toggleSelection(selected, run, false)),
  ).toEqual({
    lines: [],
    hunks: [],
  });
});

it("aligns split sides with spacers so both occupy the same rows", () => {
  const split = buildSplit(file(), true);
  expect(split.old.doc.split("\n")).toEqual([
    "keep 1",
    "old 2",
    "old 3",
    "keep 4",
    "keep 20",
    "gone 21",
  ]);
  expect(split.new.doc.split("\n")).toEqual([
    "keep 1",
    "new 2",
    "new 3",
    "new 4",
    "keep 4",
    "keep 20",
  ]);
  // Two deletions beside three additions: one spacer row on the old side,
  // placed after its deletions, before the context that follows.
  expect(split.old.blocks).toEqual([
    { beforeLine: 1, parts: [{ kind: "header", hunk: 0 }] },
    { beforeLine: 4, parts: [{ kind: "spacer", rows: 1 }] },
    { beforeLine: 5, parts: [{ kind: "header", hunk: 1 }] },
  ]);
  // The trailing deletion has no partner: the new side pads after its end.
  expect(split.new.blocks).toEqual([
    { beforeLine: 1, parts: [{ kind: "header", hunk: 0 }] },
    { beforeLine: 6, parts: [{ kind: "header", hunk: 1 }] },
    { beforeLine: 7, parts: [{ kind: "spacer", rows: 1 }] },
  ]);
  expect(visualRows(split.old)).toBe(visualRows(split.new));
  // Two headers, five rows for the first hunk, two for the second.
  expect(visualRows(split.old)).toBe(2 + 5 + 2);
  // Each side keeps the agent's ids for its own lines only.
  expect(split.old.lines.map((l) => l.lineId).filter(Boolean)).toEqual([
    id(1),
    id(2),
    id(5),
  ]);
  expect(split.new.lines.map((l) => l.lineId).filter(Boolean)).toEqual([
    id(3),
    id(4),
  ]);
});

it("keeps one document line per agent line, whatever the content holds", () => {
  const value = file();
  value.hunks = [
    {
      ...value.hunks[1],
      lines: [
        line("-", "gone 21", 21, null, id(5)),
        // git's end-of-file note arrives wrapped in newlines.
        {
          id: null,
          origin: "<",
          oldLine: null,
          newLine: null,
          content: gitPath("\n\\ No newline at end of file\n"),
        },
        line("+", "a\r", null, 21, id(6)),
      ],
    },
  ];
  const model = buildUnified(value, true);
  expect(model.doc.split("\n")).toHaveLength(3);
  expect(model.lines[1]).toMatchObject({
    kind: "meta",
    mark: "",
    text: "\\ No newline at end of file",
    selectable: false,
  });
  const split = buildSplit(value, true);
  expect(visualRows(split.old)).toBe(visualRows(split.new));
});
