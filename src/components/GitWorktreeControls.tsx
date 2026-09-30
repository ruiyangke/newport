import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { gitPath, type GitPath, type GitWorktreeAction } from "../domain/git";
import { type GitWorktrees, type GitRepository } from "../domain/gitResponses";
import { gitQueries, invalidateRepository } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button, Input, Checkbox } from "./controls";
import { Modal } from "./Editors";
import { useGitPageLoader } from "../hooks/useGitPageLoader";
import { gitErrorMessage } from "../git/errors";
import { gitProjectsFor } from "../git/registry";
import { GitLoadMore } from "./GitLoadMore";
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
  const [editing, setEditing] = useState<Editing | null>(initial ?? null);
  const [formError, setFormError] = useState("");
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [reason, setReason] = useState("");
  const [locked, setLocked] = useState(false);
  const [branchSelectOpen, setBranchSelectOpen] = useState(false);
  const [branchPick, setBranchPick] = useState<{
    snapshot: string;
    name: string;
  } | null>(null);
  const worktreeQuery = useQuery({
    ...gitQueries.worktrees(scope, repoId),
    refetchOnMount: "always",
  });
  const page = worktreeQuery.data;
  const loading =
    worktreeQuery.isFetching || !worktreeQuery.isFetchedAfterMount;
  const listError = worktreeQuery.isError
    ? gitErrorMessage(worktreeQuery.error)
    : "";
  const worktreePages = useGitPageLoader({
    queryKey: gitQueries.worktrees(scope, repoId).queryKey,
    page: page ?? null,
    enabled: !loading && !worktreeQuery.isError && !editing,
    prefetch: true,
    entryKey: (row: Worktree) => row.gitDir.bytesB64,
    read: (cursor, signal) =>
      gitProjectsFor(scope)
        .repositories.withSignal(signal)
        .worktrees(repoId, cursor),
  });
  const adding = editing?.kind === "add";
  const branchQuery = useQuery({
    ...gitQueries.branches(scope, repoId),
    enabled: adding,
    staleTime: 0,
  });
  const branchPage = branchQuery.data;
  const branchLoading =
    branchQuery.isFetching || !branchQuery.isFetchedAfterMount;
  const branchError = branchQuery.isError
    ? gitErrorMessage(branchQuery.error)
    : "";
  const branchPages = useGitPageLoader({
    queryKey: gitQueries.branches(scope, repoId).queryKey,
    page: branchPage ?? null,
    enabled:
      adding && branchSelectOpen && !branchLoading && !branchQuery.isError,
    prefetch: true,
    entryKey: (row) => row.reference.bytesB64,
    read: (cursor, signal) =>
      gitProjectsFor(scope)
        .repositories.withSignal(signal)
        .branches(repoId, cursor),
  });
  const readError = formError || listError || (adding ? branchError : "");
  const branchName =
    branchPick?.snapshot === branchPage?.snapshot
      ? (branchPick?.name ?? "")
      : "";
  const setBranchName = (name: string) =>
    branchPage && setBranchPick({ snapshot: branchPage.snapshot, name });
  const branches =
    branchPage?.entries.filter(
      (branch) =>
        !branch.remote &&
        (!branch.current || repository.bare) &&
        branch.oid &&
        usable(branch.name),
    ) ?? [];
  const branch = branches.find((branch) => branch.name.bytesB64 === branchName);
  const disabled =
    busy || loading || worktreeQuery.isError || !!blockedReason || !page;
  const titles = {
    add: "Add worktree",
    lock: "Lock worktree",
    unlock: "Unlock worktree",
    remove: "Remove worktree",
    prune: "Remove missing registration",
    repair: "Locate moved worktree",
  };
  function restart() {
    setFormError("");
    setBranchPick(null);
  }
  async function submit(action: GitWorktreeAction) {
    if (disabled || !page) return;
    try {
      if (!(await onAction(action, page.snapshot))) return;
      setEditing(null);
      restart();
      void invalidateRepository(queryClient, scope, repoId);
    } catch (error) {
      setFormError(gitErrorMessage(error));
    }
  }
  function edit(kind: Exclude<Editing["kind"], "add">, row: Worktree) {
    setReason("");
    setPath("");
    setFormError("");
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
                void worktreeQuery.refetch();
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
                  if (
                    !branch?.oid ||
                    branchLoading ||
                    branchQuery.isError ||
                    !name.trim()
                  )
                    return;
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
                setFormError(gitErrorMessage(error));
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
                    open={branchSelectOpen}
                    onOpenChange={setBranchSelectOpen}
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
                      <div onKeyDown={(event) => event.stopPropagation()}>
                        {branchPage && (
                          <GitLoadMore
                            cursor={branchPage.nextCursor}
                            loading={branchPages.loading}
                            error={branchPages.error}
                            disabled={branchLoading || branchQuery.isError}
                            automatic={branchSelectOpen}
                            onLoad={branchPages.load}
                            label="Load more branches"
                            endLabel="All branches loaded"
                          />
                        )}
                      </div>
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
                    No available local branches in the loaded results.
                    {!repository.bare && " The current branch cannot be used."}
                  </p>
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
                    (branchLoading ||
                      branchQuery.isError ||
                      !branch ||
                      !name.trim() ||
                      !path)) ||
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
            {page && (
              <GitLoadMore
                cursor={page.nextCursor}
                loading={worktreePages.loading}
                error={worktreePages.error}
                disabled={loading || worktreeQuery.isError}
                onLoad={worktreePages.load}
                label="Load more worktrees"
                endLabel="All worktrees loaded"
              />
            )}
          </>
        )}
      </div>
    </Modal>
  );
}
