/**
 * CodeMirror pieces for showing the agent's diff read-only.
 *
 * The document is built from the agent's hunk lines (see diffModel.ts); these
 * extensions only decorate it. Every line decoration carries the agent's line
 * and hunk ids, the model facet maps each document line back to them, and the
 * only way to select anything is through a line's gutter or a run's strip,
 * both of which resolve to those ids. There is no client-side diffing here.
 */
import {
  EditorState,
  Facet,
  RangeSetBuilder,
  StateEffect,
  StateField,
  type Extension,
} from "@codemirror/state";
import {
  Decoration,
  EditorView,
  GutterMarker,
  ViewPlugin,
  WidgetType,
  gutter,
  keymap,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";
import {
  HighlightStyle,
  LanguageDescription,
  defaultHighlightStyle,
  syntaxHighlighting,
} from "@codemirror/language";
import { search, searchKeymap } from "@codemirror/search";
import type { Block, BlockPart, ModelLine, SideModel } from "./diffModel";

export const ROW = 22;

/** What the editor needs from the React side. Read through a ref, so a new
    callback never forces the editor to be rebuilt. */
export interface DiffHandlers {
  toggle(ids: string[], checked: boolean): void;
  applyHunk(hunkId: string): void;
  discardHunk?(hunkId: string): void;
}
export interface DiffControls {
  /** "Stage hunk" or "Unstage hunk". */
  hunkLabel: string;
  /** "Stage" or "Unstage", for single lines. */
  lineLabel: string;
  canDiscard: boolean;
}
export interface DiffUi {
  selected: ReadonlySet<string>;
  disabled: boolean;
}
export interface DiffConfig {
  model: SideModel;
  /** Present only where lines and hunks can be staged. */
  controls: DiffControls | null;
  /** Split view draws the hunk actions once, on the new side. */
  hunkActions: boolean;
  handlers: { current: DiffHandlers };
  label: string;
}

export const setDiffUi = StateEffect.define<DiffUi>();
export const diffUi = StateField.define<DiffUi>({
  create: () => ({ selected: new Set(), disabled: false }),
  update(value, tr) {
    for (const effect of tr.effects)
      if (effect.is(setDiffUi)) value = effect.value;
    return value;
  },
});
/** The side model this editor shows: document line n is `lines[n - 1]`. */
export const diffModel = Facet.define<DiffConfig, DiffConfig>({
  combine: (values) => values[0],
  static: true,
});

/** The agent's ids for a document line; null where the line has none. */
export function lineAt(
  state: EditorState,
  lineNumber: number,
): ModelLine | null {
  return state.facet(diffModel).model.lines[lineNumber - 1] ?? null;
}

/* ------------------------------------------------------------------------ */
/* Line decorations: added, removed, context and git's notes, each tagged    */
/* with the agent's ids.                                                     */

const lineClass: Record<ModelLine["kind"], string> = {
  add: "cm-diff-add",
  del: "cm-diff-del",
  ctx: "cm-diff-ctx",
  meta: "cm-diff-meta",
};
function lineDecorations(model: SideModel, state: EditorState) {
  const builder = new RangeSetBuilder<Decoration>();
  model.lines.forEach((line, index) => {
    const attributes: Record<string, string> = { "data-mark": line.mark };
    if (line.lineId) attributes["data-line-id"] = line.lineId;
    if (line.hunkId) attributes["data-hunk-id"] = line.hunkId;
    const from = state.doc.line(index + 1).from;
    builder.add(
      from,
      from,
      Decoration.line({ class: lineClass[line.kind], attributes }),
    );
  });
  return builder.finish();
}

/* ------------------------------------------------------------------------ */
/* Blocks: the hunk header band and the spacers that keep split sides level. */

function readUi(view: EditorView) {
  return view.state.field(diffUi);
}

/** Rows a block part occupies: a header is one, a spacer its count. */
const partRows = (part: BlockPart) => (part.kind === "header" ? 1 : part.rows);

class BlockWidget extends WidgetType {
  constructor(
    readonly block: Block,
    readonly config: DiffConfig,
  ) {
    super();
  }
  eq(other: BlockWidget) {
    return other.block === this.block && other.config === this.config;
  }
  get estimatedHeight() {
    return (
      this.block.parts.reduce((rows, part) => rows + partRows(part), 0) * ROW
    );
  }
  ignoreEvent() {
    return true;
  }
  toDOM(view: EditorView) {
    const wrap = document.createElement("div");
    wrap.className = "cm-diff-block";
    const { model, controls, hunkActions, handlers } = this.config;
    const disabled = readUi(view).disabled;
    for (const part of this.block.parts) {
      if (part.kind === "spacer") {
        const spacer = document.createElement("div");
        spacer.className = "cm-diff-spacer";
        spacer.style.height = `${part.rows * ROW}px`;
        spacer.setAttribute("aria-hidden", "true");
        wrap.append(spacer);
        continue;
      }
      const hunk = model.hunks[part.hunk];
      const header = document.createElement("div");
      header.className = "cm-diff-hunk";
      const text = document.createElement("span");
      text.className = "cm-diff-hunk-text";
      text.textContent = hunk.header;
      header.append(text);
      if (controls && hunkActions && hunk.id) {
        const id = hunk.id;
        header.append(
          hunkButton(
            controls.hunkLabel,
            `${controls.hunkLabel} at line ${controls.hunkLabel === "Stage hunk" ? hunk.newStart : hunk.oldStart}`,
            disabled,
            () => handlers.current.applyHunk(id),
          ),
        );
        if (controls.canDiscard)
          header.append(
            hunkButton(
              "Discard hunk",
              `Discard hunk at line ${hunk.newStart}`,
              disabled,
              () => handlers.current.discardHunk?.(id),
              true,
            ),
          );
      }
      wrap.append(header);
    }
    return wrap;
  }
}
function hunkButton(
  text: string,
  label: string,
  disabled: boolean,
  onClick: () => void,
  destructive = false,
) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = destructive
    ? "cm-diff-hunk-button cm-diff-hunk-discard"
    : "cm-diff-hunk-button";
  button.textContent = text;
  button.setAttribute("aria-label", label);
  button.dataset.diffAction = "hunk";
  button.disabled = disabled;
  button.addEventListener("click", (event) => {
    event.preventDefault();
    if (!button.disabled) onClick();
  });
  return button;
}

