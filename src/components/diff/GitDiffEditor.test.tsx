// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { EditorView } from "@codemirror/view";
import { gitPath } from "../../domain/git";
import GitDiffEditor, { type GitDiffEditorProps } from "./GitDiffEditor";
import { diffModel, lineAt } from "./diffExtensions";
import type { DiffFile } from "./diffModel";

let root: Root;
let host: HTMLDivElement;
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});

const HUNK = "a".repeat(64);
const id = (n: number) => n.toString(16).padStart(64, "0");
/** Two deletions replaced by three additions, one of which has no id. */
function file(): DiffFile {
  const line = (
    origin: string,
    text: string,
    oldLine: number | null,
    newLine: number | null,
    lineId: string | null = null,
  ) => ({
    id: lineId,
    origin,
    oldLine,
    newLine,
    content: gitPath(`${text}\n`),
  });
  return {
    oldPath: gitPath("a.txt"),
    newPath: gitPath("a.txt"),
    oldOid: null,
    newOid: null,
    oldMode: 0o100644,
    newMode: 0o100644,
    status: "Modified",
    binary: false,
    additions: 3,
    deletions: 2,
    hunks: [
      {
        id: HUNK,
        oldStart: 1,
        oldLines: 3,
        newStart: 1,
        newLines: 4,
        lines: [
          line(" ", "keep", 1, 1),
          line("-", "old 2", 2, null, id(1)),
          line("-", "old 3", 3, null, id(2)),
          line("+", "new 2", null, 2, id(3)),
          line("+", "new 3", null, 3, null),
          line("+", "new 4", null, 4, id(4)),
        ],
      },
    ],
  } as DiffFile;
}
function props(
  overrides: Partial<GitDiffEditorProps> = {},
): GitDiffEditorProps {
  return {
    file: file(),
    layout: "unified",
    hunkLabel: "Stage hunk",
    lineLabel: "Stage",
    canDiscard: true,
    selected: new Set(),
    disabled: false,
    onToggle: vi.fn(),
    onApplyHunk: vi.fn(),
    onDiscardHunk: vi.fn(),
    ...overrides,
  };
}
const views = () =>
  [...host.querySelectorAll<HTMLElement>(".cm-editor")].map((dom) =>
    EditorView.findFromDOM(dom)!,
  );
const checkboxes = () => [
  ...host.querySelectorAll<HTMLElement>('[role="checkbox"][data-diff-line]'),
];
const labelled = (label: string) =>
  host.querySelector<HTMLElement>(`[aria-label="${label}"]`);

it("tags every drawn line with the agent's ids and maps document lines back to them", async () => {
  await act(async () => root.render(<GitDiffEditor {...props()} />));
  const [view] = views();
  const drawn = [...host.querySelectorAll<HTMLElement>(".cm-line")];
  expect(drawn.map((line) => line.dataset.lineId ?? null)).toEqual([
    null,
    id(1),
    id(2),
    id(3),
    null,
    id(4),
  ]);
  expect(drawn.every((line) => line.dataset.hunkId === HUNK)).toBe(true);
  expect(drawn.map((line) => line.dataset.mark).join("")).toBe(" --+++");
  expect(lineAt(view.state, 4)?.lineId).toBe(id(3));
  // The header band, with the hunk's actions, sits above the hunk.
  expect(host.querySelector(".cm-diff-hunk")?.textContent).toContain(
    "@@ −1,3 +1,4 @@",
  );
  expect(labelled("Stage hunk at line 1")).not.toBeNull();
  expect(labelled("Discard hunk at line 1")).not.toBeNull();
  // Read-only, and never wrapped: long lines scroll.
  expect(view.state.readOnly).toBe(true);
  expect(view.lineWrapping).toBe(false);
});

it("reports exactly the clicked line's id, and a whole run from its strip", async () => {
  const onToggle = vi.fn();
  await act(async () =>
    root.render(<GitDiffEditor {...props({ onToggle })} />),
  );
  // Four addressable lines, four checkboxes; the line without an id has none.
  expect(checkboxes().map((box) => box.getAttribute("aria-label"))).toEqual([
    "Stage removed line 2",
    "Stage removed line 3",
    "Stage added line 2",
    "Stage added line 4",
  ]);
  await act(async () => checkboxes()[2].click());
  expect(onToggle).toHaveBeenLastCalledWith([id(3)], true);
  // Clicking the unaddressed line's number cell selects nothing.
  onToggle.mockClear();
  const unaddressed = host.querySelector<HTMLElement>('[data-diff-line="4"]')!;
  expect(unaddressed.getAttribute("role")).toBeNull();
  await act(async () => unaddressed.click());
  expect(onToggle).not.toHaveBeenCalled();
  // The strip beside the first run selects the run, and only it.
  await act(async () => labelled("Stage 3 changed lines from line 2")!.click());
  expect(onToggle).toHaveBeenLastCalledWith([id(1), id(2), id(3)], true);
});

