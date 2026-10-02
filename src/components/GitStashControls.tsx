import {
  GitInspectorToolbar,
  GitInspectorSearch,
  GitInspectorList,
  GitInspectorRow,
} from "./GitInspectorPrimitives";
import {
  GitError,
  GitForm,
  GitField,
  GitCheckboxField,
  GitFormActions,
  GitFormFields,
} from "./GitForm";
import { GitEmptyState } from "./GitEmptyState";
import { Archive, Plus, Search } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { GitWriteAction } from "../domain/git";
import { type GitStashes } from "../domain/gitResponses";
import { gitKeys, gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button, Input } from "./controls";
import { Checkbox } from "./ui/checkbox";
import { GitInspectorSection } from "./GitInspectorSection";
import { GitCommitInspector, relativeTime } from "./GitCommitInspector";
import { useGitPageLoader } from "../hooks/useGitPageLoader";
import { GitLoadMore } from "./GitLoadMore";
import { gitProjectsFor } from "../git/registry";
import { gitErrorMessage } from "../git/errors";
import { plural } from "../lib/utils";

/**
 * A segmented choice on its rail: the pressed chip is raised, the other sits
 * flush. Neither fill holds under the pointer, where the outline button's own
 * hover fill shows through as it always did.
 */
const segment =
  "aria-pressed:font-semibold! aria-pressed:[box-shadow:0_1px_3px_rgb(24_34_55/8.6%)] aria-pressed:[&:not(:hover)]:bg-background! aria-[pressed=false]:border-transparent! aria-[pressed=false]:[&:not(:hover)]:bg-transparent!";
type Stash = GitStashes["entries"][number];
type Props = {
  repoId: string;
  projectName: string;
  snapshot?: string;
  busy: boolean;
  blockedReason?: string;
  /** Controlled disclosure, so a shared actions menu can open this dialog. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  hideTrigger?: boolean;
  error: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
};
export function GitStashControls({
  open: controlledOpen,
  onOpenChange,
  hideTrigger = false,
  ...props
}: Props) {
  const [selfOpen, setSelfOpen] = useState(false);
  const open = controlledOpen ?? selfOpen;
  const setOpen = onOpenChange ?? setSelfOpen;
  return (
    <>
      {!hideTrigger && (
        <Button
          disabled={props.busy}
          onClick={() => {
            setOpen(true);
          }}
        >
          Stashes
        </Button>
      )}
      {/* Mounted per disclosure, whether opened by the trigger or the actions
          menu, so every opening starts clean and reads the list afresh. */}
      {open && <StashDialog {...props} onClose={() => setOpen(false)} />}
    </>
  );
}

