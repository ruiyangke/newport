import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import {
  useQueries,
  useQuery,
  useQueryClient,
  type UseQueryResult,
} from "@tanstack/react-query";
import { GitBranch } from "lucide-react";
import { toast } from "sonner";
import { gitPath, type GitWriteAction } from "../domain/git";
import {
  appendGitPage,
  type GitRepository,
  type decodeGitBranches,
} from "../domain/gitResponses";
import { refreshQuery } from "../query/client";
import { gitKeys, gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button, Input } from "./controls";
import { GitPicker } from "./GitPicker";
import { asksForContextMenu, openHighlightedRowMenu } from "./commandRowMenu";
import { Modal } from "./Editors";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "./ui/context-menu";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";
import { Popover, PopoverContent, PopoverTrigger } from "./ui/popover";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "./ui/command";

/** `refs/remotes/origin/main` → `origin/main`, `refs/heads/main` → `main`. */
function shortRef(reference: string) {
  return reference.replace(/^refs\/(remotes|heads)\//, "");
}

type Branches = ReturnType<typeof decodeGitBranches>;
type Branch = Branches["entries"][number];
type Editing =
  | { kind: "create" }
  | { kind: "rename" | "delete" | "upstream"; branch: Branch };
/**
 * The pages loaded after the first. Each page is its own query, keyed by its
 * cursor; this only remembers which cursors were loaded. It belongs to one
 * reading of the first page (`basis`, that read's time), so a fresh first page
 * -- the panel reopening, the repository changing, a retry -- starts the list
 * over, as it always has.
 */
type Trail = {
  basis: number;
  cursors: string[];
  pending: boolean;
  error: string;
};
const startTrail = (basis: number): Trail => ({
  basis,
  cursors: [],
  pending: false,
  error: "",
});
const pageData = (results: UseQueryResult<Branches>[]) =>
  results.map((result) => result.data);
function usable(branch: Branch) {
  try {
    return (
      !branch.remote &&
      !!branch.oid &&
      gitPath(branch.name.display).bytesB64 === branch.name.bytesB64
    );
  } catch {
    return false;
  }
}
/** Helper copy in the branch surfaces: small and muted, as the design has it. */
const hint = "text-[12px] text-muted-foreground";
/**
 * A failure in the popover. The dialogs get this from the shared Git alert rule
 * in projects.css; the popover is portalled out from under every surface that
 * rule names, so it states the same treatment itself.
 */
const failureTone =
  "flex flex-wrap items-center gap-[8px] rounded-[6px] border border-destructive px-[8px] py-[6px] text-[12px] text-destructive [overflow-wrap:anywhere]";
/**
 * Why a repository cannot be written right now, and the way to find out more.
 * Set apart from the ordinary helper copy around it, which is the same size.
 */
function BlockedNotice({
  reason,
  recoveryAvailable,
  onRecover,
}: {
  reason: string;
  recoveryAvailable?: boolean;
  onRecover: () => void;
}) {
  return (
    <div className="git-branch-notice flex flex-wrap items-center gap-[8px] rounded-[8px] border border-border bg-muted px-[10px] py-[8px]">
      <p className="flex-[1_1_200px] text-[12px] text-foreground">{reason}</p>
      {recoveryAvailable && (
        <Button onClick={onRecover}>View saved outcomes</Button>
      )}
    </div>
  );
}
async function copyBranchName(name: string) {
  try {
    await navigator.clipboard.writeText(name);
    toast.success(`Copied branch name ${name}`);
  } catch (reason) {
    toast.error(`Could not copy branch name: ${String(reason)}`);
  }
}

export function GitBranchControls({
  repository,
  snapshot,
  busy,
  writable,
  blockedReason,
  recoveryAvailable,
  error,
  onAction,
  worktrees,
}: {
  /** Still passed by the page; reads go through the session's registry. */
  repository: GitRepository;
  snapshot?: string;
  busy: boolean;
  writable: boolean;
  blockedReason?: string;
  recoveryAvailable?: boolean;
  error: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
  worktrees?: WorktreeHolders;
}) {
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<Editing | null>(null);
  const title =
    repository.head.name?.display.replace(/^refs\/heads\//, "") ??
    "Detached HEAD";
  return (
    // The host stands in for the trigger as the toolbar's flex item, so it takes
    // the picker's own sizing and the button fills it. Without this the button
    // stopped being a direct flex child of the toolbar and collapsed to its
    // minimum -- measured at 152x26 against the repository picker's 213x40.
    <div className="git-branch-picker flex max-w-[280px] min-w-0 flex-[0_1_auto]">
      {/* The list is a popover hanging off the picker; the forms it opens are
          dialogs. While a form is up the popover is closed, and closing the
          form returns to the list, as it always has. */}
      <Popover
        open={open && !editing}
        onOpenChange={(next) => {
          setOpen(next);
          if (next) setEditing(null);
        }}
      >
        <PopoverTrigger asChild>
          <GitPicker
            // PopoverTrigger would otherwise stamp its own `data-slot` over the
            // button's, and the app-wide `[data-slot="button"]` rule the picker
            // is styled against would stop matching it.
            data-slot="button"
            className="max-w-none flex-1"
            aria-label={`Branches: ${title}`}
            disabled={busy}
            icon={<GitBranch size={14} />}
            emphasis={false}
            label="Current branch"
            value={title}
            valueTitle={title}
          />
        </PopoverTrigger>
        {/* Mounted per opening, so every opening reads the branches afresh. */}
        {open && (
          <BranchPanel
            repository={repository}
            snapshot={snapshot}
            busy={busy}
            writable={writable}
            blockedReason={blockedReason}
            recoveryAvailable={recoveryAvailable}
            error={error}
            onAction={onAction}
            worktrees={worktrees}
            title={title}
            editing={editing}
            setEditing={setEditing}
            onClose={() => {
              setEditing(null);
              setOpen(false);
            }}
          />
        )}
      </Popover>
    </div>
  );
}

/**
 * Which other worktree has a branch checked out, and a way into it. A branch
 * can be checked out in one worktree at a time, so choosing one that another
 * worktree holds goes there instead of failing to switch.
 */
export type WorktreeHolders = {
  holder: (branch: string) => string | null;
  open: (branch: string) => void;
};

function BranchPanel({
  repository,
  snapshot,
  busy,
  writable,
  blockedReason,
  recoveryAvailable,
  error,
  onAction,
  worktrees,
  title,
  editing,
  setEditing,
  onClose,
}: {
  repository: GitRepository;
  snapshot?: string;
  busy: boolean;
  writable: boolean;
  blockedReason?: string;
  recoveryAvailable?: boolean;
  error: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
  worktrees?: WorktreeHolders;
  title: string;
  editing: Editing | null;
  setEditing: Dispatch<SetStateAction<Editing | null>>;
  onClose: () => void;
}) {
  const scope = useCurrentServerScope();
  const queryClient = useQueryClient();
  const repoId = repository.repoId;
  const [search, setSearch] = useState("");
  const actionsHint = useId();
  const [name, setName] = useState("");
  const [upstream, setUpstream] = useState("none");
  // Forcing is an explicit, per-confirmation escalation; never carry it over.
  const [force, setForce] = useState(false);
  const [editingSnapshot, setEditingSnapshot] = useState<string>();
  const first = useQuery({
    ...gitQueries.branches(scope, repoId),
    refetchOnMount: "always",
  });
  // A new status snapshot re-reads the branches, starting the list over.
  const seenSnapshot = useRef(snapshot);
  useEffect(() => {
    if (seenSnapshot.current === snapshot) return;
    seenSnapshot.current = snapshot;
    void queryClient.invalidateQueries({
      queryKey: gitKeys.branches(scope, repoId),
    });
  }, [queryClient, scope, repoId, snapshot]);
  const [storedTrail, setTrail] = useState(() => startTrail(0));
  const trail =
    storedTrail.basis === first.dataUpdatedAt
      ? storedTrail
      : startTrail(first.dataUpdatedAt);
  // Later pages are only ever read by "Load more", which checks that each one
  // continues the list; showing them must not re-read them on their own.
  const laterData = useQueries({
    queries: trail.cursors.map((cursor) => ({
      ...gitQueries.branches(scope, repoId, cursor),
      enabled: false,
    })),
    combine: pageData,
  });
  const loaded = useMemo(() => {
    if (!first.data) return null;
    let merged = first.data;
    for (const [index, cursor] of trail.cursors.entries()) {
      const page = laterData[index];
      if (!page) break;
      try {
        merged = appendGitPage(merged, page, cursor);
      } catch {
        break;
      }
    }
    return merged;
  }, [first.data, laterData, trail.cursors]);
  // The list read last time is shown at once while the picker re-reads it --
  // opening on "Loading branches…" every time made the picker feel slow on a
  // repository with thousands of branches. Nothing that changes a branch is
  // offered from it until the fresh read lands (`loading` disables writes);
  // browsing, filtering and going to a worktree need no fresh list.
  const branches = first.isSuccess ? loaded : null;
  const refreshing = first.isFetching && !!branches;
  const loading = first.isFetching || trail.pending;
  const readError =
    trail.error ||
    (!first.isFetching && first.error ? String(first.error) : "");
  async function submit(action: GitWriteAction) {
    if (!writable || busy || loading || staleUpstream) return;
    if (await onAction(action)) onClose();
  }
  const disabled = busy || loading || !writable;
  const staleUpstream =
    editing?.kind === "upstream" && (!snapshot || editingSnapshot !== snapshot);
  const upstreamChoices =
    branches?.entries.filter((branch) => {
      try {
        return (
          branch.oid &&
          gitPath(branch.reference.display).bytesB64 ===
            branch.reference.bytesB64 &&
          (editing?.kind !== "upstream" ||
            branch.reference.bytesB64 !== editing.branch.reference.bytesB64)
        );
      } catch {
        return false;
      }
    }) ?? [];
  const upstreamUnavailable =
    upstream !== "none" &&
    !upstreamChoices.some((branch) => branch.reference.display === upstream);
  async function loadMore() {
    if (!branches?.nextCursor || loading || busy) return;
    const cursor = branches.nextCursor;
    const requested: Trail = { ...trail, pending: true, error: "" };
    setTrail(requested);
    let outcome: Partial<Trail>;
    try {
      const next = await refreshQuery(
        queryClient,
        gitQueries.branches(scope, repoId, cursor),
      );
      appendGitPage(branches, next, cursor);
      outcome = { cursors: [...trail.cursors, cursor] };
    } catch (reason) {
      outcome = { error: String(reason) };
    }
    // A page that lands after the list started over belongs to a list that is
    // no longer shown. It stays cached under its own cursor, unused.
    setTrail((current) =>
      current === requested
        ? { ...current, ...outcome, pending: false }
        : current,
    );
  }
  function retry() {
    setTrail(startTrail(-1));
    void first.refetch();
  }
  function switchTo(branch: Branch) {
    if (disabled || branch.current || !usable(branch) || !branch.oid) return;
    void submit({
      kind: "checkout",
      target: {
        kind: "branch",
        name: branch.name.display,
        expectedOid: branch.oid.hex,
      },
    });
  }
  const failure = error && (
    <p role="alert" className={failureTone}>
      {error}
    </p>
  );
  const notice = !writable && blockedReason && (
    <BlockedNotice
      reason={blockedReason}
      recoveryAvailable={recoveryAvailable}
      onRecover={onClose}
    />
  );
  return (
    <>
      <PopoverContent
        align="start"
        sideOffset={6}
        collisionPadding={12}
        aria-label="Branches"
        // A form opening from the list closes the popover under it; focus goes
        // to the form, not back to the picker behind the dialog.
        onCloseAutoFocus={(event) => {
          if (editing) event.preventDefault();
        }}
        // Measured on the prototype: a 365px panel six pixels below the
        // trigger, aligned with its left edge, radius 8 and a 0 12px 32px
        // shadow at 14%. Arbitrary values because the spacing scale is 3.5px.
        className="git-branch-panel max-h-[min(70vh,520px,var(--radix-popover-content-available-height))] w-[365px] gap-0 overflow-hidden rounded-[8px] border border-border p-0 shadow-[0_12px_32px_rgb(23_34_56/14%)] ring-0"
      >
        {(notice || failure) && (
          <div className="flex flex-col gap-[8px] px-[12px] pt-[12px]">
            {notice}
            {failure}
          </div>
        )}
        <Command
          label="Filter branches"
          // Substring, as the filter has always been, over the name alone; the
          // row's value is its reference so two rows never share one.
          filter={(_value, query, keywords) =>
            (keywords ?? [])
              .join(" ")
              .toLocaleLowerCase()
              .includes(query.toLocaleLowerCase())
              ? 1
              : 0
          }
          className="size-auto min-h-0 flex-1 rounded-none! bg-transparent p-0"
        >
          <span id={actionsHint} className="sr-only">
            Press Shift+F10 for branch actions.
          </span>
          {/* The prototype's filter row: 12px of padding round the field. */}
          <div className="git-branch-tools flex items-center gap-[8px] p-[12px] *:data-[slot=command-input-wrapper]:min-w-0 *:data-[slot=command-input-wrapper]:flex-1 *:data-[slot=command-input-wrapper]:p-0">
            <CommandInput
              autoFocus
              aria-label="Filter branches"
              aria-describedby={actionsHint}
              placeholder="Filter branches"
              className="text-[12px]"
              value={search}
              onValueChange={setSearch}
              onKeyDown={(event) => {
                // Handled here, the keystroke raises no native menu on the
                // filter as well.
                if (
                  asksForContextMenu(event) &&
                  openHighlightedRowMenu(event.currentTarget)
                )
                  event.preventDefault();
              }}
            />
            <Button
              disabled={disabled || !repository.head.oid}
              onClick={() => {
                setName("");
                setEditing({ kind: "create" });
              }}
            >
              New branch
            </Button>
          </div>
          {repository.head.unborn && (
            <p className={`px-[12px] pb-[8px] ${hint}`}>
              Create the first commit before creating another branch.
            </p>
          )}
          {readError && (
            <p role="alert" className={`mx-[12px] mb-[8px] ${failureTone}`}>
              {readError}{" "}
              <Button disabled={busy || loading} onClick={retry}>
                Retry branches
              </Button>
            </p>
          )}
          {loading && (
            <p role="status" className={`px-[12px] pb-[8px] ${hint}`}>
              {refreshing ? "Refreshing branches…" : "Loading branches…"}
            </p>
          )}
          <CommandList
            label="Branches"
            className="git-branch-list px-[4px] pb-[4px]"
          >
            {/* A list being (re)read offers nothing, so it has no empty state. */}
            {branches && (
              <CommandEmpty className={`py-[16px] ${hint}`}>
                {search
                  ? "No matching branches in the loaded results."
                  : "No branches yet."}
              </CommandEmpty>
            )}
            {(
              [
                ["Local", branches?.entries.filter((b) => !b.remote) ?? []],
                ["Remote", branches?.entries.filter((b) => b.remote) ?? []],
              ] as const
            ).map(([heading, rows]) =>
              rows.length === 0 ? null : (
                <CommandGroup
                  key={heading}
                  heading={heading}
                  className="p-0 **:[[cmdk-group-heading]]:px-[10px] **:[[cmdk-group-heading]]:pt-[8px] **:[[cmdk-group-heading]]:pb-[4px] **:[[cmdk-group-heading]]:text-[11px] **:[[cmdk-group-heading]]:font-semibold"
                >
                  {rows.map((branch) => {
                    // The actions menu as a whole was unavailable on these terms;
                    // each action keeps its own condition on top.
                    const writes = disabled || !usable(branch);
                    const holder =
                      !branch.remote && !branch.current
                        ? (worktrees?.holder(branch.name.display) ?? null)
                        : null;
                    return (
                      <ContextMenu key={branch.reference.bytesB64}>
                        <ContextMenuTrigger asChild>
                          <CommandItem
                            // The menu trigger would otherwise stamp its own
                            // `data-slot` over the row's.
                            data-slot="command-item"
                            value={branch.reference.bytesB64}
                            keywords={[branch.name.display]}
                            // Choosing a row switches to it, under the conditions
                            // `switchTo` keeps.
                            onSelect={() => {
                              if (holder && worktrees) {
                                onClose();
                                worktrees.open(branch.name.display);
                              } else switchTo(branch);
                            }}
                            data-checked={branch.current}
                            aria-current={branch.current || undefined}
                            className="min-h-[30px] gap-[8px] rounded-[4px] px-[10px] py-[4px]"
                          >
                            <div className="min-w-0 flex-1">
                              <strong>{branch.name.display}</strong>
                              <small>
                                {branch.current
                                  ? "Current branch"
                                  : holder
                                    ? `In worktree ${holder}`
                                    : branch.remote
                                      ? "Remote-tracking branch"
                                      : (branch.oid?.hex.slice(0, 8) ??
                                        "Unavailable")}
                              </small>
                              {branch.upstream && (
                                <small
                                  className="ml-[6px]"
                                  title={branch.upstream.display}
                                >
                                  Tracks {shortRef(branch.upstream.display)}
                                </small>
                              )}
                            </div>
                          </CommandItem>
                        </ContextMenuTrigger>
                        <ContextMenuContent
                          aria-label={`Actions for ${branch.name.display}`}
                          // The menu is a React child of the list: keep its keys from
                          // reaching cmdk, which reads them as moving or choosing in
                          // the list underneath.
                          onKeyDown={(event) => event.stopPropagation()}
                          // The ContextMenu key raises its native menu on release, by
                          // when focus is in this one; it has no other to offer.
                          onContextMenu={(event) => event.preventDefault()}
                        >
                          <ContextMenuItem
                            disabled={writes}
                            onSelect={() => {
                              setName(branch.name.display);
                              setEditing({ kind: "rename", branch });
                            }}
                          >
                            Rename…
                          </ContextMenuItem>
                          <ContextMenuItem
                            onSelect={() =>
                              void copyBranchName(branch.name.display)
                            }
                          >
                            Copy Branch Name
                          </ContextMenuItem>
                          <ContextMenuItem
                            disabled={
                              writes || !snapshot || !branch.tracking?.editable
                            }
                            onSelect={() => {
                              setUpstream(branch.upstream?.display ?? "none");
                              setEditingSnapshot(snapshot);
                              setEditing({ kind: "upstream", branch });
                            }}
                          >
                            {branch.tracking?.editable
                              ? "Set upstream…"
                              : "Upstream configuration unavailable or inherited"}
                          </ContextMenuItem>
                          <ContextMenuItem
                            disabled={writes || branch.current}
                            onSelect={() => (
                              setForce(false),
                              setEditing({ kind: "delete", branch })
                            )}
                          >
                            Delete…
                          </ContextMenuItem>
                        </ContextMenuContent>
                      </ContextMenu>
                    );
                  })}
                </CommandGroup>
              ),
            )}
          </CommandList>
          {branches?.nextCursor && (
            <footer className="flex flex-col items-start gap-[8px] border-t border-border p-[12px]">
              <p className={hint}>
                Showing {branches.entries.length} loaded branches. Load more to
                search the remaining branches.
              </p>
              <Button
                disabled={busy || loading}
                onClick={() => void loadMore()}
              >
                Load more branches
              </Button>
            </footer>
          )}
        </Command>
      </PopoverContent>
      {editing && (
        <Modal
          title={
            editing.kind === "create"
              ? "Create branch"
              : editing.kind === "rename"
                ? "Rename branch"
                : editing.kind === "delete"
                  ? "Delete branch"
                  : "Set upstream"
          }
          onClose={() => setEditing(null)}
          busy={busy}
          // `!` because `.editor-dialog` sets its width in an unlayered rule.
          className="git-branch-dialog w-[min(560px,calc(100vw-32px))]!"
        >
          <div className="git-branch-body px-[24px] pb-[24px]">
            {notice}
            {error && (
              <p role="alert" className={hint}>
                {error}
              </p>
            )}
            <form
              className="git-project-form"
              onSubmit={(event) => {
                event.preventDefault();
                if (disabled || staleUpstream) return;
                if (
                  editing.kind === "create" &&
                  repository.head.oid &&
                  name.trim()
                )
                  void submit({
                    kind: "branch.create",
                    name: name.trim(),
                    startOid: repository.head.oid.hex,
                  });
                else if (
                  editing.kind === "rename" &&
                  editing.branch.oid &&
                  name.trim()
                )
                  void submit({
                    kind: "branch.rename",
                    name: editing.branch.name.display,
                    newName: name.trim(),
                    expectedOid: editing.branch.oid.hex,
                  });
                else if (editing.kind === "delete" && editing.branch.oid)
                  void submit({
                    kind: "branch.delete",
                    name: editing.branch.name.display,
                    expectedOid: editing.branch.oid.hex,
                    ...(force ? { force: true } : {}),
                  });
                else if (
                  editing.kind === "upstream" &&
                  editing.branch.oid &&
                  editing.branch.tracking?.editable &&
                  !upstreamUnavailable
                )
                  void submit({
                    kind: "branch.set_upstream",
                    name: editing.branch.name.display,
                    expectedOid: editing.branch.oid.hex,
                    expectedToken: editing.branch.tracking.token,
                    upstream: upstream === "none" ? null : upstream,
                  });
              }}
            >
              {editing.kind === "upstream" ? (
                <>
                  <p className={hint}>
                    Choose the tracking branch for{" "}
                    <strong>{editing.branch.name.display}</strong>. This changes
                    local Git configuration; it does not fetch, push, or move
                    commits.
                  </p>
                  <p className={hint}>
                    Current upstream:{" "}
                    {editing.branch.upstream?.display ?? "None"}
                  </p>
                  <label>
                    Upstream branch
                    <Select
                      value={upstream}
                      onValueChange={setUpstream}
                      disabled={disabled || staleUpstream}
                    >
                      <SelectTrigger aria-label="Upstream branch">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="none">
                          None — remove tracking
                        </SelectItem>
                        {upstreamUnavailable && (
                          <SelectItem value={upstream} disabled>
                            {upstream} (not loaded or unavailable)
                          </SelectItem>
                        )}
                        {upstreamChoices.map((branch) => (
                          <SelectItem
                            key={branch.reference.bytesB64}
                            value={branch.reference.display}
                          >
                            {branch.name.display} ·{" "}
                            {branch.remote ? "remote" : "local"}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </label>
                  <p className={hint}>
                    Only stored branches are listed. Fetch from a remote to
                    discover its latest branches.
                  </p>
                  {branches?.nextCursor && (
                    <Button
                      disabled={busy || loading || staleUpstream}
                      onClick={() => void loadMore()}
                    >
                      Load more upstream branches
                    </Button>
                  )}
                  {loading && (
                    <p role="status" className={hint}>
                      Loading branches…
                    </p>
                  )}
                  {readError && (
                    <p role="alert" className={hint}>
                      {readError}
                    </p>
                  )}
                  {staleUpstream && (
                    <p role="alert" className={hint}>
                      The repository changed. Go back and select the branch
                      again.
                    </p>
                  )}
                </>
              ) : editing.kind === "delete" ? (
                <>
                  <p className={hint}>
                    Delete <strong>{editing.branch.name.display}</strong>? Git
                    will refuse if the branch contains unmerged commits or is in
                    use by a worktree.
                  </p>
                  <label className="git-branch-force">
                    <input
                      type="checkbox"
                      checked={force}
                      disabled={busy}
                      onChange={(event) => setForce(event.target.checked)}
                    />
                    Delete even if it contains unmerged commits
                  </label>
                  {force && (
                    <p role="alert" className={hint}>
                      Commits reachable only from this branch become
                      unreferenced. A worktree using the branch still blocks
                      deletion.
                    </p>
                  )}
                </>
              ) : (
                <label>
                  Branch name
                  <Input
                    autoFocus
                    value={name}
                    onChange={(event) => setName(event.target.value)}
                    required
                    disabled={busy}
                  />
                </label>
              )}
              {editing.kind === "create" && (
                <p className={hint}>
                  Starting from {title} at{" "}
                  {repository.head.oid?.hex.slice(0, 8)}.
                </p>
              )}
              <footer>
                <Button disabled={busy} onClick={() => setEditing(null)}>
                  Back
                </Button>
                <Button
                  type="submit"
                  variant={
                    editing.kind === "delete" ? "destructive" : undefined
                  }
                  disabled={
                    disabled ||
                    staleUpstream ||
                    (editing.kind === "upstream"
                      ? upstreamUnavailable ||
                        !editing.branch.tracking?.editable
                      : editing.kind !== "delete" && !name.trim())
                  }
                >
                  {editing.kind === "create"
                    ? "Create branch"
                    : editing.kind === "rename"
                      ? "Rename branch"
                      : editing.kind === "upstream"
                        ? "Save upstream"
                        : "Delete branch"}
                </Button>
              </footer>
            </form>
          </div>
        </Modal>
      )}
    </>
  );
}
