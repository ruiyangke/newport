/**
 * The diff's code area: read-only CodeMirror views of the agent's hunks.
 *
 * Loaded lazily with the diff pane, so CodeMirror stays out of the main
 * bundle. The views display the agent's lines and never compute a diff; every
 * selection they report is a list of the agent's line ids.
 */
import { useEffect, useMemo, useRef } from "react";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import {
  buildSplit,
  buildUnified,
  type DiffFile,
  type SideModel,
} from "./diffModel";
import {
  diffExtensions,
  loadLanguage,
  setDiffUi,
  type DiffControls,
  type DiffHandlers,
  type DiffUi,
} from "./diffExtensions";

export interface GitDiffEditorProps {
  file: DiffFile;
  layout: "unified" | "split";
  /** Present only where the comparison can stage lines and hunks. */
  hunkLabel?: string;
  lineLabel?: string;
  canDiscard?: boolean;
  selected: ReadonlySet<string>;
  disabled: boolean;
  onToggle: (ids: string[], checked: boolean) => void;
  onApplyHunk: (hunkId: string) => void;
  onDiscardHunk?: (hunkId: string) => void;
}

type EditorSetup = {
  model: SideModel;
  path: string | null;
  controls: DiffControls | null;
  hunkActions: boolean;
  label: string;
  handlers: { current: DiffHandlers };
  ui: DiffUi;
};

/** One CodeMirror view bound to a side model; rebuilt only when the model is. */
function useDiffEditor({
  model,
  path,
  controls,
  hunkActions,
  label,
  handlers,
  ui,
}: EditorSetup) {
  const parent = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  // The selection at the moment the view is built; later changes are
  // dispatched into the existing view instead of rebuilding it.
  const initialUi = useRef(ui);
  useEffect(() => {
    initialUi.current = ui;
  }, [ui]);
  const empty = model.lines.length === 0;
  useEffect(() => {
    const host = parent.current;
    if (!host || empty) return;
    const language = new Compartment();
    const editor = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: model.doc,
        extensions: diffExtensions(
          { model, controls, hunkActions, handlers, label },
          language.of([]),
          initialUi.current,
        ),
      }),
    });
    view.current = editor;
    let live = true;
    // Highlighting arrives when it arrives; the diff is already on screen.
    loadLanguage(path)
      .then((support) => {
        if (live && support)
          editor.dispatch({ effects: language.reconfigure(support) });
      })
      .catch(() => {});
    return () => {
      live = false;
      view.current = null;
      editor.destroy();
    };
  }, [model, path, controls, hunkActions, label, handlers, empty]);
  useEffect(() => {
    view.current?.dispatch({ effects: setDiffUi.of(ui) });
  }, [ui]);
  return { parent, view };
}

export default function GitDiffEditor({
  file,
  layout,
  hunkLabel,
  lineLabel,
  canDiscard = false,
  selected,
  disabled,
  onToggle,
  onApplyHunk,
  onDiscardHunk,
}: GitDiffEditorProps) {
  const controls = useMemo<DiffControls | null>(
    () =>
      hunkLabel && lineLabel ? { hunkLabel, lineLabel, canDiscard } : null,
    [hunkLabel, lineLabel, canDiscard],
  );
  // Callbacks are read through a ref, so a re-render never rebuilds a view.
  const handlers = useRef<DiffHandlers>({
    toggle: onToggle,
    applyHunk: onApplyHunk,
    discardHunk: onDiscardHunk,
  });
  useEffect(() => {
    handlers.current = {
      toggle: onToggle,
      applyHunk: onApplyHunk,
      discardHunk: onDiscardHunk,
    };
  }, [onToggle, onApplyHunk, onDiscardHunk]);
  const ui = useMemo(() => ({ selected, disabled }), [selected, disabled]);
  return layout === "split" ? (
    <SplitDiff file={file} controls={controls} handlers={handlers} ui={ui} />
  ) : (
    <UnifiedDiff file={file} controls={controls} handlers={handlers} ui={ui} />
  );
}

type ViewProps = {
  file: DiffFile;
  controls: DiffControls | null;
  handlers: { current: DiffHandlers };
  ui: DiffUi;
};
const pathOf = (file: DiffFile, side: "old" | "new") =>
  side === "old"
    ? (file.oldPath?.display ?? file.newPath?.display ?? null)
    : (file.newPath?.display ?? file.oldPath?.display ?? null);

function UnifiedDiff({ file, controls, handlers, ui }: ViewProps) {
  const model = useMemo(() => buildUnified(file, !!controls), [file, controls]);
  const { parent } = useDiffEditor({
    model,
    path: pathOf(file, "new"),
    controls,
    hunkActions: true,
    label: "Diff lines",
    handlers,
    ui,
  });
  // `.git-diff-code` is the scroll box in both directions: the editor grows to
  // its content, so a long line scrolls this box rather than widening the pane.
  return (
    <div className="git-diff-code" role="region" aria-label="Diff lines">
      <div ref={parent} data-diff-layout="unified" />
    </div>
  );
}

function SplitDiff({ file, controls, handlers, ui }: ViewProps) {
  const models = useMemo(() => buildSplit(file, !!controls), [file, controls]);
  const before = useDiffEditor({
    model: models.old,
    path: pathOf(file, "old"),
    controls,
    hunkActions: false,
    label: "Old version",
    handlers,
    ui,
  });
  const after = useDiffEditor({
    model: models.new,
    path: pathOf(file, "new"),
    controls,
    hunkActions: true,
    label: "New version",
    handlers,
    ui,
  });
  // Both sides sit in one vertical scroll box, so they scroll vertically
  // together by construction. Each scrolls sideways on its own; this keeps
  // the two in step.
  useEffect(() => {
    const a = before.view.current?.scrollDOM;
    const b = after.view.current?.scrollDOM;
    if (!a || !b) return;
    let echo: Element | null = null;
    const follow = (from: HTMLElement, to: HTMLElement) => () => {
      if (echo === from) {
        echo = null;
        return;
      }
      if (to.scrollLeft === from.scrollLeft) return;
      echo = to;
      to.scrollLeft = from.scrollLeft;
    };
    const onA = follow(a, b);
    const onB = follow(b, a);
    a.addEventListener("scroll", onA, { passive: true });
    b.addEventListener("scroll", onB, { passive: true });
    return () => {
      a.removeEventListener("scroll", onA);
      b.removeEventListener("scroll", onB);
    };
  }, [before.view, after.view, models]);
  return (
    <div className="git-diff-code" role="region" aria-label="Diff lines">
      <div
        className="grid grid-cols-[minmax(0,1fr)_8px_minmax(0,1fr)]"
        data-diff-layout="split"
      >
        <SideHost
          parentRef={before.parent}
          empty={!models.old.lines.length}
          note="Not in the old version"
        />
        <div
          aria-hidden="true"
          className="bg-[rgb(225,228,233)] dark:bg-[rgb(60,59,64)]"
        />
        <SideHost
          parentRef={after.parent}
          empty={!models.new.lines.length}
          note="Not in the new version"
        />
      </div>
    </div>
  );
}

function SideHost({
  parentRef,
  empty,
  note,
}: {
  parentRef: React.RefObject<HTMLDivElement | null>;
  empty: boolean;
  note: string;
}) {
  // A side with no lines at all (an added or deleted file) has nothing to
  // align, and an empty editor would still draw one blank line.
  return empty ? (
    <p className="m-0! min-w-0 px-[12px] py-[4px] text-[12px] text-muted-foreground">
      {note}
    </p>
  ) : (
    <div ref={parentRef} className="min-w-0" />
  );
}
