import { useEffect, useId, useMemo, useRef, useState } from "react";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { FolderTree, GitBranch, Lock, Plus, TriangleAlert } from "lucide-react";
import { toast } from "sonner";
import { cn } from "cn";
import type { GitProject } from "../domain/git";
import type { GitRepository } from "../domain/gitResponses";
import { gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import {
  openable,
  worktreeBranch,
  worktreeKey,
  worktreeLabel,
  type WorktreeRow,
} from "../git/worktrees";
import { Button } from "./controls";
import { GitPicker } from "./GitPicker";
import { Popover, PopoverContent, PopoverTrigger } from "./ui/popover";
import {
  Command,
  CommandEmpty,
  CommandInput,
  CommandItem,
  CommandList,
} from "./ui/command";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "./ui/context-menu";
import { asksForContextMenu, openHighlightedRowMenu } from "./commandRowMenu";

/** What the worktree's status read said, when one was asked for. */
type Summary = {
  branch?: string | null;
  changes?: number | null;
  outgoing?: number | null;
};

/** A worktree as a bookmark-shaped read target, for the summary query. */
function asProject(row: WorktreeRow, serverId: string): GitProject | null {
  if (!row.path) return null;
  return {
    id: `worktree:${worktreeKey(row)}`,
    serverId,
    name: worktreeLabel(row),
    path: row.path,
  };
}

function StatusFact({ summary }: { summary?: Summary }) {
  if (!summary) return null;
  const changes = typeof summary.changes === "number" ? summary.changes : null;
  const outgoing =
    typeof summary.outgoing === "number" && summary.outgoing > 0
      ? summary.outgoing
      : null;
  return (
    <span className="flex flex-none items-center gap-[6px] text-[11px] text-muted-foreground tabular-nums">
      {changes === null ? null : changes > 0 ? (
        <span className="flex items-center gap-[4px] text-foreground">
          <span
            aria-hidden="true"
            className="size-[6px] rounded-full bg-(--orange)"
          />
          {changes} {changes === 1 ? "change" : "changes"}
        </span>
      ) : (
        <span className="flex items-center gap-[4px]">
          <span
            aria-hidden="true"
            className="size-[6px] rounded-full bg-(--green)"
          />
          Clean
        </span>
      )}
      {outgoing && (
        <span title="Commits ahead of the upstream, from stored refs">
          ↑{outgoing}
        </span>
      )}
    </span>
  );
}

/**
 * The toolbar's worktree picker: every checkout of this repository, which one
 * the page is showing, and a way into any of them. Agents work in worktrees,
 * one branch each, so this is how a person follows them: pick one and the
 * whole page -- changes, history, commit, push -- is that checkout.
 *
 * Status is read only when asked ("Check status"), one worktree at a time on
 * the server's one connection, and never inferred: an unread worktree shows
 * nothing rather than "clean".
 */
export function GitWorktreePicker({
  project,
  repository,
  busy,
  onOpen,
  onNew,
  onManage,
  onFiles,
  currentSummary,
}: {
  project: GitProject;
  repository: GitRepository;
  busy: boolean;
  /** The page's own read of the checkout it shows, which needs no check. */
  currentSummary?: Summary;
  /** Makes this worktree the page's workspace. */
  onOpen: (row: WorktreeRow) => void;
  onNew: () => void;
  /** Opens the worktree manager, at a row's form when one is given. */
  onManage: (edit?: {
    kind: "lock" | "unlock" | "remove" | "prune" | "repair";
    row: WorktreeRow;
  }) => void;
  onFiles: (path: string) => void;
}) {
  const scope = useCurrentServerScope();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [checking, setChecking] = useState(false);
  const hintId = useId();
  const listing = useQuery(gitQueries.worktreeList(scope, repository.repoId));
  const rows = useMemo(() => listing.data?.entries ?? [], [listing.data]);
  const current = rows.find((row) => row.current) ?? null;
  const targets = useMemo(
    () =>
      rows.flatMap((row) => {
        const target = asProject(row, scope.id);
        return target && openable(row) ? [{ row, target }] : [];
      }),
    [rows, scope.id],
  );
  // Reads nothing on its own: each is enabled only by "Check status".
  const summaries = useQueries({
    queries: targets.map(({ target }) => ({
      ...gitQueries.checkout(scope, target),
      enabled: false,
    })),
    combine: (results) =>
      Object.fromEntries(
        results.flatMap((result, index) =>
          result.data && targets[index]
            ? [[worktreeKey(targets[index].row), result.data as Summary]]
            : [],
        ),
      ) as Record<string, Summary>,
  });
  // The current run; closing the picker ends it, since nothing it reads is
  // on screen any more and every read holds the connection for the page.
  const run = useRef(0);
  useEffect(() => () => void (run.current += 1), []);
  async function checkAll() {
    const mine = ++run.current;
    setChecking(true);
    try {
      for (const { target } of targets) {
        if (mine !== run.current) return;
        await queryClient
          .fetchQuery({ ...gitQueries.checkout(scope, target), staleTime: 0 })
          .catch(() => undefined);
      }
    } finally {
      if (mine === run.current) setChecking(false);
    }
  }
  async function copyPath(path: string) {
    try {
      await navigator.clipboard.writeText(path);
      toast.success("Copied worktree path");
    } catch (reason) {
      toast.error(`Could not copy the path: ${String(reason)}`);
    }
  }
  const linked = rows.filter((row) => row.kind === "linked").length;
  // An agent too old to list worktrees has nothing to pick from; the picker
  // steps aside rather than standing in the toolbar with an error in it.
  if (listing.isError && !listing.data) return null;
  const title = current
    ? worktreeLabel(current)
    : repository.root.display === project.path.display
      ? "Main worktree"
      : (repository.root.display.split("/").pop() ?? "Worktree");

  return (
    <>
      <Popover
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) {
            run.current += 1;
            setChecking(false);
          }
          if (next) {
            setSearch("");
            // Opening reads the list afresh: agents add and remove worktrees
            // behind the page's back.
            void listing.refetch();
          }
        }}
      >
        <PopoverTrigger asChild>
          <GitPicker
            data-slot="button"
            aria-label={`Worktrees: ${title}`}
            disabled={busy}
            icon={<FolderTree size={14} />}
            emphasis={false}
            label="Current worktree"
            value={
              <>
                {title}
                {linked > 0 && (
                  <span className="ml-[6px] rounded-full bg-[color-mix(in_srgb,var(--foreground)_9%,transparent)] px-[5px] text-[10px] leading-[16px] font-semibold text-muted-foreground tabular-nums">
                    {linked + 1}
                  </span>
                )}
              </>
            }
            valueTitle={current?.path?.display ?? repository.root.display}
          />
        </PopoverTrigger>
        <PopoverContent
          align="start"
          sideOffset={6}
          collisionPadding={12}
          aria-label="Worktrees"
          className="git-worktree-panel flex max-h-[min(70vh,560px,var(--radix-popover-content-available-height))] w-[440px] flex-col gap-0 overflow-hidden rounded-[8px] border border-border p-0 shadow-[0_12px_32px_rgb(23_34_56/14%)] ring-0"
        >
          <Command
            label="Filter worktrees"
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
            <span id={hintId} className="sr-only">
              Press Shift+F10 for worktree actions.
            </span>
            <div className="flex items-center gap-[8px] p-[12px] *:data-[slot=command-input-wrapper]:min-w-0 *:data-[slot=command-input-wrapper]:flex-1 *:data-[slot=command-input-wrapper]:p-0">
              <CommandInput
                autoFocus
                aria-label="Filter worktrees"
                aria-describedby={hintId}
                placeholder="Filter by name, branch or path"
                className="text-[12px]"
                value={search}
                onValueChange={setSearch}
                onKeyDown={(event) => {
                  if (
                    asksForContextMenu(event) &&
                    openHighlightedRowMenu(event.currentTarget)
                  )
                    event.preventDefault();
                }}
              />
              <Button
                disabled={busy}
                className="gap-[5px]"
                onClick={() => {
                  setOpen(false);
                  onNew();
                }}
              >
                <Plus size={13} aria-hidden="true" />
                New worktree
              </Button>
            </div>
            {listing.isError && (
              <p
                role="alert"
                className="mx-[12px] mb-[8px] flex flex-wrap items-center gap-[8px] rounded-[6px] border border-destructive px-[8px] py-[6px] text-[12px] text-destructive"
              >
                {String(
                  (listing.error as { message?: string })?.message ??
                    listing.error,
                )}
                <Button onClick={() => void listing.refetch()}>Retry</Button>
              </p>
            )}
            {listing.isPending && (
              <p
                role="status"
                className="px-[12px] pb-[8px] text-[12px] text-muted-foreground"
              >
                Loading worktrees…
              </p>
            )}
            <CommandList
              label="Worktrees"
              className="git-worktree-list max-h-none min-h-0 flex-1 overflow-y-auto px-[4px] pb-[4px]"
            >
              {listing.data && (
                <CommandEmpty className="py-[16px] text-center text-[12px] text-muted-foreground">
                  No worktree matches.
                </CommandEmpty>
              )}
              {rows.map((row) => {
                const label = worktreeLabel(row);
                const branch = worktreeBranch(row);
                const canOpen = openable(row) && !row.current;
                const managed =
                  row.kind === "linked" &&
                  (row.state === "available" || row.state === "missing");
                return (
                  <ContextMenu key={worktreeKey(row)}>
                    <ContextMenuTrigger asChild>
                      <CommandItem
                        data-slot="command-item"
                        value={worktreeKey(row)}
                        keywords={[
                          label,
                          branch ?? "",
                          row.path?.display ?? "",
                        ]}
                        onSelect={() => {
                          if (!canOpen || busy) return;
                          setOpen(false);
                          onOpen(row);
                        }}
                        data-checked={row.current}
                        aria-current={row.current || undefined}
                        aria-disabled={
                          !canOpen && !row.current ? true : undefined
                        }
                        className="min-h-[44px] items-start gap-[10px] rounded-[5px] px-[10px] py-[7px]"
                      >
                        <FolderTree
                          size={15}
                          aria-hidden="true"
                          className={cn(
                            "mt-[2px] flex-none",
                            row.current
                              ? "text-primary"
                              : "text-muted-foreground",
                          )}
                        />
                        <div className="min-w-0 flex-1">
                          <div className="flex min-w-0 items-center gap-[6px]">
                            <strong className="truncate text-[13px] font-semibold">
                              {label}
                            </strong>
                            {row.locked && (
                              <Lock
                                size={12}
                                aria-label={
                                  row.lockReason
                                    ? `Locked: ${row.lockReason.display}`
                                    : "Locked"
                                }
                                className="flex-none text-muted-foreground"
                              />
                            )}
                            {row.state !== "available" && (
                              <span className="flex flex-none items-center gap-[3px] rounded-[4px] bg-[color-mix(in_srgb,var(--orange)_14%,transparent)] px-[5px] text-[10px] leading-[16px] font-semibold text-(--orange)">
                                <TriangleAlert size={10} aria-hidden="true" />
                                {row.state}
                              </span>
                            )}
                          </div>
                          <div className="mt-[2px] flex min-w-0 items-center gap-[5px] text-[11px] text-muted-foreground">
                            {branch && (
                              <>
                                <GitBranch
                                  size={11}
                                  aria-hidden="true"
                                  className="flex-none"
                                />
                                <span className="max-w-[45%] flex-none truncate text-foreground/80">
                                  {branch}
                                </span>
                                <span aria-hidden="true">·</span>
                              </>
                            )}
                            <span
                              className="min-w-0 truncate [direction:rtl] text-left"
                              title={row.path?.display}
                            >
                              <bdi>
                                {row.path?.display ?? "Path unavailable"}
                              </bdi>
                            </span>
                          </div>
                        </div>
                        <div className="flex flex-none flex-col items-end gap-[4px] pt-[1px]">
                          <StatusFact
                            summary={
                              row.current
                                ? (currentSummary ??
                                  summaries[worktreeKey(row)])
                                : summaries[worktreeKey(row)]
                            }
                          />
                        </div>
                      </CommandItem>
                    </ContextMenuTrigger>
                    <ContextMenuContent
                      aria-label={`Actions for ${label}`}
                      onKeyDown={(event) => event.stopPropagation()}
                      onContextMenu={(event) => event.preventDefault()}
                    >
                      <ContextMenuItem
                        disabled={!canOpen || busy}
                        onSelect={() => {
                          setOpen(false);
                          onOpen(row);
                        }}
                      >
                        Open worktree
                      </ContextMenuItem>
                      <ContextMenuItem
                        disabled={!row.path}
                        onSelect={() => {
                          setOpen(false);
                          if (row.path) onFiles(row.path.display);
                        }}
                      >
                        Open in Files
                      </ContextMenuItem>
                      <ContextMenuItem
                        disabled={!row.path}
                        onSelect={() =>
                          row.path && void copyPath(row.path.display)
                        }
                      >
                        Copy path
                      </ContextMenuItem>
                      {managed && (
                        <>
                          <ContextMenuSeparator />
                          {row.locked !== null && (
                            <ContextMenuItem
                              disabled={busy}
                              onSelect={() => {
                                setOpen(false);
                                onManage({
                                  kind: row.locked ? "unlock" : "lock",
                                  row,
                                });
                              }}
                            >
                              {row.locked ? "Unlock…" : "Lock…"}
                            </ContextMenuItem>
                          )}
                          {row.state === "missing" && (
                            <ContextMenuItem
                              disabled={busy}
                              onSelect={() => {
                                setOpen(false);
                                onManage({ kind: "repair", row });
                              }}
                            >
                              Locate moved worktree…
                            </ContextMenuItem>
                          )}
                          {!row.current && row.locked === false && (
                            <ContextMenuItem
                              variant="destructive"
                              disabled={busy}
                              onSelect={() => {
                                setOpen(false);
                                onManage({
                                  kind:
                                    row.state === "missing"
                                      ? "prune"
                                      : "remove",
                                  row,
                                });
                              }}
                            >
                              {row.state === "missing"
                                ? "Remove missing registration…"
                                : "Remove worktree…"}
                            </ContextMenuItem>
                          )}
                        </>
                      )}
                    </ContextMenuContent>
                  </ContextMenu>
                );
              })}
            </CommandList>
            <footer className="flex flex-none items-center gap-[8px] border-t border-border px-[12px] py-[8px]">
              <p className="min-w-0 flex-1 text-[11px] text-muted-foreground">
                {rows.length === 1
                  ? "Worktrees are separate checkouts of this repository, one branch each — how agents work side by side."
                  : `${rows.length} checkouts of this repository. Right-click one for more.`}
              </p>
              {targets.length > 0 && (
                <Button
                  disabled={busy || checking}
                  onClick={() => void checkAll()}
                  className="flex-none"
                >
                  {checking ? "Checking…" : "Check status"}
                </Button>
              )}
              <Button
                variant="ghost"
                disabled={busy}
                className="flex-none"
                onClick={() => {
                  setOpen(false);
                  onManage();
                }}
              >
                Manage…
              </Button>
            </footer>
          </Command>
        </PopoverContent>
      </Popover>
      {/* The divider before the branch picker is the worktree picker's own,
        so it goes when the picker does. */}
      <span
        className="mx-[2px] h-[16px] w-px flex-none bg-border"
        aria-hidden="true"
      />
    </>
  );
}