function StashDialog({
  repoId,
  projectName,
  snapshot,
  busy,
  blockedReason,
  error,
  onAction,
  onClose,
}: Omit<Props, "open" | "onOpenChange" | "hideTrigger"> & {
  onClose: () => void;
}) {
  const scope = useCurrentServerScope();
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState<Stash | null>(null);
  const [message, setMessage] = useState("");
  const [filter, setFilter] = useState("");
  const [includeUntracked, setIncludeUntracked] = useState(false);
  const [keepIndex, setKeepIndex] = useState(false);
  const [reinstateIndex, setReinstateIndex] = useState(false);
  const [confirm, setConfirm] = useState<"apply" | "pop" | "drop" | null>(null);
  const [saving, setSaving] = useState(false);
  const [showUntracked, setShowUntracked] = useState(false);
  const first = useQuery({
    ...gitQueries.stashes(scope, repoId),
    refetchOnMount: "always",
  });
  const stashes = first.isFetchedAfterMount ? first.data : null;
  const loading = first.isFetching;
  const listError =
    first.isError && !loading ? gitErrorMessage(first.error) : "";
  const pages = useGitPageLoader({
    queryKey: gitQueries.stashes(scope, repoId).queryKey,
    page: stashes ?? null,
    enabled: !loading && !first.isError && !saving,
    prefetch: false,
    entryKey: (stash: Stash) => `${stash.index}:${stash.oid}`,
    read: (cursor, signal) =>
      gitProjectsFor(scope)
        .repositories.withSignal(signal)
        .stashes(repoId, cursor),
  });
  // A new snapshot means the stash list may have changed under the dialog.
  const seen = useRef(snapshot);
  useEffect(() => {
    if (seen.current === snapshot) return;
    seen.current = snapshot;
    setSelected(null);
    setConfirm(null);
    void queryClient.invalidateQueries(
      { queryKey: [...gitKeys.repo(scope, repoId), "stashes"] },
      { cancelRefetch: false },
    );
  }, [snapshot, queryClient, scope, repoId]);
  // A stash is a commit whose second parent is the saved index and whose
  // third, when present, holds the untracked files.
  const tracked = useQuery({
    ...gitQueries.commit(scope, repoId, selected?.oid ?? ""),
    enabled: !!selected,
  });
  const trackedCommit = tracked.data;
  const trackedMatches = !!selected && trackedCommit?.oid.hex === selected.oid;
  const untrackedOid = trackedMatches
    ? trackedCommit!.parents[2]?.hex
    : undefined;
  const untracked = useQuery({
    ...gitQueries.commit(scope, repoId, untrackedOid ?? ""),
    enabled: !!untrackedOid && showUntracked,
  });
  const untrackedCommit = untracked.data;
  const untrackedMatches =
    !!untrackedOid && untrackedCommit?.oid.hex === untrackedOid;
  const previewLoading =
    !!selected &&
    (tracked.isFetching ||
      (showUntracked && !!untrackedOid && untracked.isFetching));
  const previewError = !selected
    ? ""
    : tracked.isError && !tracked.isFetching
      ? gitErrorMessage(tracked.error)
      : tracked.data && !trackedMatches
        ? String(new Error("The selected stash could not be inspected."))
        : showUntracked && untracked.isError && !untracked.isFetching
          ? gitErrorMessage(untracked.error)
          : showUntracked && untracked.data && !untrackedMatches
            ? String(
                new Error(
                  "The stash’s untracked files could not be inspected.",
                ),
              )
            : "";
  const preview = trackedMatches
    ? {
        tracked: trackedCommit!,
        untracked: untrackedOid ? untrackedCommit! : null,
      }
    : null;
  const readError = selected ? previewError : listError;
  const disabled = busy || loading || previewLoading || !!blockedReason;
  async function submit(action: GitWriteAction) {
    if (disabled) return;
    if (await onAction(action)) {
      onClose();
      setConfirm(null);
      setSelected(null);
    }
  }
  return (
    <GitInspectorSection
      title={
        saving
          ? "Save stash"
          : confirm === "drop"
            ? "Drop stash"
            : confirm === "pop"
              ? "Pop stash"
              : confirm === "apply"
                ? "Apply stash"
                : "Stashes"
      }
      busy={busy}
      onClose={onClose}
      fill
    >
      <div
        data-slot="git-stash-workspace"
        className="flex min-h-0 max-w-none flex-1 p-0"
      >
        <aside
          data-slot="git-stash-sidebar"
          className="min-h-0 w-[280px] flex-[0_0_280px] overflow-y-auto border-r border-border p-[8px] @max-[700px]/git-project:w-[210px] @max-[700px]/git-project:basis-[210px]"
          aria-label="Saved stashes"
          data-git-scroll-root
        >
          <GitInspectorToolbar className="px-[4px] pt-[4px] pb-[8px]">
            <span className="text-[11px] font-normal text-muted-foreground">
              {plural(stashes?.entries.length ?? 0, "stash")}
              {stashes?.nextCursor ? " loaded" : ""}
            </span>
            <Button
              disabled={disabled}
              onClick={() => {
                setMessage("");
                setIncludeUntracked(false);
                setKeepIndex(false);
                setConfirm(null);
                setSaving(true);
              }}
            >
              <Plus size={13} aria-hidden="true" /> Save changes…
            </Button>
          </GitInspectorToolbar>
          <GitInspectorSearch className="mb-[8px]">
            <Search size={13} aria-hidden="true" />
            <Input
              aria-label="Filter loaded stashes"
              placeholder="Filter loaded stashes"
              value={filter}
              onChange={(event) => setFilter(event.target.value)}
            />
          </GitInspectorSearch>
          {stashes?.entries.length === 0 && <p>No saved stashes.</p>}
          {filter &&
            stashes &&
            !stashes.entries.some((stash) =>
              stash.message.toLowerCase().includes(filter.toLowerCase()),
            ) && <p>No matching loaded stashes.</p>}
          <GitInspectorList className="git-stash-list">
            {stashes?.entries
              .filter((stash) =>
                stash.message.toLowerCase().includes(filter.toLowerCase()),
              )
              .map((stash) => (
                <li key={`${stash.oid}:${stash.index}`} className="my-[2px]">
                  <GitInspectorRow
                    variant="ghost"

                    aria-pressed={selected?.oid === stash.oid && !saving}
                    title={stash.message || "Unnamed stash"}
                    disabled={busy}
                    onClick={() => {
                      setSelected(stash);
                      setSaving(false);
                      setConfirm(null);
                      setShowUntracked(false);
                    }}
                  >
                    <Archive
                      size={14}
                      className="flex-none text-muted-foreground"
                      aria-hidden="true"
                    />
                    <span className="grid min-w-0 flex-1 gap-[3px]">
                      <span className="truncate text-[12px] font-medium">
                        {stash.message || "Unnamed stash"}
                        {stash.messageTruncated ? "…" : ""}
                      </span>
                      <span className="text-[11px] font-normal text-muted-foreground">
                        stash@{`{${stash.index}}`} ·{" "}
                        <time
                          title={new Date(stash.time * 1000).toLocaleString()}
                        >
                          {relativeTime(stash.time)}
                        </time>
                      </span>
                    </span>
                  </GitInspectorRow>
                </li>
              ))}
            {stashes && (
              <li className="list-none">
                <GitLoadMore
                  scrollOnly
                  cursor={stashes.nextCursor}
                  loading={pages.loading}
                  error={pages.error}
                  disabled={busy || loading || first.isError}
                  onLoad={pages.load}
                  label="Load more stashes"
                  endLabel="All stashes loaded"
                />
              </li>
            )}
          </GitInspectorList>
        </aside>
        <div
          data-slot="git-stash-detail"
          className="flex min-h-0 min-w-0 flex-1 flex-col overflow-auto [&>p]:m-[16px] [&>[data-slot=git-project-form]]:m-[16px] [&>.git-commit-inspector]:flex-1"
        >
          {(saving || confirm) && <p>{projectName}</p>}
          {error && <GitError role="alert">{error}</GitError>}
          {blockedReason && (
            <p>
              {blockedReason}{" "}
              <Button onClick={onClose}>Back to repository</Button>
            </p>
          )}
          {readError && (
            <GitError role="alert">
              {readError}{" "}
              <Button
                disabled={busy || loading}
                onClick={() => {
                  setSelected(null);
                  setConfirm(null);
                  void first.refetch();
                }}
              >
                Retry stashes
              </Button>
            </GitError>
          )}
          {(loading || previewLoading) && <p role="status">Loading stashes…</p>}
          {saving ? (
            <GitForm
              onSubmit={(event) => {
                event.preventDefault();
                void submit({
                  kind: "stash.save",
                  message,
                  includeUntracked,
                  keepIndex,
                });
              }}
            >
              <GitField>
                Stash message
                <Input
                  value={message}
                  onChange={(event) => setMessage(event.target.value)}
                  disabled={busy}
                  placeholder="Optional description"
                />
              </GitField>
              <GitCheckboxField>
                <Checkbox
                  checked={includeUntracked}
                  onCheckedChange={(value) =>
                    setIncludeUntracked(value === true)
                  }
                  disabled={busy}
                />
                Include untracked files
              </GitCheckboxField>
              <GitCheckboxField>
                <Checkbox
                  checked={keepIndex}
                  onCheckedChange={(value) => setKeepIndex(value === true)}
                  disabled={busy}
                />
                Keep staged changes in the working tree
              </GitCheckboxField>
              <p>
                {keepIndex
                  ? "Save changes while keeping staged changes in the working tree. Ignored files are kept."
                  : "Save changes and clean them from the working tree. Ignored files are kept."}
              </p>
              <GitFormActions>
                <Button disabled={busy} onClick={() => setSaving(false)}>
                  Back
                </Button>
                <Button type="submit" variant="default" disabled={disabled}>
                  Save stash
                </Button>
              </GitFormActions>
            </GitForm>
          ) : confirm && selected && stashes ? (
            <GitFormFields>
              <p>
                <strong>{selected.message || "Unnamed stash"}</strong> ·{" "}
                <code>{selected.oid.slice(0, 12)}</code>
              </p>
              <p>
                {confirm === "drop"
                  ? "Remove this stash from the stash list. You may lose the only saved copy of these changes."
                  : confirm === "pop"
                    ? "Apply this stash, then remove it only if it applies without conflicts."
                    : "Apply this stash to the working tree and keep it in the stash list."}
              </p>
              {confirm !== "drop" && (
                <GitCheckboxField>
                  <Checkbox
                    checked={reinstateIndex}
                    onCheckedChange={(value) =>
                      setReinstateIndex(value === true)
                    }
                    disabled={busy}
                  />
                  Restore which changes were staged
                </GitCheckboxField>
              )}
              <GitFormActions>
                <Button disabled={busy} onClick={() => setConfirm(null)}>
                  Cancel
                </Button>
                <Button
                  variant={confirm === "drop" ? "destructive" : "default"}
                  disabled={disabled}
                  onClick={() =>
                    void submit(
                      confirm === "drop"
                        ? {
                            kind: "stash.drop",
                            oid: selected.oid,
                            index: selected.index,
                            expectedToken: stashes.metadata.listToken,
                          }
                        : {
                            kind:
                              confirm === "pop" ? "stash.pop" : "stash.apply",
                            oid: selected.oid,
                            index: selected.index,
                            expectedToken: stashes.metadata.listToken,
                            reinstateIndex,
                          },
                    )
                  }
                >
                  {confirm === "drop"
                    ? "Drop stash"
                    : confirm === "pop"
                      ? "Pop stash"
                      : "Apply stash"}
                </Button>
              </GitFormActions>
            </GitFormFields>
          ) : selected ? (
            <>
              {preview && (
                <>
                  <div className="git-stash-tools flex flex-none flex-wrap gap-1 border-b border-border p-2">
                    <Button
                      className={segment}
                      aria-pressed={!showUntracked}
                      onClick={() => setShowUntracked(false)}
                    >
                      Tracked changes
                    </Button>
                    {untrackedOid && (
                      <Button
                        className={segment}
                        aria-pressed={showUntracked}
                        onClick={() => setShowUntracked(true)}
                      >
                        Untracked files
                      </Button>
                    )}
                  </div>
                  {(!showUntracked || preview.untracked) && (
                    <GitCommitInspector
                      key={
                        showUntracked
                          ? preview.untracked!.oid.hex
                          : preview.tracked.oid.hex
                      }
                      repoId={repoId}
                      actions={
                        <footer className="flex flex-wrap gap-2">
                          <Button
                            disabled={disabled}
                            onClick={() => {
                              setReinstateIndex(false);
                              setConfirm("apply");
                            }}
                          >
                            Apply…
                          </Button>
                          <Button
                            disabled={disabled}
                            onClick={() => {
                              setReinstateIndex(false);
                              setConfirm("pop");
                            }}
                          >
                            Pop…
                          </Button>
                          <Button
                            disabled={disabled}
                            onClick={() => setConfirm("drop")}
                          >
                            Drop…
                          </Button>
                        </footer>
                      }
                      commit={
                        showUntracked ? preview.untracked! : preview.tracked
                      }
                    />
                  )}
                </>
              )}
            </>
          ) : (
            <>
              <GitEmptyState>
                Select a stash to review its changes.
              </GitEmptyState>
            </>
          )}
        </div>
      </div>
    </GitInspectorSection>
  );
}
