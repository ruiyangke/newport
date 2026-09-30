/**
 * The agent's diff, laid out as documents for a read-only code view.
 *
 * Nothing here computes a diff. Every line placed in a document is one of the
 * agent's hunk lines, in the agent's order, carrying the agent's line and hunk
 * identifiers; the only thing decided locally is where each line sits on
 * screen. Staging sends those identifiers back, so what is highlighted is
 * exactly what git will be asked to stage.
 *
 * Kept free of CodeMirror so the selection rules can be used, and tested,
 * without loading the editor.
 */
import type { GitDiff } from "../../domain/gitResponses";

export type DiffFile = GitDiff["files"][number];
type AgentHunk = DiffFile["hunks"][number];
type AgentLine = AgentHunk["lines"][number];

export type LineKind = "add" | "del" | "ctx" | "meta";
export interface ModelLine {
  kind: LineKind;
  /** The mark column: `+`, `-`, a space for context, empty for git's notes. */
  mark: string;
  oldLine: number | null;
  newLine: number | null;
  /** The agent's identifier for this line; null for context and notes. */
  lineId: string | null;
  /** The agent's identifier for the hunk the line belongs to. */
  hunkId: string | null;
  hunkIndex: number;
  /** Only an identified line in an identified hunk can be staged by itself. */
  selectable: boolean;
  /** Index into `runs` for a selectable line. */
  run: number | null;
  text: string;
}
export interface ModelHunk {
  index: number;
  id: string | null;
  header: string;
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
}
/** A block placed above a document line: hunk headers and alignment spacers. */
export type BlockPart =
  { kind: "header"; hunk: number } | { kind: "spacer"; rows: number };
export interface Block {
  /** 1-based document line the block sits above; `lines.length + 1` = after the end. */
  beforeLine: number;
  parts: BlockPart[];
}
/** A run of consecutive selectable lines: the design's selection strip. */
export interface Run {
  lineIds: string[];
  /** 1-based document lines. */
  first: number;
  last: number;
}
export type SideKind = "unified" | "old" | "new";
export interface SideModel {
  side: SideKind;
  lines: ModelLine[];
  hunks: ModelHunk[];
  blocks: Block[];
  runs: Run[];
  doc: string;
}

const MINUS = "−";
export function hunkHeader(
  hunk: Pick<AgentHunk, "oldStart" | "oldLines" | "newStart" | "newLines">,
) {
  return `@@ ${MINUS}${hunk.oldStart},${hunk.oldLines} +${hunk.newStart},${hunk.newLines} @@`;
}

function kindOf(origin: string): LineKind {
  switch (origin) {
    case "+":
      return "add";
    case "-":
      return "del";
    case " ":
    case "":
      return "ctx";
    default:
      // git's "\ No newline at end of file" notes: `=`, `>` and `<`.
      return "meta";
  }
}
/** Which side of a split view an agent line belongs to. */
function sidesOf(origin: string): { old: boolean; new: boolean } {
  switch (origin) {
    case "+":
    case ">":
      return { old: false, new: true };
    case "-":
    case "<":
      return { old: true, new: false };
    default:
      return { old: true, new: true };
  }
}
/**
 * One document line per agent line. The agent's content carries its own line
 * ending, and git's end-of-file notes arrive wrapped in newlines; neither may
 * add a document line, or every line after it would carry the wrong id.
 */
function lineText(line: AgentLine) {
  const text = line.content.display.replace(/\r?\n$/, "");
  return kindOf(line.origin) === "meta"
    ? text.replace(/^\r?\n/, "").replace(/[\r\n]+/g, " ")
    : text.replace(/\r?\n/g, "␤");
}

function modelLine(
  line: AgentLine,
  hunk: AgentHunk,
  hunkIndex: number,
  selectable: boolean,
): ModelLine {
  const kind = kindOf(line.origin);
  return {
    kind,
    mark:
      kind === "add" ? "+" : kind === "del" ? "-" : kind === "ctx" ? " " : "",
    oldLine: line.oldLine,
    newLine: line.newLine,
    lineId: line.id,
    hunkId: hunk.id,
    hunkIndex,
    selectable: selectable && !!line.id && !!hunk.id,
    run: null,
    text: lineText(line),
  };
}

function hunksOf(file: DiffFile): ModelHunk[] {
  return file.hunks.map((hunk, index) => ({
    index,
    id: hunk.id,
    header: hunkHeader(hunk),
    oldStart: hunk.oldStart,
    oldLines: hunk.oldLines,
    newStart: hunk.newStart,
    newLines: hunk.newLines,
  }));
}

/** Groups consecutive selectable lines of one hunk into runs. */
function withRuns(lines: ModelLine[]): Run[] {
  const runs: Run[] = [];
  let current: Run | null = null;
  lines.forEach((line, index) => {
    const previous = lines[index - 1];
    if (!line.selectable) {
      current = null;
      return;
    }
    if (!current || previous?.hunkIndex !== line.hunkIndex) {
      current = { lineIds: [], first: index + 1, last: index + 1 };
      runs.push(current);
    }
    current.lineIds.push(line.lineId!);
    current.last = index + 1;
    line.run = runs.length - 1;
  });
  return runs;
}

