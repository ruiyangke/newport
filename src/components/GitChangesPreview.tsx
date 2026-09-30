import {
  Suspense,
  lazy,
  useCallback,
  useContext,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { ChevronLeft } from "lucide-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { GitDiff, GitStatus } from "../domain/gitResponses";
import type { GitReadRequest, GitWriteAction } from "../domain/git";
import { gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button, Select, SelectItem } from "./controls";
import { Modal } from "./Editors";
import { GitConflictControls } from "./GitConflictControls";
import { GitChangesPaneContext } from "./diff/GitChangesSplit";
import {
  selectionPayload,
  toggleSelection,
  type DiffFile,
} from "./diff/diffModel";
import { useCurrentGitState } from "../state/git";
import "./changesPreview.css";
import { GitNotice } from "./GitNotice";
import { DiffStat, GitDiffSettings } from "./diff/GitDiffSettings";
import { GitStatusBadge, type GitStatusMark } from "./GitStatusBadge";
import { ChangePath } from "./GitChangeList";

/** CodeMirror arrives with the first diff, not with the app. */
const GitDiffEditor = lazy(() => import("./diff/GitDiffEditor"));

type Side = Extract<GitReadRequest, { method: "repo.diff" }>["params"]["side"];
type Entry = GitStatus["entries"][number];
/** Staged and unstaged diffs answer different questions, so the header says
    which one is on screen in words instead of leaving it to be inferred. */
const COMPARISONS: Record<Side, string> = {
  head_to_index: "Staged changes: index compared with HEAD",
  index_to_worktree: "Unstaged changes: working tree compared with the index",
  head_to_worktree: "All changes: working tree compared with HEAD",
};
function messageOf(error: unknown) {
  return error && typeof error === "object" && "message" in error
    ? String(error.message)
    : String(error);
}

export function GitChangesPreview({
  repoId,
  snapshot,
  entry,
  preferredSide,
  disabled = false,
  blockedReason,
  conflictBlockedReason,
  mark,
  actions,
  onAction,
}: {
  repoId: string;
  snapshot: string;
  entry: Entry;
  /** The letter the list showed for this file in the group it was chosen in. */
  mark?: GitStatusMark;
  /** Whole-file actions (stage, unstage, discard), drawn in this header. */
  actions?: ReactNode;
  /** The comparison the caller's selection implies, when it is available. */
  preferredSide?: Side;
  disabled?: boolean;
  blockedReason?: string;
  /** Resolving a conflict is not blocked by the conflict itself. */
  conflictBlockedReason?: string;
  onAction?: (action: GitWriteAction, snapshot: string) => Promise<boolean>;
}) {
  const choices: { value: Side; label: string }[] = [];
  if (entry.staged) choices.push({ value: "head_to_index", label: "Staged" });
  if (entry.unstaged || entry.untracked)
    choices.push({ value: "index_to_worktree", label: "Unstaged" });
  if (!choices.length)
    choices.push({ value: "head_to_worktree", label: "Working tree" });
  const [selectedSide, setSide] = useState<Side | null>(null);
  const requested = selectedSide ?? preferredSide ?? null;
  const side = choices.some((choice) => choice.value === requested)
    ? requested!
    : choices[0].value;
  const scope = useCurrentServerScope();
  const queryClient = useQueryClient();
  const [applying, setApplying] = useState(false);
  // Smaller context splits hunks that sit close together, so it is the coarse
  // control that complements per-line selection.
  const [context, setContext] = useState(3);
  const [confirm, setConfirm] = useState<{
    ids: string[];
    lines?: string[];
  } | null>(null);
  const pending = useRef(false);
  const rootRef = useRef<HTMLElement>(null);
  // The list/diff split, and the single-pane flow at narrow widths, belong to
  // the GitChangesSplit around this preview; it is read here only to offer the
  // way back to the list.
  const split = useContext(GitChangesPaneContext);
  const narrow = split?.narrow ?? false;
  const showList = () => split?.setPane("list");
  const queryKey = JSON.stringify([
    repoId,
    snapshot,
    entry.entryId,
    side,
    context,
  ]);
  // A working diff is fixed by the snapshot it was read at, so a new snapshot
  // is a new read and a late answer for an old one lands under its own key.
  const read = gitQueries.diff(scope, {
    repoId,
    snapshot,
    entryId: entry.entryId,
    side,
    contextLines: context,
  });
  const diff = useQuery({ ...read, enabled: !entry.conflicted });
  // After a write that changed the file, the hunks on screen are spent: the
  // diff stays hidden until the next snapshot brings the next read.
  const [spent, setSpent] = useState<string | null>(null);
  const currentDiff = spent === queryKey ? null : (diff.data ?? null);
  // A failed write is shown against the diff it was made from.
  const [writeError, setWriteError] = useState<{
    key: string;
    message: string;
  } | null>(null);
  const error =
    (writeError?.key === queryKey ? writeError.message : "") ||
    (diff.isError && !diff.isFetching ? messageOf(diff.error) : "");
  async function applySelection(ids: string[], lines?: string[]) {
    if (
      !onAction ||
      disabled ||
      blockedReason ||
      entry.conflicted ||
      pending.current ||
      !currentDiff ||
      !ids.length
    )
      return;
    pending.current = true;
    setApplying(true);
    try {
      const changed = await onAction(
        {
          kind: side === "head_to_index" ? "unstage" : "stage",
          entryIds: [entry.entryId],
          hunks: { ids, ...(lines ? { lines } : {}), contextLines: context },
        },
        snapshot,
      );
      if (changed) setSpent(queryKey);
    } catch (reason) {
      setWriteError({
        key: queryKey,
        message: reason instanceof Error ? reason.message : String(reason),
      });
    } finally {
      pending.current = false;
      setApplying(false);
    }
  }
  async function discardSelection(ids: string[], lines?: string[]) {
    if (
      !onAction ||
      disabled ||
      blockedReason ||
      entry.conflicted ||
      pending.current ||
      !currentDiff ||
      !ids.length
    )
      return;
    pending.current = true;
    setApplying(true);
    try {
      const changed = await onAction(
        {
          kind: "discard",
          entryIds: [entry.entryId],
          source: "index",
          hunks: { ids, ...(lines ? { lines } : {}), contextLines: context },
        },
        snapshot,
      );
      if (changed) setSpent(queryKey);
    } catch (reason) {
      setWriteError({
        key: queryKey,
        message: reason instanceof Error ? reason.message : String(reason),
      });
    } finally {
      pending.current = false;
      setApplying(false);
      setConfirm(null);
    }
  }
  const title =
    entry.path?.display ?? entry.oldPath?.display ?? "Selected file";
  const stat = currentDiff?.files.reduce(
    (total, file) => ({
      additions: total.additions + file.additions,
      deletions: total.deletions + file.deletions,
    }),
    { additions: 0, deletions: 0 },
  );
  return (
    <section
      ref={rootRef}
      className="git-diff-preview flex h-full min-h-0 flex-col"
      data-pane={narrow ? "diff" : undefined}
      aria-label={`Diff of ${title}`}
    >
      {/* One header for the whole diff: which file, how much changed, which
          comparison (in words, and switchable when the file has both), what
          can be done to the file, and how the diff is drawn. It replaces a
          stack of three bands -- actions, selects, counts -- above the code. */}
      <header className="git-diff-toolbar flex min-h-[46px] flex-none flex-wrap items-center gap-x-[8px] gap-y-[6px] border-b border-border px-[12px] py-[6px]">
        {narrow && (
          <Button
            variant="ghost"
            size="icon"
            aria-label="Back to changes"
            className="git-diff-back -ml-[4px] h-[26px]! w-[26px] flex-none"
            onClick={showList}
          >
            <ChevronLeft size={16} aria-hidden="true" />
          </Button>
        )}
        {mark && <GitStatusBadge mark={mark} />}
        <div className="flex min-w-[160px] flex-[1_1_160px] flex-col gap-[1px]">
          <div className="flex min-w-0 items-baseline gap-[8px]">
            <ChangePath
              path={title}
              className="text-[13px] leading-[18px] font-medium"
            />
            {stat && (
              <DiffStat additions={stat.additions} deletions={stat.deletions} />
            )}
          </div>
          {!entry.conflicted && (
            <p className="git-diff-comparison truncate text-[11px] leading-[15px] text-muted-foreground">
              {COMPARISONS[side]}
            </p>
          )}
        </div>
        {!entry.conflicted && choices.length > 1 && (
          <Select
            aria-label="Diff comparison"
            className="h-[26px]! w-auto min-w-[104px] flex-none"
            value={side}
            onValueChange={(value) => setSide(value as Side)}
          >
            {choices.map((choice) => (
              <SelectItem key={choice.value} value={choice.value}>
                {choice.label}
              </SelectItem>
            ))}
          </Select>
        )}
        {actions}
        {!entry.conflicted && (
          <GitDiffSettings context={context} onContext={setContext} />
        )}
      </header>
      {entry.conflicted && (
        <GitConflictControls
          repoId={repoId}
          snapshot={snapshot}
          entry={entry}
          disabled={disabled}
          blockedReason={conflictBlockedReason}
          onAction={onAction}
        />
      )}
      {blockedReason && !entry.conflicted && (
        <GitNotice tone="info">{blockedReason}</GitNotice>
      )}
      {entry.conflicted ? null : error ? (
        <GitNotice tone="error">
          {error}
          <Button
            onClick={() => {
              setWriteError(null);
              // Read again from scratch, so the answer that failed to apply
              // is not on screen while it is reread.
              void queryClient.resetQueries({
                queryKey: read.queryKey,
                exact: true,
              });
            }}
          >
            Retry diff
          </Button>
        </GitNotice>
      ) : !currentDiff ? (
        <p className="git-projects-empty" role="status">
          Loading diff…
        </p>
      ) : (
        <GitDiffView
          key={queryKey}
          diff={currentDiff}
          hunkAction={
            onAction && !entry.conflicted && side !== "head_to_worktree"
              ? {
                  label:
                    side === "head_to_index" ? "Unstage hunk" : "Stage hunk",
                  lineLabel: side === "head_to_index" ? "Unstage" : "Stage",
                  disabled: disabled || !!blockedReason || applying,
                  onApply: (id) => applySelection([id]),
                  onApplyLines: (ids, lines) => applySelection(ids, lines),
                  onDiscard:
                    side === "index_to_worktree"
                      ? (ids, lines) => setConfirm({ ids, lines })
                      : undefined,
                }
              : undefined
          }
        />
      )}
      {confirm && (
        <Modal
          title={
            confirm.lines
              ? `Discard ${confirm.lines.length} ${confirm.lines.length === 1 ? "line" : "lines"}?`
              : "Discard this hunk?"
          }
          busy={applying}
          onClose={() => setConfirm(null)}
        >
          <p>
            The selected changes are removed from the working file on the
            server. This cannot be undone, and nothing else in the file, the
            index or other files is changed.
          </p>
          <div className="editor-actions">
            <Button disabled={applying} onClick={() => setConfirm(null)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              disabled={applying}
              onClick={() => void discardSelection(confirm.ids, confirm.lines)}
            >
              Discard
            </Button>
          </div>
        </Modal>
      )}
    </section>
  );
}

type HunkAction = {
  label: string;
  lineLabel: string;
  disabled: boolean;
  onApply: (id: string) => Promise<void>;
  onApplyLines: (ids: string[], lines: string[]) => Promise<void>;
  /** Present only where discarding has a defined meaning: the unstaged side. */
  onDiscard?: (ids: string[], lines?: string[]) => void;
};
const TEXT_MODES = [0o100644, 0o100755];
/** Whether git's hunks for this file can be staged one by one. */
function hunksSupported(diff: GitDiff, file: DiffFile) {
  // An added file has no old side and a deleted file has no new side, so each
  // status checks only the mode that exists.
  const modes =
    file.status === "Modified"
      ? file.oldMode === file.newMode && TEXT_MODES.includes(file.oldMode)
      : file.status === "Added" || file.status === "Untracked"
        ? TEXT_MODES.includes(file.newMode)
        : file.status === "Deleted"
          ? TEXT_MODES.includes(file.oldMode)
          : false;
  return (
    !diff.truncated &&
    diff.files.length === 1 &&
    !file.binary &&
    modes &&
    file.oldPath?.bytesB64 === file.newPath?.bytesB64 &&
    file.hunks.length > 0 &&
    file.hunks.every((hunk) => !!hunk.id)
  );
}
export function GitDiffView({
  diff,
  hunkAction,
}: {
  diff: GitDiff;
  hunkAction?: HunkAction;
}) {
  const [fileIndex, setFileIndex] = useState(0);
  const [selected, setSelected] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  // Chosen in the review bar's Diff Settings, not here: the design keeps the
  // diff's settings with the file's name, above both panes.
  const [layout] = useCurrentGitState("diffLayout");
  const toggle = useCallback(
    (ids: string[], checked: boolean) =>
      setSelected((current) => toggleSelection(current, ids, checked)),
    [],
  );
  const onApply = hunkAction?.onApply;
  const onDiscard = hunkAction?.onDiscard;
  const applyHunk = useCallback((id: string) => void onApply?.(id), [onApply]);
  const discardHunk = useCallback(
    (id: string) => onDiscard?.([id]),
    [onDiscard],
  );
  const file = diff.files[fileIndex];
  if (!file)
    return (
      <p className="git-projects-empty">No differences in this comparison.</p>
    );
  const supportsHunks = hunksSupported(diff, file);
  const action = supportsHunks ? hunkAction : undefined;
  // Exactly what a selection sends: the agent's line ids in the order they
  // were picked, and the agent's ids of the hunks that hold them.
  const payload = selectionPayload(file, selected);
  const picked = payload.lines;
  // Selecting an addition without its paired deletion keeps both lines.
  const mixed = file.hunks.some(
    (hunk) =>
      hunk.lines.some(
        (line) => line.id && selected.has(line.id) && line.origin === "+",
      ) &&
      hunk.lines.some(
        (line) => line.id && !selected.has(line.id) && line.origin === "-",
      ),
  );
  const hasLines = file.hunks.some((hunk) => hunk.lines.length > 0);
  return (
    <>
      {diff.truncated && (
        <GitNotice tone="status">
          The diff exceeds the agent’s limit. Some changes are not shown.
        </GitNotice>
      )}
      {diff.files.length > 1 && (
        <Select
          aria-label="Diff file"
          value={String(fileIndex)}
          onValueChange={(value) => {
            setFileIndex(Number(value));
            setSelected(new Set());
          }}
        >
          {diff.files.map((entry, index) => (
            <SelectItem key={index} value={String(index)}>
              {entry.newPath?.display ??
                entry.oldPath?.display ??
                `File ${index + 1}`}
            </SelectItem>
          ))}
        </Select>
      )}
      {/* Status and counts are in the header that names the file; what stays
          here is only what the header cannot say in passing. */}
      {/* A permission change, when there is one. A file that is new or gone
          has no mode on one side, and "Mode 0 → 100644" says nothing the
          status letter did not. */}
      {file.oldMode !== 0 &&
        file.newMode !== 0 &&
        file.oldMode !== file.newMode && (
          <p className="git-diff-summary border-b border-border px-[12px] py-[5px] text-[11px] text-muted-foreground">
            Mode {file.oldMode.toString(8)} → {file.newMode.toString(8)}
          </p>
        )}
      {file.oldPath &&
        file.newPath &&
        file.oldPath.bytesB64 !== file.newPath.bytesB64 && (
          <p className="git-diff-rename border-b border-border px-[12px] py-[5px] text-[11px] text-muted-foreground">
            Renamed from {file.oldPath.display}
          </p>
        )}
      {hunkAction && !supportsHunks && (
        <GitNotice tone="info">
          Individual hunks are unavailable for this comparison. Use the file
          actions in the header.
        </GitNotice>
      )}
      {action && picked.length > 0 && (
        <div
          className="git-diff-selection"
          role="group"
          aria-label="Selected lines"
        >
          <span>
            {picked.length} {picked.length === 1 ? "line" : "lines"} selected
          </span>
          {/* The three actions travel together; a bar too narrow for the note
              beside them wraps the note, not the last button away from them. */}
          <div className="git-diff-selection-actions">
            <Button
              size="sm"
              disabled={action.disabled}
              onClick={() => void action.onApplyLines(payload.hunks, picked)}
            >
              {action.lineLabel} {picked.length}{" "}
              {picked.length === 1 ? "line" : "lines"}
            </Button>
            {action.onDiscard && (
              <Button
                size="sm"
                variant="destructive"
                disabled={action.disabled}
                onClick={() => action.onDiscard!(payload.hunks, picked)}
              >
                Discard {picked.length} {picked.length === 1 ? "line" : "lines"}
              </Button>
            )}
            <Button size="sm" onClick={() => setSelected(new Set())}>
              Clear
            </Button>
          </div>
          {mixed && (
            <span className="git-diff-selection-note">
              Deletions you did not select stay in the file.
            </span>
          )}
        </div>
      )}
      {file.binary ? (
        <p className="git-projects-empty">
          Binary file changed. A text diff is not available.
        </p>
      ) : !hasLines ? (
        <p className="git-projects-empty">No text changes to display.</p>
      ) : (
        <Suspense
          fallback={
            <p className="git-projects-empty" role="status">
              Preparing the diff view…
            </p>
          }
        >
          <GitDiffEditor
            file={file}
            layout={layout}
            hunkLabel={action?.label}
            lineLabel={action?.lineLabel}
            canDiscard={!!action?.onDiscard}
            selected={selected}
            disabled={!!action?.disabled}
            onToggle={toggle}
            onApplyHunk={applyHunk}
            onDiscardHunk={discardHunk}
          />
        </Suspense>
      )}
    </>
  );
}