function blockDecorations(config: DiffConfig, state: EditorState) {
  const { model } = config;
  const ranges = model.blocks.map((block) => {
    const atEnd = block.beforeLine > state.doc.lines;
    const pos = atEnd
      ? state.doc.length
      : state.doc.line(block.beforeLine).from;
    return Decoration.widget({
      widget: new BlockWidget(block, config),
      block: true,
      side: atEnd ? 1 : -1,
    }).range(pos);
  });
  return Decoration.set(ranges, true);
}

const staticDecorations = StateField.define<DecorationSet>({
  create(state) {
    const config = state.facet(diffModel);
    return lineDecorations(config.model, state).update({
      add: [...iterate(blockDecorations(config, state))],
      sort: true,
    });
  },
  update: (value) => value,
  provide: (field) => EditorView.decorations.from(field),
});
function* iterate(set: DecorationSet) {
  for (let cursor = set.iter(); cursor.value; cursor.next())
    yield cursor.value.range(cursor.from, cursor.to);
}

/* ------------------------------------------------------------------------ */
/* Gutters. Selection state is written onto existing gutter nodes in place   */
/* (see `syncSelection`) rather than by replacing them, so a checkbox keeps  */
/* its identity, and its focus, when it is toggled.                          */

function toggleLine(view: EditorView, index: number) {
  const { model, handlers } = view.state.facet(diffModel);
  const line = model.lines[index];
  const ui = readUi(view);
  if (!line?.selectable || !line.lineId || ui.disabled) return;
  handlers.current.toggle([line.lineId], !ui.selected.has(line.lineId));
}
function toggleRun(view: EditorView, run: number) {
  const { model, handlers } = view.state.facet(diffModel);
  const ids = model.runs[run]?.lineIds;
  const ui = readUi(view);
  if (!ids?.length || ui.disabled) return;
  handlers.current.toggle(ids, !ids.every((id) => ui.selected.has(id)));
}
function activate(element: HTMLElement, run: () => void) {
  element.addEventListener("mousedown", (event) => event.preventDefault());
  element.addEventListener("click", (event) => {
    event.preventDefault();
    run();
  });
  element.addEventListener("keydown", (event) => {
    if (event.key !== " " && event.key !== "Enter") return;
    event.preventDefault();
    run();
  });
}

