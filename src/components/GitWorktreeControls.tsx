import { useState } from "react";
import { useQueries, useQueryClient } from "@tanstack/react-query";
import { gitPath, type GitPath, type GitWorktreeAction } from "../domain/git";
import {
  appendGitPage,
  type GitPage,
  type GitWorktrees,
  type GitRepository,
} from "../domain/gitResponses";
import { gitQueries, invalidateRepository } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button, Input, Checkbox } from "./controls";
import { Modal } from "./Editors";
import { Pagination, PaginationContent, PaginationItem } from "./ui/pagination";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";

type Worktree = GitWorktrees["entries"][number];
export type GitWorktreeEdit = Exclude<Editing, { kind: "add" }>;
type Editing =
  | { kind: "add" }
  | { kind: "lock" | "unlock" | "remove" | "prune" | "repair"; row: Worktree };
/** Every paragraph in the dialog body reads as secondary copy. */
const NOTE = "text-[12px] text-muted-foreground";
/** A worktree row's details sit a step below its name. */
const ROW_NOTE = `${NOTE} mt-[6px]!`;
function usable(path: GitPath | null) {
  try {
    return !!path && gitPath(path.display).bytesB64 === path.bytesB64;
  } catch {
    return false;
  }
}
type Read<T> = {
  data: T | undefined;
  error: unknown;
  isError: boolean;
  isFetching: boolean;
  isFetchedAfterMount: boolean;
};
/**
 * The page on screen, out of pages read one cursor at a time. A page is shown
 * once it and each page before it were read for this opening and each
 * continues the one before; until then the last such page stays, as it did
 * while the next was being fetched.
 */
function paged<T extends GitPage<unknown, unknown>>(
  reads: Read<T>[],
  cursors: string[],
  requested: number,
) {
  const pages: T[] = [];
  let failure = "";
  for (const [index, read] of reads.slice(0, requested + 1).entries()) {
    if (read.isError && !read.isFetching) {
      failure = String(read.error);
      break;
    }
    if (!read.data || !read.isFetchedAfterMount) break;
    if (index > 0)
      try {
        appendGitPage(pages[index - 1], read.data, cursors[index - 1]);
      } catch (error) {
        failure = String(error);
        break;
      }
    pages.push(read.data);
  }
  const index = Math.max(0, pages.length - 1);
  return {
    index,
    page: pages[index] as T | undefined,
    error: failure,
    loading: reads.slice(0, requested + 1).some((read) => read.isFetching),
  };
}
type Props = {
  repository: GitRepository;
  busy: boolean;
  blockedReason?: string;
  /** Controlled disclosure, so a shared actions menu can open this dialog. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  hideTrigger?: boolean;
  /** Opens straight into a row's form, as the worktree picker asks. */
  initial?: GitWorktreeEdit;
  error: string;
  onAction: (action: GitWorktreeAction, snapshot: string) => Promise<boolean>;
};
export function GitWorktreeControls({
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
        <Button disabled={props.busy} onClick={() => setOpen(true)}>
          Worktrees
        </Button>
      )}
      {/* Mounted per disclosure, whether opened by the trigger or the actions
          menu, so every opening starts clean and reads the list afresh. */}
      {open && <WorktreeDialog {...props} onClose={() => setOpen(false)} />}
    </>
  );
}