it("shows a selection on the same checkbox it was made with, and blocks it while disabled", async () => {
  const onToggle = vi.fn();
  // The same diff throughout: a new diff is a new document.
  const same = file();
  await act(async () =>
    root.render(<GitDiffEditor {...props({ onToggle, file: same })} />),
  );
  const box = checkboxes()[0];
  expect(box.getAttribute("aria-checked")).toBe("false");
  await act(async () =>
    root.render(
      <GitDiffEditor
        {...props({ onToggle, file: same, selected: new Set([id(1)]) })}
      />,
    ),
  );
  // Updated in place, so focus and identity survive a toggle.
  expect(checkboxes()[0]).toBe(box);
  expect(box.getAttribute("aria-checked")).toBe("true");
  expect(
    labelled("Stage 3 changed lines from line 2")!.getAttribute("aria-checked"),
  ).toBe("mixed");
  // Clicking a selected line deselects exactly it.
  await act(async () => box.click());
  expect(onToggle).toHaveBeenLastCalledWith([id(1)], false);
  onToggle.mockClear();
  await act(async () =>
    root.render(
      <GitDiffEditor
        {...props({
          onToggle,
          file: same,
          selected: new Set([id(1)]),
          disabled: true,
        })}
      />,
    ),
  );
  expect(box.getAttribute("aria-disabled")).toBe("true");
  expect(labelled("Stage hunk at line 1")).toHaveProperty("disabled", true);
  await act(async () => box.click());
  expect(onToggle).not.toHaveBeenCalled();
});

it("offers no selection or hunk actions where staging is not available", async () => {
  await act(async () =>
    root.render(
      <GitDiffEditor
        {...props({ hunkLabel: undefined, lineLabel: undefined })}
      />,
    ),
  );
  expect(host.querySelectorAll('[role="checkbox"]')).toHaveLength(0);
  expect(host.querySelectorAll("button")).toHaveLength(0);
  expect(host.querySelectorAll(".cm-line")).toHaveLength(6);
});

it("draws split sides with the same number of rows and the hunk actions once", async () => {
  await act(async () =>
    root.render(<GitDiffEditor {...props({ layout: "split" })} />),
  );
  const [before, after] = views();
  expect(before.state.doc.toString()).toBe("keep\nold 2\nold 3");
  expect(after.state.doc.toString()).toBe("keep\nnew 2\nnew 3\nnew 4");
  const rows = (editor: EditorView) => {
    const dom = editor.dom;
    const spacers = [...dom.querySelectorAll<HTMLElement>(".cm-diff-spacer")]
      .map((spacer) => parseInt(spacer.style.height, 10) / 22)
      .reduce((sum, n) => sum + n, 0);
    return (
      dom.querySelectorAll(".cm-line").length +
      dom.querySelectorAll(".cm-diff-hunk").length +
      spacers
    );
  };
  expect(rows(before)).toBe(rows(after));
  expect(rows(before)).toBe(5);
  expect(
    host.querySelectorAll('[aria-label="Stage hunk at line 1"]'),
  ).toHaveLength(1);
  // Each side's checkboxes are that side's lines, by the agent's ids.
  expect(
    [
      ...before.dom.querySelectorAll<HTMLElement>(
        '[role="checkbox"][data-diff-line]',
      ),
    ].map(
      (box) => lineAt(before.state, Number(box.dataset.diffLine) + 1)?.lineId,
    ),
  ).toEqual([id(1), id(2)]);
  expect(
    [
      ...after.dom.querySelectorAll<HTMLElement>(
        '[role="checkbox"][data-diff-line]',
      ),
    ].map(
      (box) => lineAt(after.state, Number(box.dataset.diffLine) + 1)?.lineId,
    ),
  ).toEqual([id(3), id(4)]);
});

it("keeps every staging control reachable by assistive technology", async () => {
  // CodeMirror hides its gutter containers from assistive technology. Ours
  // hold the staging checkboxes, so a hidden container made every one of them
  // unreachable -- invisible to a screen reader, and to anything that finds a
  // control by its role. The unit tests found them by selector and passed.
  for (const layout of ["unified", "split"] as const) {
    await act(async () =>
      root.render(<GitDiffEditor {...props({ layout })} />),
    );
    await act(async () => new Promise((resolve) => setTimeout(resolve, 0)));
    const controls = [
      ...document.querySelectorAll<HTMLElement>('[role="checkbox"]'),
    ];
    expect(controls.length, layout).toBeGreaterThan(0);
    for (const control of controls)
      expect(control.closest('[aria-hidden="true"]'), layout).toBeNull();
    // Line numbers that are not controls stay out of the way.
    for (const number of document.querySelectorAll<HTMLElement>(
      ".cm-diff-number:not([role])",
    ))
      expect(number.getAttribute("aria-hidden"), layout).toBe("true");
  }
});