function lineState(element: HTMLElement, line: ModelLine, ui: DiffUi) {
  const selected = !!line.lineId && ui.selected.has(line.lineId);
  if (selected) element.dataset.selected = "true";
  else delete element.dataset.selected;
  if (element.getAttribute("role") === "checkbox") {
    element.setAttribute("aria-checked", String(selected));
    if (ui.disabled) element.setAttribute("aria-disabled", "true");
    else element.removeAttribute("aria-disabled");
  }
}
function runState(
  element: HTMLElement,
  lineIds: readonly string[],
  ui: DiffUi,
) {
  const selected = lineIds.every((id) => ui.selected.has(id));
  if (selected) element.dataset.selected = "true";
  else delete element.dataset.selected;
  if (element.getAttribute("role") === "checkbox") {
    element.setAttribute(
      "aria-checked",
      selected
        ? "true"
        : lineIds.some((id) => ui.selected.has(id))
          ? "mixed"
          : "false",
    );
    if (ui.disabled) element.setAttribute("aria-disabled", "true");
    else element.removeAttribute("aria-disabled");
  }
}

const numberText = (value: number | null) =>
  value === null ? "" : String(value);

class NumberMarker extends GutterMarker {
  constructor(
    readonly index: number,
    readonly column: "old" | "new",
    readonly checkbox: boolean,
    kind: ModelLine["kind"],
  ) {
    super();
    // The row's tint carries across its gutter, as the design's rows do.
    this.elementClass = `cm-diff-gutter-${kind}`;
  }
  eq(other: NumberMarker) {
    return (
      other.index === this.index &&
      other.column === this.column &&
      other.checkbox === this.checkbox
    );
  }
  toDOM(view: EditorView) {
    const { model, controls } = view.state.facet(diffModel);
    const line = model.lines[this.index];
    const element = document.createElement("span");
    element.className = "cm-diff-number";
    element.dataset.diffLine = String(this.index);
    element.textContent = numberText(
      this.column === "old" ? line.oldLine : line.newLine,
    );
    // Only a staging control is exposed. Every other number is a visual echo
    // of what the checkbox's label already says, so it is hidden here rather
    // than by hiding the whole gutter -- see exposeGutterControls.
    if (!(line.selectable && controls && this.checkbox))
      element.setAttribute("aria-hidden", "true");
    if (line.selectable && controls) {
      if (this.checkbox) {
        element.setAttribute("role", "checkbox");
        element.tabIndex = 0;
        element.setAttribute(
          "aria-label",
          `${controls.lineLabel} ${line.kind === "add" ? "added" : "removed"} line ${line.newLine ?? line.oldLine ?? ""}`,
        );
      } else element.setAttribute("aria-hidden", "true");
      element.dataset.selectable = "true";
      activate(element, () => toggleLine(view, this.index));
    }
    lineState(element, line, readUi(view));
    return element;
  }
}