function WorktreeDialog({
  repository,
  busy,
  blockedReason,
  initial,
  error,
  onAction,
  onClose,
}: Omit<Props, "open" | "onOpenChange" | "hideTrigger"> & {
  onClose: () => void;
}) {
  const scope = useCurrentServerScope();
  const queryClient = useQueryClient();
  const repoId = repository.repoId;
  // Each page is its own read, keyed by its cursor: these are the cursors of
  // the pages after the first, and the page asked for.
  const [cursors, setCursors] = useState<string[]>([]);
  const [requested, setRequested] = useState(0);
  const [editing, setEditing] = useState<Editing | null>(initial ?? null);
  const [formError, setFormError] = useState("");
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [reason, setReason] = useState("");
  const [locked, setLocked] = useState(false);
  const [branchCursors, setBranchCursors] = useState<string[]>([]);
  const [branchRequested, setBranchRequested] = useState(0);
  // A branch choice belongs to the page it was made on.
  const [branchPick, setBranchPick] = useState<{
    page: number;
    name: string;
  } | null>(null);
  const worktreeReads = useQueries({
    queries: [undefined, ...cursors].map((cursor) => ({
      ...gitQueries.worktrees(scope, repoId, cursor),
      refetchOnMount: "always" as const,
    })),
  });
  // Branches are read only while adding, and afresh each time the form opens.
  const adding = editing?.kind === "add";
  const branchReads = useQueries({
    queries: adding
      ? [undefined, ...branchCursors].map((cursor) => ({
          ...gitQueries.branches(scope, repoId, cursor),
          refetchOnMount: "always" as const,
        }))
      : [],
  });
  const {
    index: pageIndex,
    page,
    error: listError,
    loading,
  } = paged(worktreeReads, cursors, requested);
  const {
    index: branchPageIndex,
    page: branchPage,
    error: branchError,
    loading: branchLoading,
  } = paged(branchReads, branchCursors, branchRequested);
  const readError = formError || listError || (adding ? branchError : "");
  const branchName =
    branchPick?.page === branchPageIndex ? branchPick.name : "";
  const setBranchName = (name: string) =>
    setBranchPick({ page: branchPageIndex, name });
  const branches =
    branchPage?.entries.filter(
      (branch) =>
        !branch.remote &&
        (!branch.current || repository.bare) &&
        branch.oid &&
        usable(branch.name),
    ) ?? [];
  const branch = branches.find((branch) => branch.name.bytesB64 === branchName);
  const disabled = busy || loading || !!blockedReason || !page;
  const titles = {
    add: "Add worktree",
    lock: "Lock worktree",
    unlock: "Unlock worktree",
    remove: "Remove worktree",
    prune: "Remove missing registration",
    repair: "Locate moved worktree",
  };
  /** Back to the first page, read afresh. */
  function restart() {
    setFormError("");
    setCursors([]);
    setRequested(0);
  }
  async function submit(action: GitWorktreeAction) {
    if (disabled || !page) return;
    await onAction(action, page.snapshot);
    setEditing(null);
    restart();
    void invalidateRepository(queryClient, scope, repoId);
  }
  function next(branches: boolean) {
    const selected = branches ? branchPage : page;
    if (!selected?.nextCursor || busy || (branches ? branchLoading : loading))
      return;
    const index = branches ? branchPageIndex : pageIndex;
    const known = branches ? branchCursors : cursors;
    const reads = branches ? branchReads : worktreeReads;
    setFormError("");
    // A page already visited is shown at once; one that failed is asked for
    // again; otherwise the next one is read.
    if (known[index] !== selected.nextCursor)
      (branches ? setBranchCursors : setCursors)([
        ...known.slice(0, index),
        selected.nextCursor,
      ]);
    else if (reads[index + 1]?.isError) void reads[index + 1].refetch();
    (branches ? setBranchRequested : setRequested)(index + 1);
  }
  function edit(kind: Exclude<Editing["kind"], "add">, row: Worktree) {
    setReason("");
    setPath("");
    setFormError("");
    // Stop asking for a page that has not arrived.
    setRequested(pageIndex);
    setEditing({ kind, row });
  }
  return (
    <Modal
      title={editing ? titles[editing.kind] : "Worktrees"}
      busy={busy}
      onClose={onClose}
      className="git-worktree-dialog w-[min(680px,calc(100vw_-_32px))]!"
    >
      <div className="git-worktree-body min-h-0 overflow-y-auto px-[24px] pt-[16px] pb-[24px] wrap-anywhere">
        {blockedReason && (
          <p className={NOTE} role="status">
            {blockedReason}
          </p>
        )}
        {(readError || error) && (
          <div role="alert">
            <p className={NOTE}>{readError || error}</p>
            <Button
              disabled={busy || loading}
              onClick={() => {
                setEditing(null);
                restart();
                void worktreeReads[0].refetch();
              }}
            >
              Refresh worktrees
            </Button>
          </div>
        )}
        {editing ? (
          <form
            className="git-project-form"
            onSubmit={(event) => {
              event.preventDefault();
              if (disabled) return;
              try {
                if (editing.kind === "add") {
                  if (!branch?.oid || branchLoading || !name.trim()) return;
                  if (!path.startsWith("/"))
                    throw new Error(
                      "Enter an absolute destination on the server.",
                    );
                  void submit({
                    kind: "worktree.add",
                    name: name.trim(),
                    path: gitPath(path),
                    branch: branch.name.display,
                    expectedOid: branch.oid.hex,
                    locked,
                  });
                } else {
                  if (!usable(editing.row.name)) return;
                  const selectedName = editing.row.name!.display;
                  if (editing.kind === "repair") {
                    if (!path.startsWith("/"))
                      throw new Error(
                        "Enter the moved worktree’s absolute path on the server.",
                      );
                    void submit({
                      kind: "worktree.repair",
                      name: selectedName,
                      path: gitPath(path),
                    });
                  } else if (editing.kind === "lock")
                    void submit({
                      kind: "worktree.lock",
                      name: selectedName,
                      ...(reason ? { reason } : {}),
                    });
                  else
                    void submit({
                      kind: `worktree.${editing.kind}`,
                      name: selectedName,
                    });
                }
              } catch (error) {
                setFormError(
                  error instanceof Error ? error.message : String(error),
                );
              }
            }}
          >
            {editing.kind === "add" ? (
              <>
                <p className={NOTE}>
                  Check out an existing local branch in a separate directory on
                  this server. Create a branch in Branches first if needed.
                </p>
                <label>
                  Worktree name
                  <Input
                    autoFocus
                    required
                    value={name}
                    onChange={(event) => setName(event.target.value)}
                    disabled={busy}
                  />
                </label>
                <label>
                  Destination on server
                  <Input
                    required
                    value={path}
                    onChange={(event) => setPath(event.target.value)}
                    disabled={busy}
                    placeholder="/home/user/projects/feature"
                  />
                  <small>
                    The parent directory must exist; the destination must not.
                  </small>
                </label>
                <label>
                  Local branch
                  <Select
                    value={branchName}
                    onValueChange={setBranchName}
                    disabled={busy || branchLoading}
                  >
                    <SelectTrigger aria-label="Local branch">
                      <SelectValue placeholder="Choose a branch" />
                    </SelectTrigger>
                    <SelectContent>
                      {branches.map((branch) => (
                        <SelectItem
                          key={branch.name.bytesB64}
                          value={branch.name.bytesB64}
                        >
                          {branch.name.display}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </label>
                {branchLoading && (
                  <p className={NOTE} role="status">
                    Loading branches…
                  </p>
                )}
                {branchPage && !branches.length && (
                  <p className={NOTE}>
                    No available local branches on this page.
                    {!repository.bare && " The current branch cannot be used."}
                  </p>
                )}
                {branchPage &&
                  (branchPageIndex > 0 || branchPage.nextCursor) && (
                    <Pagination
                      aria-label="Branch pages"
                      className="git-worktree-actions mx-0 my-[12px] flex flex-wrap items-center justify-normal gap-[8px]"
                    >
                      <PaginationContent className="flex-wrap gap-[8px]">
                        <PaginationItem>
                          <Button
                            disabled={
                              busy || branchLoading || branchPageIndex === 0
                            }
                            onClick={() =>
                              setBranchRequested(branchPageIndex - 1)
                            }
                          >
                            Previous branches
                          </Button>
                        </PaginationItem>
                        <PaginationItem>
                          <Button
                            disabled={
                              busy || branchLoading || !branchPage.nextCursor
                            }
                            onClick={() => next(true)}
                          >
                            Next branches
                          </Button>
                        </PaginationItem>
                      </PaginationContent>
                    </Pagination>
                  )}
                <label className="git-checkbox-row">
                  <Checkbox
                    checked={locked}
                    onCheckedChange={(value) => setLocked(value === true)}
                    disabled={busy}
                    aria-label="Lock against removal"
                  />
                  Lock against removal
                </label>
              </>
            ) : (
              <>
                <strong>{editing.row.name?.display}</strong>
                <p className={NOTE}>
                  {editing.row.path?.display ?? "Path unavailable"}
                </p>
                {editing.kind === "lock" && (
                  <>
                    <p className={NOTE}>
                      A lock protects this worktree from removal and pruning.
                    </p>
                    <label>
                      Reason (optional)
                      <Input
                        autoFocus
                        value={reason}
                        onChange={(event) => setReason(event.target.value)}
                        disabled={busy}
                      />
                    </label>
                  </>
                )}
                {editing.kind === "unlock" && (
                  <p className={NOTE}>
                    Allow this worktree to be removed or pruned again?
                  </p>
                )}
                {editing.kind === "remove" && (
                  <p className={NOTE}>
                    Remove this checkout directory and its registration? Git
                    will refuse if it contains local changes or untracked files.
                    The branch and its commits will remain.
                  </p>
                )}
                {editing.kind === "prune" && (
                  <p className={NOTE}>
                    Remove the registration for this missing checkout? If you
                    moved the folder, use Locate moved worktree instead. No
                    checkout files will be deleted.
                  </p>
                )}
                {editing.kind === "repair" && (
                  <>
                    <p className={NOTE}>
                      Select the folder after moving it. Its files and
                      uncommitted changes will be kept.
                    </p>
                    <label>
                      New location on server
                      <Input
                        autoFocus
                        required
                        value={path}
                        onChange={(event) => setPath(event.target.value)}
                        disabled={busy}
                      />
                    </label>
                  </>
                )}
              </>
            )}
            <footer>
              <Button disabled={busy} onClick={() => setEditing(null)}>
                Back
              </Button>
              <Button
                type="submit"
                loading={busy}
                disabled={
                  disabled ||
                  (editing.kind === "add" &&
                    (branchLoading || !branch || !name.trim() || !path)) ||
                  (editing.kind === "repair" && !path)
                }
              >
                {titles[editing.kind]}
              </Button>
            </footer>
          </form>
        ) : (
          <>
            <div className="git-worktree-toolbar mx-0 my-[12px] flex flex-wrap items-center gap-[8px]">
              <p className={`${NOTE} flex-1`}>
                Separate checkouts sharing this repository’s branches and
                history.
              </p>
              <Button
                disabled={disabled}
                onClick={() => {
                  setName("");
                  setPath("");
                  setLocked(false);
                  setBranchCursors([]);
                  setBranchRequested(0);
                  setBranchPick(null);
                  setEditing({ kind: "add" });
                }}
              >
                Add worktree
              </Button>
            </div>
            {loading && (
              <p className={NOTE} role="status">
                Loading worktrees…
              </p>
            )}
            {page?.entries.length === 0 && (
              <p className={NOTE}>No worktrees found.</p>
            )}
            <ul className="git-worktree-list">
              {page?.entries.map((row) => {
                const mutable =
                  row.kind === "linked" &&
                  usable(row.name) &&
                  (row.state === "available" || row.state === "missing");
                return (
                  <li
                    key={row.gitDir.bytesB64}
                    className="border-b border-border px-0 py-[16px]"
                  >
                    <strong>
                      {row.name?.display ??
                        (row.kind === "bare"
                          ? "Bare repository"
                          : "Main worktree")}
                      {row.current ? " · Current" : ""}
                    </strong>
                    <p className={ROW_NOTE}>
                      {row.path?.display ?? "Path unavailable"}
                    </p>
                    <p className={ROW_NOTE}>
                      {row.head?.name?.display.replace(/^refs\/heads\//, "") ??
                        (row.head?.detached
                          ? "Detached HEAD"
                          : "Branch unavailable")}{" "}
                      · {row.state}
                      {row.locked === true
                        ? " · Locked"
                        : row.locked === null
                          ? " · Lock status unknown"
                          : ""}
                    </p>
                    {row.lockReason && (
                      <p className={ROW_NOTE}>
                        Lock reason: {row.lockReason.display}
                      </p>
                    )}
                    {row.lockReasonUnavailable && (
                      <p className={ROW_NOTE}>Lock reason unavailable.</p>
                    )}
                    {row.errorCode && (
                      <p className={ROW_NOTE}>
                        Details unavailable: {row.errorCode}
                      </p>
                    )}
                    {mutable && (
                      <div className="git-worktree-actions mx-0 my-[12px] flex flex-wrap items-center gap-[8px]">
                        {row.locked !== null && (
                          <Button
                            disabled={disabled}
                            onClick={() =>
                              edit(row.locked ? "unlock" : "lock", row)
                            }
                          >
                            {row.locked ? "Unlock…" : "Lock…"}
                          </Button>
                        )}
                        {row.state === "missing" && (
                          <Button
                            disabled={disabled}
                            onClick={() => edit("repair", row)}
                          >
                            Locate moved worktree…
                          </Button>
                        )}
                        {!row.current && row.locked === false && (
                          <Button
                            disabled={disabled}
                            onClick={() =>
                              edit(
                                row.state === "missing" ? "prune" : "remove",
                                row,
                              )
                            }
                          >
                            {row.state === "missing"
                              ? "Remove missing registration…"
                              : "Remove worktree…"}
                          </Button>
                        )}
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
            {page && (pageIndex > 0 || page.nextCursor) && (
              <Pagination
                aria-label="Worktree pages"
                className="git-worktree-pagination mx-0 my-[12px] flex flex-wrap items-center justify-between gap-[8px]"
              >
                <PaginationContent className="w-full flex-wrap justify-between gap-[8px]">
                  <PaginationItem>
                    <Button
                      disabled={busy || loading || pageIndex === 0}
                      onClick={() => setRequested(pageIndex - 1)}
                    >
                      Previous worktrees
                    </Button>
                  </PaginationItem>
                  <PaginationItem>
                    <span aria-current="page">Page {pageIndex + 1}</span>
                  </PaginationItem>
                  <PaginationItem>
                    <Button
                      disabled={busy || loading || !page.nextCursor}
                      onClick={() => next(false)}
                    >
                      Next worktrees
                    </Button>
                  </PaginationItem>
                </PaginationContent>
              </Pagination>
            )}
          </>
        )}
      </div>
    </Modal>
  );
}