function side(
  kind: SideKind,
  lines: ModelLine[],
  hunks: ModelHunk[],
  blocks: Block[],
): SideModel {
  return {
    side: kind,
    lines,
    hunks,
    blocks,
    runs: withRuns(lines),
    doc: lines.map((line) => line.text).join("\n"),
  };
}

/**
 * The unified document: every agent line in order, with each hunk's header
 * above its first line.
 */
export function buildUnified(file: DiffFile, selectable: boolean): SideModel {
  const lines: ModelLine[] = [];
  const blocks: Block[] = [];
  file.hunks.forEach((hunk, hunkIndex) => {
    blocks.push({
      beforeLine: lines.length + 1,
      parts: [{ kind: "header", hunk: hunkIndex }],
    });
    for (const line of hunk.lines)
      lines.push(modelLine(line, hunk, hunkIndex, selectable));
  });
  return side("unified", lines, hunksOf(file), blocks);
}

/** One visual row of a split view: a line on either side, or on one. */
type Row =
  | { kind: "header"; hunk: number }
  | {
      kind: "pair";
      old: AgentLine | null;
      new: AgentLine | null;
      hunk: number;
    };

/**
 * The rows the hunks already imply. Context appears on both sides; within a
 * run of changes, the n-th deletion sits beside the n-th addition, and the
 * shorter side is padded. That pairing is read off the agent's own line order,
 * so nothing is compared here.
 */
function splitRows(file: DiffFile): Row[] {
  const rows: Row[] = [];
  file.hunks.forEach((hunk, hunkIndex) => {
    rows.push({ kind: "header", hunk: hunkIndex });
    let index = 0;
    while (index < hunk.lines.length) {
      const line = hunk.lines[index];
      const on = sidesOf(line.origin);
      if (on.old && on.new) {
        rows.push({ kind: "pair", old: line, new: line, hunk: hunkIndex });
        index += 1;
        continue;
      }
      const removed: AgentLine[] = [];
      const added: AgentLine[] = [];
      while (index < hunk.lines.length) {
        const next = sidesOf(hunk.lines[index].origin);
        if (next.old && next.new) break;
        (next.old ? removed : added).push(hunk.lines[index]);
        index += 1;
      }
      for (let n = 0; n < Math.max(removed.length, added.length); n += 1)
        rows.push({
          kind: "pair",
          old: removed[n] ?? null,
          new: added[n] ?? null,
          hunk: hunkIndex,
        });
    }
  });
  return rows;
}

function splitSide(
  file: DiffFile,
  rows: Row[],
  which: "old" | "new",
  selectable: boolean,
): SideModel {
  const lines: ModelLine[] = [];
  const blocks: Block[] = [];
  let pending: BlockPart[] = [];
  const flush = () => {
    if (pending.length)
      blocks.push({ beforeLine: lines.length + 1, parts: pending });
    pending = [];
  };
  for (const row of rows) {
    if (row.kind === "header") {
      pending.push({ kind: "header", hunk: row.hunk });
      continue;
    }
    const line = row[which];
    if (!line) {
      const last = pending.at(-1);
      if (last?.kind === "spacer") last.rows += 1;
      else pending.push({ kind: "spacer", rows: 1 });
      continue;
    }
    flush();
    lines.push(modelLine(line, file.hunks[row.hunk], row.hunk, selectable));
  }
  flush();
  return side(which, lines, hunksOf(file), blocks);
}

export function buildSplit(file: DiffFile, selectable: boolean) {
  const rows = splitRows(file);
  return {
    old: splitSide(file, rows, "old", selectable),
    new: splitSide(file, rows, "new", selectable),
  };
}

/** Rows a side occupies on screen: its lines plus every block above them. */
export function visualRows(model: SideModel) {
  return (
    model.lines.length +
    model.blocks.reduce(
      (sum, block) =>
        sum +
        block.parts.reduce(
          (rows, part) => rows + (part.kind === "header" ? 1 : part.rows),
          0,
        ),
      0,
    )
  );
}

/** Applies one gesture to a selection, keeping the order lines were picked in. */
export function toggleSelection(
  selected: ReadonlySet<string>,
  ids: readonly string[],
  checked: boolean,
): ReadonlySet<string> {
  const next = new Set(selected);
  for (const id of ids) {
    if (checked) next.add(id);
    else next.delete(id);
  }
  return next;
}

/**
 * What a selection sends: the agent's line ids, in the order they were picked,
 * and the agent's ids of the hunks they belong to. An id the current diff does
 * not contain, or one on a line that cannot be staged by itself, is dropped.
 */
export function selectionPayload(
  file: DiffFile,
  selected: ReadonlySet<string>,
) {
  const hunkOf = new Map<string, string>();
  for (const hunk of file.hunks)
    for (const line of hunk.lines)
      if (hunk.id && line.id) hunkOf.set(line.id, hunk.id);
  const lines = [...selected].filter((id) => hunkOf.has(id));
  return { hunks: [...new Set(lines.map((id) => hunkOf.get(id)!))], lines };
}