class StripMarker extends GutterMarker {
  constructor(
    readonly run: number,
    readonly first: boolean,
    readonly last: boolean,
    readonly kind: ModelLine["kind"],
  ) {
    super();
    // The strip is part of its row: without the row's tint it read as a white
    // gap between the pane's edge and the coloured line.
    this.elementClass = `cm-diff-gutter-${kind}`;
  }
  eq(other: StripMarker) {
    return (
      other.run === this.run &&
      other.first === this.first &&
      other.last === this.last &&
      other.kind === this.kind
    );
  }
  toDOM(view: EditorView) {
    const { model, controls } = view.state.facet(diffModel);
    const run = model.runs[this.run];
    const element = document.createElement("span");
    element.className = "cm-diff-strip-cell";
    element.dataset.diffRun = String(this.run);
    if (this.first) element.dataset.first = "true";
    if (this.last) element.dataset.last = "true";
    if (this.first && controls) {
      const start = model.lines[run.first - 1];
      const count = run.lineIds.length;
      element.setAttribute("role", "checkbox");
      element.tabIndex = 0;
      element.setAttribute(
        "aria-label",
        `${controls.lineLabel} ${count} changed ${count === 1 ? "line" : "lines"} from line ${start.newLine ?? start.oldLine ?? ""}`,
      );
    } else element.setAttribute("aria-hidden", "true");
    activate(element, () => toggleRun(view, this.run));
    runState(element, run.lineIds, readUi(view));
    return element;
  }
}

const uiChanged = (update: ViewUpdate) =>
  update.transactions.some((tr) => tr.effects.some((e) => e.is(setDiffUi)));

class SpacerMarker extends GutterMarker {
  constructor(readonly text: string) {
    super();
  }
  eq(other: SpacerMarker) {
    return other.text === this.text;
  }
  toDOM() {
    const element = document.createElement("span");
    element.className = "cm-diff-number";
    element.setAttribute("aria-hidden", "true");
    element.textContent = this.text;
    return element;
  }
}

/*
 * The hunk header band runs across the gutters too, as the design's does:
 * every gutter puts a marker beside each block widget through `widgetMarker`.
 * A block made only of hunk headers tints its whole gutter cell through the
 * marker's `elementClass`. A split side can fold alignment spacers into the
 * same block as the next hunk's header (a hunk ending in lines this side does
 * not have), so there the marker draws one slice per part instead, tinting
 * only the header rows; spacers stay blank, and a block of spacers alone gets
 * no marker at all. Presentational only: hidden from assistive technology.
 */
const BAND = "cm-diff-gutter-hunk";

class BandMarker extends GutterMarker {
  constructor(readonly block: Block) {
    super();
    if (this.whole) this.elementClass = BAND;
  }
  get whole() {
    return this.block.parts.every((part) => part.kind === "header");
  }
  eq(other: BandMarker) {
    return other.block === this.block;
  }
  toDOM() {
    const element = document.createElement("span");
    element.className = "cm-diff-band";
    element.setAttribute("aria-hidden", "true");
    if (!this.whole)
      for (const part of this.block.parts) {
        const slice = document.createElement("span");
        slice.className = part.kind === "header" ? BAND : "cm-diff-band-gap";
        slice.style.height = `${partRows(part) * ROW}px`;
        element.append(slice);
      }
    return element;
  }
}

function bandMarker(widget: WidgetType) {
  if (!(widget instanceof BlockWidget)) return null;
  const { block } = widget;
  return block.parts.some((part) => part.kind === "header")
    ? new BandMarker(block)
    : null;
}

function numberGutter(
  column: "old" | "new",
  checkbox: boolean,
  after: boolean,
) {
  return gutter({
    class: `cm-diff-numbers cm-diff-numbers-${column}`,
    side: after ? "after" : "before",
    lineMarker(view, block) {
      const index = view.state.doc.lineAt(block.from).number - 1;
      const line = view.state.facet(diffModel).model.lines[index];
      return line ? new NumberMarker(index, column, checkbox, line.kind) : null;
    },
    widgetMarker: (_view, widget) => bandMarker(widget),
    initialSpacer(view) {
      const lines = view.state.facet(diffModel).model.lines;
      let widest = 0;
      for (const line of lines)
        widest = Math.max(
          widest,
          (column === "old" ? line.oldLine : line.newLine) ?? 0,
        );
      return new SpacerMarker("9".repeat(Math.max(3, String(widest).length)));
    },
  });
}