/**
 * Two hunks. The first ends in a deletion with no addition, so the new side of
 * a split folds a spacer into the same block as the second hunk's header; the
 * second ends in an addition, so the old side ends in a block of spacers only.
 */
function twoHunks(): DiffFile {
  const line = (
    origin: string,
    text: string,
    oldLine: number | null,
    newLine: number | null,
    lineId: string | null = null,
  ) => ({
    id: lineId,
    origin,
    oldLine,
    newLine,
    content: gitPath(`${text}\n`),
  });
  return {
    ...file(),
    additions: 1,
    deletions: 1,
    hunks: [
      {
        id: HUNK,
        oldStart: 1,
        oldLines: 2,
        newStart: 1,
        newLines: 1,
        lines: [line(" ", "keep", 1, 1), line("-", "gone", 2, null, id(1))],
      },
      {
        id: "b".repeat(64),
        oldStart: 10,
        oldLines: 1,
        newStart: 9,
        newLines: 2,
        lines: [line(" ", "stay", 10, 9), line("+", "came", null, 10, id(2))],
      },
    ],
  } as DiffFile;
}

it("carries the hunk header band across every gutter, and never beside a spacer", async () => {
  for (const layout of ["unified", "split"] as const) {
    await act(async () =>
      root.render(<GitDiffEditor {...props({ layout, file: twoHunks() })} />),
    );
    await act(async () => new Promise((resolve) => setTimeout(resolve, 0)));
    let mixed = 0;
    let spacersOnly = 0;
    for (const view of views()) {
      const { blocks } = view.state.facet(diffModel).model;
      const withHeader = blocks.filter((block) =>
        block.parts.some((part) => part.kind === "header"),
      );
      spacersOnly += blocks.length - withHeader.length;
      const gutters = [...view.dom.querySelectorAll<HTMLElement>(".cm-gutter")];
      // The strip and the number gutter(s): every one of them is banded.
      expect(gutters.length, layout).toBe(
        view.state.facet(diffModel).model.side === "unified" ? 3 : 2,
      );
      for (const gutter of gutters) {
        const bands = [
          ...gutter.querySelectorAll<HTMLElement>(".cm-diff-band"),
        ];
        // One band per block that holds a header, in document order; a block
        // of spacers alone gets nothing.
        expect(bands, layout).toHaveLength(withHeader.length);
        bands.forEach((band, index) => {
          const { parts } = withHeader[index];
          expect(band.getAttribute("aria-hidden"), layout).toBe("true");
          expect(band.textContent, layout).toBe("");
          const cell = band.parentElement!;
          expect(cell.classList.contains("cm-gutterElement"), layout).toBe(
            true,
          );
          if (parts.every((part) => part.kind === "header")) {
            // The whole cell takes the band colour.
            expect(cell.classList.contains("cm-diff-gutter-hunk"), layout).toBe(
              true,
            );
            expect(band.children, layout).toHaveLength(0);
          } else {
            // Header and spacers share a block: only the header rows tint.
            mixed += 1;
            expect(cell.classList.contains("cm-diff-gutter-hunk"), layout).toBe(
              false,
            );
            expect(
              [...band.children].map((slice) => [
                slice.className,
                (slice as HTMLElement).style.height,
              ]),
              layout,
            ).toEqual(
              parts.map((part) =>
                part.kind === "header"
                  ? ["cm-diff-gutter-hunk", "22px"]
                  : ["cm-diff-band-gap", `${part.rows * 22}px`],
              ),
            );
          }
        });
        // Nothing else in the gutter is tinted as a band.
        const tinted = gutter.querySelectorAll(
          ".cm-gutterElement.cm-diff-gutter-hunk, .cm-diff-band > .cm-diff-gutter-hunk",
        );
        expect(tinted, layout).toHaveLength(
          withHeader.reduce(
            (count, block) =>
              count +
              (block.parts.every((part) => part.kind === "header")
                ? 1
                : block.parts.filter((part) => part.kind === "header").length),
            0,
          ),
        );
      }
    }
    if (layout === "split") {
      // The fixture exercises both split cases.
      expect(mixed).toBeGreaterThan(0);
      expect(spacersOnly).toBeGreaterThan(0);
    }
    for (const control of document.querySelectorAll<HTMLElement>(
      '[role="checkbox"]',
    ))
      expect(control.closest('[aria-hidden="true"]'), layout).toBeNull();
  }
});
