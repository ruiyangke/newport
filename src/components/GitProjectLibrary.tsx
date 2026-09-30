import { useQuery, useQueryClient } from "@tanstack/react-query";
import { gitQueries } from "../query/git";
import type { ServerScope } from "../query/keys";
import { gitProjectsFor } from "../git/registry";
import { readIPC, readSharedQuery } from "../query/client";
import { useGitPageLoader } from "../hooks/useGitPageLoader";
import { GitLoadMore } from "./GitLoadMore";
import { gitErrorMessage } from "../git/errors";
import {
  Fragment,
  useEffect,
  useId,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import {
  Activity,
  ArrowUp,
  ArrowUpDown,
  ChevronDown,
  ChevronRight,
  Download,
  FolderGit2,
  FolderTree,
  FolderPlus,
  GitBranch,
  MoreHorizontal,
  Plus,
  RefreshCw,
  Search,
  Star,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "./controls";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { GitProject } from "../domain/git";
import {
  openable,
  worktreeBranch,
  worktreeKey,
  worktreeLabel,
  type WorktreeRow,
} from "../git/worktrees";

export type GitProjectFilter = "all" | "favorites" | "archived";
export type GitProjectSort = "name" | "path";
/**
 * Per-project Git state the caller has actually observed. Every field is
 * optional because the library lists bookmarks, not open repositories: a row
 * renders a neutral placeholder for anything it was not told, and never a zero
 * or a "clean" it inferred.
 */
export interface GitProjectRowStatus {
  branch?: string | null;
  changes?: number | null;
  clean?: boolean;
  running?: number | null;
  outgoing?: number | null;
  /** The repository's other worktrees, when they were listed. */
  worktrees?: WorktreeRow[];
}

const FILTERS: { value: GitProjectFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "favorites", label: "Favorites" },
  { value: "archived", label: "Archived" },
];
const SORTS: { value: GitProjectSort; label: string }[] = [
  { value: "name", label: "Name" },
  { value: "path", label: "Path" },
];

function plural(count: number, noun: string) {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

/*
 * The library is a list like Services': a page heading at the app's scale, one
 * row of controls, and a bordered list whose columns line up. Row actions
 * (favourite, the menu) appear on hover or focus, and a favourite's star stays
 * lit, so ten rows are not ten pairs of idle icons.
 *
 * `!` only where src/styles.css has an UNLAYERED claim: `[data-slot="button"]`
 * (height, radius, padding-inline, font-size, font-weight), bare `h1`, `p`
 * and `small`. Unlayered CSS beats `@layer utilities` whatever the
 * specificity. The spacing scale is 3.5px, so sizes are arbitrary values.
 */
const BRANCH_COLUMN = "w-[170px] flex-none min-w-0 max-[860px]:hidden";
const TREE_COLUMN = "w-[170px] flex-none min-w-0 max-[640px]:hidden";
const ACTIONS_COLUMN =
  "flex w-[60px] flex-none items-center justify-end gap-[2px]";
const ROW_ICON_BUTTON =
  "h-[26px]! w-[26px] rounded-[5px]! px-0! text-muted-foreground hover:text-foreground";
const MENU_CONTENT = "git-library-menu-list min-w-[190px]";
/** More worktrees than this start folded under their project. */
const FOLD_ABOVE = 6;

/** A dot and a word: the state a column reports, or that nothing was read. */
function Fact({
  tone,
  children,
}: {
  tone: "changed" | "clean" | "none";
  children: ReactNode;
}) {
  return (
    <span className="flex min-w-0 items-center gap-[6px]">
      {tone !== "none" && (
        <span
          aria-hidden="true"
          className={cn(
            "size-[7px] flex-none rounded-full",
            tone === "changed" ? "bg-(--orange)" : "bg-(--green)",
          )}
        />
      )}
      <span className={cn("truncate", tone === "changed" && "text-foreground")}>
        {children}
      </span>
    </span>
  );
}

/**
 * The branch and working-tree cells. Absent facts stay absent: a row nobody
 * read says so, never "clean" by inference.
 */
function StateCells({
  state,
  onCheck,
  checking,
  name,
}: {
  state: GitProjectRowStatus | undefined;
  onCheck?: () => void;
  checking?: boolean;
  name: string;
}) {
  const unread = (
    <span className="git-library-unknown flex min-w-0 items-center gap-[6px] text-muted-foreground/80">
      {checking ? (
        // A read in flight says so, and cannot be asked for twice.
        <button type="button" className="git-library-check" disabled>
          Reading…
        </button>
      ) : onCheck ? (
        <>
          <span className="truncate">Not checked</span>
          {/* Reading every bookmark on sight would read a repository per
              row; the row offers the read instead of performing it. */}
          <button
            type="button"
            className="git-library-check flex-none text-primary opacity-0 group-hover/row:opacity-100 hover:underline focus-visible:opacity-100"
            aria-label={`Check status of ${name}`}
            onClick={onCheck}
          >
            Check
          </button>
        </>
      ) : (
        "Not checked"
      )}
    </span>
  );
  if (!state)
    return (
      <>
        <div className={cn(BRANCH_COLUMN, "text-[12px]")}>
          <span className="sr-only">Branch: </span>
          {unread}
        </div>
        <div className={cn(TREE_COLUMN, "text-[12px]")}>
          <span className="text-muted-foreground/80" aria-hidden="true">
            —
          </span>
        </div>
      </>
    );
  const changes = typeof state.changes === "number" ? state.changes : null;
  let worktree: ReactNode;
  if (changes !== null && changes > 0)
    worktree = (
      <span title={plural(changes, "uncommitted change")} className="min-w-0">
        <Fact tone="changed">{plural(changes, "change")}</Fact>
      </span>
    );
  else if (changes === 0 || state.clean === true)
    worktree = <Fact tone="clean">Clean</Fact>;
  else if (state.clean === false)
    worktree = <Fact tone="changed">Uncommitted changes</Fact>;
  else
    worktree = (
      <span className="git-library-unknown text-muted-foreground/80">
        Not read
      </span>
    );
  return (
    <>
      <div className={cn(BRANCH_COLUMN, "text-[12px]")}>
        <span className="sr-only">Branch: </span>
        {/* `null` means read and on no branch; only a missing field is unread. */}
        {state.branch ? (
          <span className="flex min-w-0 items-center gap-[5px]">
            <GitBranch
              size={13}
              strokeWidth={1.6}
              aria-hidden="true"
              className="flex-none text-muted-foreground"
            />
            <span className="truncate">{state.branch}</span>
          </span>
        ) : state.branch === null ? (
          <span className="text-muted-foreground">Detached HEAD</span>
        ) : (
          <span className="git-library-unknown text-muted-foreground/80">
            Not read
          </span>
        )}
      </div>
      <div
        className={cn(
          TREE_COLUMN,
          "flex items-center gap-[6px] text-[12px] text-muted-foreground",
        )}
      >
        <span className="sr-only">Working tree: </span>
        {worktree}
        {/* Ahead of the stored upstream: commits not yet pushed, as far as
            the last fetch knows. */}
        {typeof state.outgoing === "number" && state.outgoing > 0 && (
          <span
            className="flex flex-none items-center gap-[1px] tabular-nums"
            title={`${plural(state.outgoing, "commit")} ahead of the upstream, from stored refs`}
            aria-label={`${state.outgoing} ahead`}
          >
            · <ArrowUp size={11} aria-hidden="true" className="ml-[3px]" />
            {state.outgoing}
          </span>
        )}
      </div>
    </>
  );
}

/**
 * A worktree's branch and working-tree cells. The branch is known from the
 * listing; the working tree only once it has been read, as for a project.
 */
function WorktreeCells({
  row,
  state,
  checking,
  onCheck,
}: {
  row: WorktreeRow;
  state?: GitProjectRowStatus;
  checking?: boolean;
  onCheck?: () => void;
}) {
  const branch = worktreeBranch(row);
  const changes = typeof state?.changes === "number" ? state.changes : null;
  const label = worktreeLabel(row);
  return (
    <>
      <div className={cn(BRANCH_COLUMN, "text-[12px]")}>
        <span className="sr-only">Branch: </span>
        {branch ? (
          <span className="flex min-w-0 items-center gap-[5px]">
            <GitBranch
              size={13}
              strokeWidth={1.6}
              aria-hidden="true"
              className="flex-none text-muted-foreground"
            />
            <span className="truncate">{branch}</span>
          </span>
        ) : (
          <span className="text-muted-foreground">Not on a branch</span>
        )}
      </div>
      <div
        className={cn(
          TREE_COLUMN,
          "flex items-center gap-[6px] text-[12px] text-muted-foreground",
        )}
      >
        <span className="sr-only">Working tree: </span>
        {changes !== null ? (
          changes > 0 ? (
            <Fact tone="changed">{plural(changes, "change")}</Fact>
          ) : (
            <Fact tone="clean">Clean</Fact>
          )
        ) : checking ? (
          <button type="button" className="git-library-check" disabled>
            Reading…
          </button>
        ) : (
          <span className="git-library-unknown flex min-w-0 items-center gap-[6px] text-muted-foreground/80">
            <span className="truncate">Not checked</span>
            {onCheck && (
              <button
                type="button"
                className="git-library-check flex-none text-primary opacity-0 group-hover/row:opacity-100 hover:underline focus-visible:opacity-100"
                aria-label={`Check status of ${label}`}
                onClick={onCheck}
              >
                Check
              </button>
            )}
          </span>
        )}
        {typeof state?.outgoing === "number" && state.outgoing > 0 && (
          <span
            className="flex flex-none items-center gap-[1px] tabular-nums"
            title={`${plural(state.outgoing, "commit")} ahead of the upstream, from stored refs`}
            aria-label={`${state.outgoing} ahead`}
          >
            · <ArrowUp size={11} aria-hidden="true" className="ml-[3px]" />
            {state.outgoing}
          </span>
        )}
      </div>
    </>
  );
}

export function GitProjectLibrary({
  projects,
  busy = false,
  search,
  onSearch,
  onOpen,
  onEdit,
  onRemove,
  onAdd,
  onRefresh,
  onClone,
  onCreate,
  favourites,
  onToggleFavourite,
  archived,
  sort,
  onSort,
  filter,
  onFilter,
  status,
  onCheck,
  onCheckAll,
  checking,
  description,
  worktreeStatus,
  onOpenWorktree,
  onCheckWorktree,
  scope,
  onVisibleWorktrees,
}: {
  scope?: ServerScope;
  onVisibleWorktrees?: (projectId: string, rows: WorktreeRow[]) => void;
  projects: GitProject[];
  busy?: boolean;
  search: string;
  onSearch: (value: string) => void;
  onOpen: (project: GitProject) => void;
  onEdit: (project: GitProject) => void;
  onRemove: (project: GitProject) => void;
  onAdd: () => void;
  /** Optional; renders a refresh control in the heading when supplied. */
  onRefresh?: () => void;
  onClone: () => void;
  /** Initialising a new repository; the menu entry is hidden without it. */
  onCreate?: () => void;
  favourites: ReadonlySet<string>;
  onToggleFavourite: (id: string) => void;
  /**
   * Ids the caller has archived. Without it nothing can be archived, so the
   * Archived filter is not offered at all -- a tab that can only ever be
   * empty is a dead end, not a feature.
   */
  archived?: Set<string>;
  sort: GitProjectSort;
  onSort: (value: GitProjectSort) => void;
  filter: GitProjectFilter;
  onFilter: (value: GitProjectFilter) => void;
  status?: Record<string, GitProjectRowStatus>;
  /** Reads one bookmark's Git state on request. Rows stay passive without it. */
  onCheck?: (project: GitProject) => void;
  /** Reads every row being shown, on request. */
  onCheckAll?: (projects: GitProject[]) => void;
  /** Ids whose read is in flight. */
  checking?: Set<string>;
  /** Optional per-project blurb; rows omit the line when there is none. */
  description?: (project: GitProject) => string | undefined;
  /** What each worktree's read said, keyed `worktree:<git dir>`. */
  worktreeStatus?: Record<string, GitProjectRowStatus>;
  /** Opens a project at one of its worktrees. */
  onOpenWorktree?: (project: GitProject, row: WorktreeRow) => void;
  /** Reads one worktree's working tree on request. */
  onCheckWorktree?: (project: GitProject, row: WorktreeRow) => void;
}) {
  const searchId = useId();
  // Only the choices someone made; the rest follow FOLD_ABOVE.
  const [folding, setFolding] = useState<ReadonlyMap<string, boolean>>(
    () => new Map(),
  );
  const rows = useMemo(() => {
    const needle = search.trim().toLowerCase();
    const isArchived = (project: GitProject) =>
      archived?.has(project.id) ?? false;
    const visible = projects.filter((project) => {
      if (filter === "favorites" && !favourites.has(project.id)) return false;
      if (filter === "archived" ? !isArchived(project) : isArchived(project))
        return false;
      if (!needle) return true;
      return `${project.name} ${project.path.display}`
        .toLowerCase()
        .includes(needle);
    });
    return [...visible].sort((left, right) =>
      sort === "path"
        ? left.path.display.localeCompare(right.path.display)
        : left.name.localeCompare(right.name),
    );
  }, [projects, search, filter, favourites, archived, sort]);
  const filters = FILTERS.filter(
    (option) => option.value !== "archived" || archived,
  );
  const sortLabel =
    SORTS.find((option) => option.value === sort)?.label ?? "Name";
  const empty =
    filter === "archived"
      ? "No archived projects."
      : filter === "favorites"
        ? "No favorite projects yet. Star a project to pin it here."
        : search.trim()
          ? `No projects match “${search.trim()}”.`
          : null;
  const anyChecking = !!checking && checking.size > 0;

  return (
    <section
      className="git-library min-h-0 w-full max-w-[1240px] flex-1 overflow-y-auto px-[22px] pt-[18px] pb-[24px] text-[13px]"
      aria-label="Projects"
    >
      <div className="git-library-heading flex flex-wrap items-center gap-[8px]">
        <div className="min-w-0 flex-1">
          <h1 className="text-[18px]! leading-[24px]! font-semibold! tracking-[-0.01em]!">
            Projects
          </h1>
          <p className="git-library-subtitle mt-[1px]! text-[12px] text-muted-foreground">
            {projects.length}{" "}
            {projects.length === 1 ? "Git repository" : "Git repositories"}{" "}
            bookmarked on this server
          </p>
        </div>
        {onRefresh && (
          <Button
            size="icon"
            aria-label="Refresh"
            disabled={busy}
            onClick={onRefresh}
            className="h-[30px]! w-[30px]"
          >
            <RefreshCw size={14} aria-hidden="true" />
          </Button>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="default"
              aria-label="Add project"
              disabled={busy}
              className="h-[30px]! gap-[6px] px-[12px]! text-[12px]! font-medium!"
            >
              <Plus size={14} aria-hidden="true" />
              Add project
              <ChevronDown
                size={13}
                aria-hidden="true"
                className="-mr-[2px] opacity-80"
              />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            sideOffset={6}
            className={MENU_CONTENT}
          >
            <DropdownMenuItem onSelect={() => onAdd()}>
              <FolderPlus size={14} aria-hidden="true" />
              Add existing
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => onClone()}>
              <Download size={14} aria-hidden="true" />
              Clone repository
            </DropdownMenuItem>
            {onCreate && (
              <DropdownMenuItem onSelect={() => onCreate()}>
                <FolderGit2 size={14} aria-hidden="true" />
                New repository
              </DropdownMenuItem>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      <div className="mt-[16px] flex flex-wrap items-center gap-[8px]">
        {/* The wrapper stays a <label> so the field keeps its own text label
            rather than leaning on the placeholder. */}
        <label
          className="flex h-[30px] w-[260px] max-w-full items-center gap-[6px] rounded-[6px] border border-border bg-background px-[8px] text-muted-foreground focus-within:border-ring"
          htmlFor={searchId}
        >
          <Search size={14} strokeWidth={1.6} aria-hidden="true" />
          <span className="sr-only">Search projects</span>
          <Input
            id={searchId}
            value={search}
            placeholder="Find a project…"
            className="h-[22px]! w-full min-w-0 border-0 bg-transparent px-[2px] py-0 text-[12px] text-foreground shadow-none focus-visible:border-0 focus-visible:ring-0 dark:bg-transparent"
            onChange={(event) => onSearch(event.target.value)}
          />
        </label>
        {/* Buttons rather than a ToggleGroup: single-select ToggleGroup swaps
            the group/aria-pressed pair for radiogroup/aria-checked. */}
        <div
          role="group"
          aria-label="Filter projects"
          className="flex h-[30px] items-center gap-[2px] rounded-[7px] bg-(--native-toolbar) p-[2px]"
        >
          {filters.map((option) => {
            const active = filter === option.value;
            return (
              <Button
                key={option.value}
                variant="ghost"
                aria-pressed={active}
                onClick={() => onFilter(option.value)}
                className={cn(
                  "h-[26px]! rounded-[5px]! border-0 px-[10px]! text-[12px] font-medium",
                  active
                    ? "bg-background text-foreground shadow-[0_1px_2px_rgb(0_0_0/0.1)] hover:bg-background dark:hover:bg-background"
                    : "bg-transparent text-muted-foreground hover:bg-transparent dark:hover:bg-transparent",
                )}
              >
                {option.label}
              </Button>
            );
          })}
        </div>
        <span className="flex-1" />
        {onCheckAll && rows.length > 0 && (
          <Button
            disabled={anyChecking}
            onClick={() => onCheckAll(rows)}
            className="h-[30px]! gap-[6px]"
          >
            <Activity size={13} aria-hidden="true" />
            {anyChecking ? "Checking…" : "Check status"}
          </Button>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              aria-label={`Sort: ${sortLabel}`}
              className="h-[30px]! gap-[6px]"
            >
              <ArrowUpDown size={13} aria-hidden="true" />
              {sortLabel}
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="end"
            sideOffset={6}
            className={MENU_CONTENT}
          >
            {/* One of a set, so the items are radios and announce which. */}
            <DropdownMenuRadioGroup
              value={sort}
              onValueChange={(value) => onSort(value as GitProjectSort)}
            >
              {SORTS.map((option) => (
                <DropdownMenuRadioItem key={option.value} value={option.value}>
                  {option.label}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {projects.length === 0 ? (
        <div className="git-library-empty mt-[14px] flex flex-col items-center gap-[8px] rounded-[8px] border border-dashed border-border px-[24px] py-[40px] text-center">
          <FolderGit2
            size={28}
            strokeWidth={1.4}
            aria-hidden="true"
            className="text-muted-foreground"
          />
          <h2 className="text-[14px]! font-semibold!">No projects yet</h2>
          <p className="max-w-[360px] text-[12px] text-muted-foreground">
            A project is a bookmark to a Git repository on this server. Add one
            you already have, clone one, or start a new one.
          </p>
          <div className="mt-[6px] flex gap-[8px]">
            <Button onClick={onAdd}>Add existing</Button>
            <Button onClick={onClone}>Clone repository</Button>
          </div>
        </div>
      ) : (
        <div className="git-library-list mt-[12px] overflow-hidden rounded-[8px] border border-border">
          <div
            className="git-library-caption flex h-[30px] items-center gap-[12px] border-b border-border bg-(--native-toolbar) px-[12px] text-[11px] font-medium text-muted-foreground"
            aria-hidden="true"
          >
            <span className="w-[18px] flex-none" />
            <span className="min-w-0 flex-1">
              {plural(rows.length, "project")}
            </span>
            <span className={BRANCH_COLUMN}>Branch</span>
            <span className={TREE_COLUMN}>Working tree</span>
            <span className={ACTIONS_COLUMN} />
          </div>
          {rows.length === 0 ? (
            <p className="git-library-empty px-[16px] py-[28px] text-center text-[12px] text-muted-foreground">
              {empty}
            </p>
          ) : (
            <ul className="git-library-rows m-0 list-none p-0">
              {rows.map((project) => {
                const favourite = favourites.has(project.id);
                const blurb = description?.(project);
                const listed = status?.[project.id]?.worktrees ?? [];
                // Folded to start with when there are many: agents leave
                // dozens, and the next project should not be pages away.
                const folded =
                  folding.get(project.id) ??
                  (scope ? true : listed.length > FOLD_ABOVE);
                const worktrees = folded ? [] : listed;
                return (
                  <Fragment key={project.id}>
                    <li
                      className="git-library-row group/row flex w-full items-center gap-[12px] border-b border-border px-[12px] py-[9px] last:border-b-0 hover:bg-[color-mix(in_srgb,var(--accent)_55%,transparent)]"
                      key={project.id}
                    >
                      <span className="flex w-[18px] flex-none self-start pt-[2px] text-muted-foreground">
                        <FolderGit2
                          size={17}
                          strokeWidth={1.6}
                          aria-hidden="true"
                        />
                      </span>
                      <div className="git-library-identity min-w-0 flex-1">
                        {/* The name WRAPS and is never cut: it is how the row is
                          recognised, and a truncated one can differ from its
                          neighbour only in the part that was hidden. */}
                        <Button
                          variant="ghost"
                          className="git-library-open block h-auto! min-h-[18px] w-fit max-w-full rounded-[4px]! bg-transparent px-0! text-left text-[13px]! leading-[18px] font-semibold! whitespace-normal text-foreground hover:bg-transparent hover:underline dark:hover:bg-transparent [overflow-wrap:anywhere]"
                          disabled={busy}
                          onClick={() => onOpen(project)}
                        >
                          {project.name}
                        </Button>
                        {blurb && (
                          <p className="my-[2px]! truncate text-[12px] text-muted-foreground">
                            {blurb}
                          </p>
                        )}
                        <div
                          className="truncate text-[11px] leading-[16px] text-muted-foreground"
                          title={project.path.display}
                        >
                          {project.path.display}
                        </div>
                        {(scope || listed.length > 0) && (
                          // Agents leave many; the project's own row stays
                          // put when its worktrees are folded away.
                          <button
                            type="button"
                            aria-expanded={!folded}
                            onClick={() =>
                              setFolding((current) =>
                                new Map(current).set(project.id, !folded),
                              )
                            }
                            className="git-library-worktrees-toggle mt-[2px] flex items-center gap-[3px] rounded-[4px] text-[11px] font-medium text-muted-foreground hover:text-foreground"
                          >
                            <ChevronRight
                              size={12}
                              aria-hidden="true"
                              className={cn(
                                "transition-transform",
                                !folded && "rotate-90",
                              )}
                            />
                            {scope
                              ? "Worktrees"
                              : plural(listed.length, "worktree")}
                          </button>
                        )}
                      </div>
                      <StateCells
                        state={status?.[project.id]}
                        onCheck={onCheck ? () => onCheck(project) : undefined}
                        checking={checking?.has(project.id)}
                        name={project.name}
                      />
                      <div className={ACTIONS_COLUMN}>
                        <Button
                          variant="ghost"
                          className={cn(
                            "git-library-favourite",
                            ROW_ICON_BUTTON,
                            favourite
                              ? "text-amber-500 hover:text-amber-500"
                              : "opacity-0 group-hover/row:opacity-100 focus-visible:opacity-100",
                          )}
                          aria-pressed={favourite}
                          aria-label={
                            favourite
                              ? `Remove ${project.name} from favorites`
                              : `Add ${project.name} to favorites`
                          }
                          onClick={() => onToggleFavourite(project.id)}
                        >
                          <Star
                            size={14}
                            strokeWidth={1.7}
                            aria-hidden="true"
                            className="size-[14px]"
                            fill={favourite ? "currentColor" : "none"}
                          />
                        </Button>
                        <DropdownMenu>
                          <DropdownMenuTrigger asChild>
                            <Button
                              variant="ghost"
                              aria-label={`Actions for ${project.name}`}
                              disabled={busy}
                              className={cn(
                                ROW_ICON_BUTTON,
                                "opacity-0 group-hover/row:opacity-100 focus-visible:opacity-100 aria-expanded:opacity-100",
                              )}
                            >
                              <MoreHorizontal
                                size={15}
                                strokeWidth={1.6}
                                aria-hidden="true"
                                className="size-[15px]"
                              />
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent
                            align="end"
                            sideOffset={6}
                            className={MENU_CONTENT}
                          >
                            <DropdownMenuItem onSelect={() => onOpen(project)}>
                              Open project
                            </DropdownMenuItem>
                            {onCheck && (
                              <DropdownMenuItem
                                onSelect={() => onCheck(project)}
                              >
                                Check status
                              </DropdownMenuItem>
                            )}
                            <DropdownMenuItem onSelect={() => onEdit(project)}>
                              Rename…
                            </DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem
                              variant="destructive"
                              onSelect={() => onRemove(project)}
                            >
                              Remove from projects…
                            </DropdownMenuItem>
                          </DropdownMenuContent>
                        </DropdownMenu>
                      </div>
                    </li>
                    {/* The project's other checkouts, under it: where agents
                      work, each on its own branch. */}
                    {!folded && scope ? (
                      <PagedLibraryWorktrees
                        scope={scope}
                        project={project}
                        busy={busy}
                        worktreeStatus={worktreeStatus}
                        checking={checking}
                        onOpenWorktree={onOpenWorktree}
                        onCheckWorktree={onCheckWorktree}
                        onVisible={onVisibleWorktrees}
                      />
                    ) : (
                      <LibraryWorktreeRows
                        rows={worktrees}
                        project={project}
                        busy={busy}
                        worktreeStatus={worktreeStatus}
                        checking={checking}
                        onOpenWorktree={onOpenWorktree}
                        onCheckWorktree={onCheckWorktree}
                      />
                    )}
                  </Fragment>
                );
              })}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}

type ChildProps = {
  project: GitProject;
  busy?: boolean;
  worktreeStatus?: Record<string, GitProjectRowStatus>;
  checking?: Set<string>;
  onOpenWorktree?: (project: GitProject, row: WorktreeRow) => void;
  onCheckWorktree?: (project: GitProject, row: WorktreeRow) => void;
};
function LibraryWorktreeRows({
  rows,
  project,
  busy,
  worktreeStatus,
  checking,
  onOpenWorktree,
  onCheckWorktree,
}: ChildProps & { rows: WorktreeRow[] }) {
  return (
    <>
      {rows.map((row) => {
        const key = `worktree:${worktreeKey(row)}`;
        const label = worktreeLabel(row);
        const canOpen = openable(row) && !!onOpenWorktree;
        return (
          <li
            key={key}
            className="git-library-worktree group/row flex w-full items-center gap-[12px] border-b border-border py-[7px] pr-[12px] pl-[42px] last:border-b-0 hover:bg-[color-mix(in_srgb,var(--accent)_55%,transparent)]"
          >
            <span className="flex w-[16px] flex-none self-start pt-[1px] text-muted-foreground">
              <FolderTree size={14} strokeWidth={1.6} aria-hidden="true" />
            </span>
            <div className="min-w-0 flex-1">
              <div className="flex min-w-0 items-center gap-[6px]">
                <Button
                  variant="ghost"
                  className="git-library-open-worktree block h-auto! min-h-[18px] max-w-full truncate rounded-[4px]! bg-transparent px-0! text-left text-[12px]! leading-[18px] font-medium! text-foreground hover:bg-transparent hover:underline dark:hover:bg-transparent"
                  disabled={busy || !canOpen}
                  aria-label={`Open worktree ${label} of ${project.name}`}
                  onClick={() => onOpenWorktree?.(project, row)}
                >
                  {label}
                </Button>
                {row.state !== "available" && (
                  <span className="flex-none rounded-[4px] bg-[color-mix(in_srgb,var(--orange)_14%,transparent)] px-[5px] text-[10px] leading-[16px] font-semibold text-(--orange)">
                    {row.state}
                  </span>
                )}
              </div>
              <div
                className="truncate text-[11px] leading-[15px] text-muted-foreground"
                title={row.path?.display}
              >
                {row.path?.display ?? "Path unavailable"}
              </div>
            </div>
            <WorktreeCells
              row={row}
              state={worktreeStatus?.[key]}
              checking={checking?.has(key)}
              onCheck={
                onCheckWorktree && openable(row)
                  ? () => onCheckWorktree(project, row)
                  : undefined
              }
            />
            <div className={ACTIONS_COLUMN} />
          </li>
        );
      })}
    </>
  );
}

function PagedLibraryWorktrees({
  scope,
  onVisible,
  ...props
}: ChildProps & {
  scope: ServerScope;
  onVisible?: (projectId: string, rows: WorktreeRow[]) => void;
}) {
  const client = useQueryClient();
  const opened = useQuery({
    queryKey: [
      ...gitQueries.projects(scope).queryKey,
      "worktree-repository",
      props.project.id,
      props.project.path.bytesB64,
    ],
    queryFn: ({ signal }) =>
      readIPC(signal, () => gitProjectsFor(scope).open(props.project)),
  });
  const repoId = opened.data?.repoId ?? "";
  const query = gitQueries.worktrees(scope, repoId);
  const listing = useQuery({ ...query, enabled: !!repoId });
  const pages = useGitPageLoader({
    queryKey: query.queryKey,
    page: listing.data ?? null,
    enabled: !!repoId && !listing.isFetching && !listing.isError,
    entryKey: worktreeKey,
    read: (cursor, signal) =>
      readSharedQuery(
        client,
        {
          ...gitQueries.worktrees(scope, repoId, cursor),
          staleTime: Infinity,
        },
        signal,
      ),
  });
  const rows = useMemo(
    () =>
      listing.data?.entries.filter(
        (row) => row.kind === "linked" && !row.current,
      ) ?? [],
    [listing.data],
  );
  useEffect(() => {
    onVisible?.(props.project.id, rows);
  }, [onVisible, props.project.id, rows]);
  useEffect(
    () => () => {
      onVisible?.(props.project.id, []);
    },
    [onVisible, props.project.id],
  );
  const error = opened.error ?? listing.error;
  return (
    <>
      <LibraryWorktreeRows {...props} rows={rows} />
      <li className="px-10 py-2 text-xs text-muted-foreground">
        {error ? (
          <span role="alert">
            {gitErrorMessage(error)}{" "}
            <Button
              onClick={() =>
                void (opened.error ? opened.refetch() : listing.refetch())
              }
            >
              Retry
            </Button>
          </span>
        ) : opened.isPending || listing.isPending ? (
          "Loading worktrees…"
        ) : rows.length === 0 && !listing.data?.nextCursor ? (
          "No other worktrees"
        ) : null}
        {listing.data && (
          <GitLoadMore
            cursor={listing.data.nextCursor}
            disabled={
              listing.isFetching ||
              listing.isError ||
              opened.isFetching ||
              opened.isError
            }
            loading={pages.loading}
            error={pages.error}
            onLoad={pages.load}
            label="Load more worktrees"
            endLabel="All worktrees loaded"
          />
        )}
      </li>
    </>
  );
}