const stripGutter = gutter({
  class: "cm-diff-strip",
  lineMarker(view, block) {
    const { model } = view.state.facet(diffModel);
    const number = view.state.doc.lineAt(block.from).number;
    const line = model.lines[number - 1];
    if (line?.run === null || line?.run === undefined) return null;
    const run = model.runs[line.run];
    return new StripMarker(
      line.run,
      run.first === number,
      run.last === number,
      line.kind,
    );
  },
  widgetMarker: (_view, widget) => bandMarker(widget),
  initialSpacer: () => new SpacerMarker(""),
});

/** Writes the current selection onto every rendered gutter node and button. */
function syncSelection(view: EditorView) {
  const { model } = view.state.facet(diffModel);
  const ui = readUi(view);
  for (const element of view.dom.querySelectorAll<HTMLElement>(
    "[data-diff-line]",
  )) {
    const line = model.lines[Number(element.dataset.diffLine)];
    if (line) lineState(element, line, ui);
  }
  for (const element of view.dom.querySelectorAll<HTMLElement>(
    "[data-diff-run]",
  )) {
    const run = model.runs[Number(element.dataset.diffRun)];
    if (run) runState(element, run.lineIds, ui);
  }
  for (const button of view.dom.querySelectorAll<HTMLButtonElement>(
    "button[data-diff-action]",
  ))
    button.disabled = ui.disabled;
}
/*
 * CodeMirror marks every gutter container `aria-hidden="true"`, on the
 * reasonable assumption that gutters hold line numbers. Ours hold the staging
 * controls: a checkbox per selectable line and one per run. Left hidden, they
 * were unreachable by a screen reader and by anything that finds controls by
 * role -- which is how the browser suite caught it, while the unit tests,
 * querying the DOM by selector, passed. Each marker above decides its own
 * visibility, so the containers are opened up. The old side's gutter in split
 * view is created lazily after construction, hence the check on every update.
 */
function exposeGutters(view: EditorView) {
  for (const gutters of view.dom.querySelectorAll(".cm-gutters[aria-hidden]"))
    gutters.removeAttribute("aria-hidden");
}
const exposeGutterControls = ViewPlugin.define((view) => {
  queueMicrotask(() => exposeGutters(view));
  return { update: (update: ViewUpdate) => exposeGutters(update.view) };
});

const selectionSync = ViewPlugin.define(() => ({
  update(update: ViewUpdate) {
    if (uiChanged(update)) syncSelection(update.view);
  },
}));

/* ------------------------------------------------------------------------ */
/* Theme. Colours are CSS variables so the `.dark` palette remaps them.     */

const SYNTAX = HighlightStyle.define(
  defaultHighlightStyle.specs.map((spec) =>
    typeof spec.color === "string"
      ? {
          ...spec,
          // The stock palette is tuned for a light ground. Mixed toward the
          // foreground by a theme-controlled amount, it stays legible on dark.
          color: `color-mix(in srgb, ${spec.color} var(--git-diff-syntax-strength), var(--foreground))`,
        }
      : spec,
  ),
);

const light = {
  "--git-diff-add-bg": "rgb(237 248 240)",
  "--git-diff-add-ink": "rgb(36 113 75)",
  "--git-diff-del-bg": "rgb(255 240 241)",
  "--git-diff-del-ink": "rgb(172 52 72)",
  "--git-diff-hunk-bg": "rgb(241 248 255)",
  "--git-diff-hunk-ink": "rgb(88 96 105)",
  "--git-diff-gutter-ink": "rgb(98 107 121)",
  "--git-diff-gutter-selected": "rgb(33 136 255)",
  "--git-diff-gutter-selected-ink": "rgb(255 255 255)",
  "--git-diff-strip-selected": "rgb(0 92 197)",
  "--git-diff-strip-hover": "rgb(0 92 197 / 0.35)",
  "--git-diff-rule": "rgb(225 228 233)",
  "--git-diff-spacer": "rgb(246 247 249)",
  "--git-diff-syntax-strength": "100%",
};
const dark: typeof light = {
  "--git-diff-add-bg": "rgb(30 46 38)",
  "--git-diff-add-ink": "rgb(121 206 169)",
  "--git-diff-del-bg": "rgb(56 34 39)",
  "--git-diff-del-ink": "rgb(251 138 141)",
  "--git-diff-hunk-bg": "rgb(36 40 48)",
  "--git-diff-hunk-ink": "rgb(160 168 180)",
  "--git-diff-gutter-ink": "rgb(139 146 158)",
  "--git-diff-gutter-selected": "rgb(31 111 235)",
  "--git-diff-gutter-selected-ink": "rgb(255 255 255)",
  "--git-diff-strip-selected": "rgb(56 139 253)",
  "--git-diff-strip-hover": "rgb(56 139 253 / 0.35)",
  "--git-diff-rule": "rgb(60 59 64)",
  "--git-diff-spacer": "rgb(38 39 42)",
  "--git-diff-syntax-strength": "55%",
};

export const diffTheme = EditorView.theme({
  "&": {
    ...light,
    color: "var(--foreground)",
    backgroundColor: "transparent",
    fontSize: "12px",
  },
  ".dark &": dark,
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": {
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
    lineHeight: `${ROW}px`,
  },
  ".cm-content": { padding: "0", minHeight: "0" },
  // Unified grows to its content and lets the surrounding `.git-diff-code`
  // box do all the scrolling, so a long line scrolls rather than wrapping.
  "&.cm-diff-unified .cm-scroller": { overflow: "visible" },
  ".cm-line": {
    position: "relative",
    padding: "0 20px 0 18px",
    tabSize: "4",
  },
  ".cm-line::before": {
    content: "attr(data-mark)",
    position: "absolute",
    left: "0",
    width: "18px",
    textAlign: "center",
    whiteSpace: "pre",
  },
  ".cm-diff-add": {
    backgroundColor: "var(--git-diff-add-bg)",
    color: "var(--git-diff-add-ink)",
  },
  ".cm-diff-del": {
    backgroundColor: "var(--git-diff-del-bg)",
    color: "var(--git-diff-del-ink)",
  },
  ".cm-diff-meta": {
    color: "var(--muted-foreground)",
    fontStyle: "italic",
  },
  ".cm-gutters": {
    backgroundColor: "var(--background)",
    color: "var(--git-diff-gutter-ink)",
    border: "none",
  },
  ".cm-gutters.cm-gutters-before, .cm-gutters.cm-gutters-after": {
    borderWidth: "0",
  },
  ".cm-gutterElement": { padding: "0" },
  ".cm-gutterElement.cm-diff-gutter-add": {
    backgroundColor: "var(--git-diff-add-bg)",
  },
  ".cm-gutterElement.cm-diff-gutter-del": {
    backgroundColor: "var(--git-diff-del-bg)",
  },
  // Beside a hunk header, the gutter takes the header's band colour.
  [`.cm-gutterElement.${BAND}, .cm-diff-band > .${BAND}`]: {
    backgroundColor: "var(--git-diff-hunk-bg)",
  },
  ".cm-diff-band": {
    display: "flex",
    flex: "1",
    flexDirection: "column",
  },
  ".cm-diff-band > span": { flex: "none" },
  ".cm-diff-numbers .cm-gutterElement": { display: "flex" },
  ".cm-diff-number": {
    display: "block",
    flex: "1",
    boxSizing: "border-box",
    minWidth: "45px",
    padding: "0 8px",
    fontSize: "10px",
    textAlign: "right",
    userSelect: "none",
  },
  ".cm-diff-number[data-selectable]": { cursor: "pointer" },
  ".cm-diff-number[data-selected]": {
    backgroundColor: "var(--git-diff-gutter-selected)",
    color: "var(--git-diff-gutter-selected-ink)",
  },
  ".cm-diff-number:focus-visible, .cm-diff-strip-cell:focus-visible": {
    outline: "2px solid var(--ring)",
    outlineOffset: "-2px",
  },
  ".cm-diff-strip": { width: "16px" },
  ".cm-diff-strip .cm-gutterElement": { display: "flex" },
  ".cm-diff-strip-cell": {
    display: "block",
    width: "16px",
    cursor: "pointer",
  },
  ".cm-diff-strip-cell:hover": {
    backgroundColor: "var(--git-diff-strip-hover)",
  },
  ".cm-diff-strip-cell[data-selected]": {
    backgroundColor: "var(--git-diff-strip-selected)",
  },
  ".cm-diff-strip-cell[aria-disabled]": { cursor: "default" },
  ".cm-diff-hunk": {
    display: "flex",
    alignItems: "center",
    gap: "8px",
    boxSizing: "border-box",
    height: `${ROW}px`,
    overflow: "hidden",
    padding: "0 8px",
    backgroundColor: "var(--git-diff-hunk-bg)",
    color: "var(--git-diff-hunk-ink)",
    fontSize: "11px",
    lineHeight: "20px",
    whiteSpace: "nowrap",
  },
  ".cm-diff-hunk-button": {
    height: "18px",
    padding: "0 6px",
    border: "1px solid var(--border)",
    borderRadius: "4px",
    backgroundColor: "var(--background)",
    color: "var(--foreground)",
    font: "500 11px/16px var(--font-sans, system-ui)",
    cursor: "pointer",
  },
  ".cm-diff-hunk-button:hover": { backgroundColor: "var(--muted)" },
  ".cm-diff-hunk-button:focus-visible": {
    outline: "2px solid var(--ring)",
    outlineOffset: "1px",
  },
  ".cm-diff-hunk-button:disabled": { opacity: "0.5", cursor: "default" },
  ".cm-diff-hunk-discard": { color: "var(--destructive)" },
  ".cm-diff-spacer": { backgroundColor: "var(--git-diff-spacer)" },
  // Split sides, measured: 55px gutters and 12px of code padding on the right.
  "&.cm-diff-side .cm-diff-number": { minWidth: "55px" },
  "&.cm-diff-side .cm-line": { paddingRight: "12px" },
  // The old side of a split: its numbers sit on the inner edge, as designed.
  ".cm-gutters-after": { backgroundColor: "var(--background)" },
  ".cm-panels": {
    backgroundColor: "var(--muted)",
    color: "var(--foreground)",
    fontFamily: "var(--font-sans, system-ui)",
  },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--border)" },
  ".cm-searchMatch": { backgroundColor: "rgb(255 213 0 / 0.35)" },
  ".cm-searchMatch-selected": { backgroundColor: "rgb(255 150 0 / 0.5)" },
});

/**
 * Everything a diff editor needs, except the language, which is loaded later
 * into `language` so highlighting can never hold the diff back.
 */
export function diffExtensions(
  config: DiffConfig,
  language: Extension,
  ui: DiffUi = { selected: new Set(), disabled: false },
): Extension[] {
  const { model, controls } = config;
  const selectable = !!controls && model.runs.length > 0;
  const gutters: Extension[] = [];
  if (model.side === "unified") {
    if (selectable) gutters.push(stripGutter);
    gutters.push(
      numberGutter("old", true, false),
      numberGutter("new", false, false),
    );
  } else if (model.side === "old") {
    if (selectable) gutters.push(stripGutter);
    gutters.push(numberGutter("old", true, true));
  } else {
    if (selectable) gutters.push(stripGutter);
    gutters.push(numberGutter("new", true, false));
  }
  return [
    diffModel.of(config),
    diffUi.init(() => ui),
    EditorState.readOnly.of(true),
    EditorView.contentAttributes.of({ "aria-label": config.label }),
    EditorView.editorAttributes.of({
      class: model.side === "unified" ? "cm-diff-unified" : "cm-diff-side",
    }),
    staticDecorations,
    selectionSync,
    ...gutters,
    exposeGutterControls,
    search({ top: true }),
    keymap.of(searchKeymap),
    syntaxHighlighting(SYNTAX),
    language,
    diffTheme,
  ];
}

/** Resolves the language for a path lazily; null when none matches. */
export async function loadLanguage(path: string | null | undefined) {
  if (!path) return null;
  const { languages } = await import("@codemirror/language-data");
  const description = LanguageDescription.matchFilename(languages, path);
  if (!description) return null;
  return description.load();
}
