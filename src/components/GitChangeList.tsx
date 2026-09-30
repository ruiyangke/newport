import { useId } from "react";
import { ListFilter, Minus, Plus, Search } from "lucide-react";
import { cn } from "cn";
import type { GitStatus } from "../domain/gitResponses";
import type { GitChangeGroup } from "../state/git";
import { Button } from "./controls";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "./ui/input-group";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import { GitNotice } from "./GitNotice";
import {
  GitStatusBadge,
  gitStatusName,
  type GitStatusMark,
} from "./GitStatusBadge";

type StatusEntry = GitStatus["entries"][number];

/** git2 Status bits, as emitted by the agent. */
const INDEX_NEW = 1 << 0;
const INDEX_DELETED = 1 << 2;
const INDEX_RENAMED = 1 << 3;
const WT_DELETED = 1 << 9;
const WT_RENAMED = 1 << 11;

/**
 * One letter describing what this entry does in this group. A file can appear
 * in both Staged and Unstaged because they are different comparisons, so the
 * letter depends on the group rather than the entry alone.
 */
export function changeMark(
  entry: StatusEntry,
  group: GitChangeGroup,
): GitStatusMark {
  if (group === "conflicted") return "!";
  if (group === "untracked") return "?";
  if (group === "staged") {
    if (entry.flags & INDEX_NEW) return "A";
    if (entry.flags & INDEX_DELETED) return "D";
    if (entry.flags & INDEX_RENAMED) return "R";
    return "M";
  }
  if (entry.flags & WT_DELETED) return "D";
  if (entry.flags & WT_RENAMED) return "R";
  return "M";
}

/*
 * Conflicts first, then the comparisons. One table, so the filter-options menu
 * and the list cannot come to disagree about what a group is or what it is
 * called.
 */
export const CHANGE_GROUPS: {
  key: GitChangeGroup;
  label: string;
  holds: (entry: StatusEntry) => boolean;
}[] = [
  {
    key: "conflicted",
    label: "Conflicts",
    holds: (entry) => entry.conflicted,
  },
  {
    key: "staged",
    label: "Staged",
    holds: (entry) => entry.staged && !entry.conflicted,
  },
  {
    key: "unstaged",
    label: "Unstaged",
    holds: (entry) => entry.unstaged && !entry.conflicted && !entry.untracked,
  },
  {
    key: "untracked",
    label: "Untracked",
    holds: (entry) => entry.untracked && !entry.conflicted,
  },
];

/** Empty groups are omitted, and a chosen group narrows to just that one. */
export function changeGroups(
  entries: StatusEntry[],
  filter: string,
  only: GitChangeGroup | "all" = "all",
) {
  const needle = filter.trim().toLowerCase();
  const visible = needle
    ? entries.filter((entry) =>
        `${entry.path?.display ?? ""} ${entry.oldPath?.display ?? ""}`
          .toLowerCase()
          .includes(needle),
      )
    : entries;
  return CHANGE_GROUPS.map((group) => ({
    key: group.key,
    label: group.label,
    entries: visible.filter(group.holds),
  })).filter(
    (group) =>
      group.entries.length > 0 && (only === "all" || group.key === only),
  );
}

function countLabel(count: number, noun: string) {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

/**
 * Splits a path into its directory and the file's own name, which are set in
 * different inks. The trailing slash stays with the directory so the halves
 * rejoin exactly.
 */
function splitChangePath(path: string): ["dir" | "base", string][] {
  const cut = path.lastIndexOf("/");
  if (cut < 0) return [["base", path]];
  return [
    ["dir", path.slice(0, cut + 1)],
    ["base", path.slice(cut + 1)],
  ];
}

/**
 * A changed file's path: the directory in muted ink and the name in body ink,
 * so the name reads first in a column of similar paths. The directory is the
 * half that shrinks and ellipses; the name always keeps its full width.
 */
export function ChangePath({
  path,
  className,
}: {
  path: string;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "git-change-path flex min-w-0 overflow-hidden whitespace-nowrap",
        className,
      )}
      /* The halves are flex items, which the name computation separates with a
         space ("src/ app.ts"); naming the span keeps the path as written. */
      aria-label={path}
      title={path}
    >
      {splitChangePath(path).map(([kind, text]) =>
        kind === "dir" ? (
          <span
            key={kind}
            className="git-change-dir min-w-0 truncate text-muted-foreground"
          >
            {text}
          </span>
        ) : (
          /* The name gives way only once the directory has nothing left:
             its shrink weight is a ten-thousandth of the directory's. */
          <span
            key={kind}
            className="git-change-base min-w-0 flex-[0_0.0001_auto] truncate"
          >
            {text}
          </span>
        ),
      )}
    </span>
  );
}

/**
 * The Changes list: a filter, then the working tree grouped by comparison.
 *
 * Staging stays explicit and per comparison: a row's quick action stages or
 * unstages that file in the index, and a group's action does the same for the
 * whole group. Nothing is committed from a selection.
 */
export function GitChangeList({
  status,
  filter,
  onFilter,
  group,
  onGroup,
  selectedEntry,
  selectedSide,
  onSelect,
  writable,
  busy,
  onStage,
  onUnstage,
  onLoadMore,
}: {
  status: GitStatus;
  filter: string;
  onFilter: (value: string) => void;
  group: GitChangeGroup | "all";
  onGroup: (value: GitChangeGroup | "all") => void;
  selectedEntry: string | null;
  selectedSide: GitChangeGroup;
  onSelect: (entryId: string, group: GitChangeGroup) => void;
  /** Whether index writes are allowed right now. */
  writable: boolean;
  busy: boolean;
  onStage: (entryIds: string[]) => void;
  onUnstage: (entryIds: string[]) => void;
  onLoadMore?: () => void;
}) {
  const statusId = useId();
  const groups = changeGroups(status.entries, filter, group);
  const filtering = !!filter.trim() || group !== "all";
  const shown = groups.reduce((total, each) => total + each.entries.length, 0);
  const integrating = !!status.metadata.integration;
  // Unstaging during a merge or rebase would lose the resolution bookkeeping,
  // so the file controls never offered it; the quick actions follow suit.
  const canUnstage = writable && !integrating;
  return (
    <>
      <div className="git-changes-toolbar flex flex-none items-center px-[10px] pt-[8px] pb-[6px]">
        <InputGroup className="h-[28px] rounded-[6px] border-border bg-background">
          <InputGroupAddon className="pl-[8px] text-muted-foreground">
            <Search size={13} aria-hidden="true" />
          </InputGroupAddon>
          <InputGroupInput
            aria-label="Filter changed files"
            placeholder="Filter files"
            value={filter}
            onChange={(event) => onFilter(event.target.value)}
            className="h-[26px] min-w-0 px-[4px] text-[12px]"
          />
          <InputGroupAddon
            align="inline-end"
            className="pr-[3px] has-[>button]:mr-0"
          >
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <InputGroupButton
                  size="icon-xs"
                  aria-label="Filter options"
                  disabled={busy}
                  data-slot="button"
                  className={cn(
                    "h-[22px]! w-[22px] rounded-[4px]! px-0!",
                    group !== "all" ? "text-primary" : "text-muted-foreground",
                  )}
                >
                  <ListFilter size={13} aria-hidden="true" />
                </InputGroupButton>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuLabel>Show</DropdownMenuLabel>
                <DropdownMenuRadioGroup
                  value={group}
                  onValueChange={(next) =>
                    onGroup(next as GitChangeGroup | "all")
                  }
                >
                  <DropdownMenuRadioItem value="all">
                    All changes
                  </DropdownMenuRadioItem>
                  {CHANGE_GROUPS.map((each) => (
                    <DropdownMenuRadioItem key={each.key} value={each.key}>
                      {each.label} only
                    </DropdownMenuRadioItem>
                  ))}
                </DropdownMenuRadioGroup>
              </DropdownMenuContent>
            </DropdownMenu>
          </InputGroupAddon>
        </InputGroup>
      </div>
      {status.metadata.truncated && (
        <GitNotice tone="status">
          This working tree has{" "}
          {status.metadata.totalEntries?.toLocaleString() ?? "more"} changed
          files; the first{" "}
          {status.metadata.entryLimit?.toLocaleString() ?? "several thousand"}{" "}
          are shown. Untracked directories are already collapsed to one row, so
          a count this large usually means a build or dependency directory is
          not ignored. Add it to .gitignore, then refresh.
        </GitNotice>
      )}
      {/* Only while filtering: the total stays in view, so a filter cannot
          quietly look like an empty working tree. Unfiltered, the count is on
          the Changes tab. */}
      {filtering && (
        <p className="git-projects-summary flex-none px-[16px] pb-[4px] text-[11px] text-muted-foreground">
          {shown} of {countLabel(status.entries.length, "changed file")}
          {status.nextCursor ? " loaded" : ""}
        </p>
      )}
      {/* The scrolling region. The filter above and the composer below stay
          put, so the commit button is never eighty files away. */}
      <div className="git-changes-scroll min-h-0 flex-1 overflow-y-auto pb-[6px]">
        {groups.map((each) => {
          const ids = each.entries.map((entry) => entry.entryId);
          const bulk =
            each.key === "staged"
              ? canUnstage && {
                  label: "Unstage all",
                  name: `Unstage all ${each.entries.length} staged`,
                  run: () => onUnstage(ids),
                }
              : each.key === "unstaged" || each.key === "untracked"
                ? writable && {
                    label: "Stage all",
                    name: `Stage all ${each.entries.length} ${each.key}`,
                    run: () => onStage(ids),
                  }
                : false;
          return (
            <section
              className="git-changes-group"
              key={each.key}
              aria-label={each.label}
            >
              <h4 className="sticky top-0 z-[1] flex h-[28px] items-center gap-[6px] bg-background pr-[10px] pl-[16px] text-[11px]! font-semibold! text-muted-foreground">
                <span>{each.label}</span>
                <span className="font-normal tabular-nums">
                  {each.entries.length}
                </span>
                {bulk && (
                  <Button
                    variant="ghost"
                    aria-label={bulk.name}
                    disabled={busy}
                    onClick={bulk.run}
                    className="ml-auto h-[22px]! rounded-[4px]! px-[6px]! text-[11px]! font-medium! text-muted-foreground hover:text-foreground"
                  >
                    {bulk.label}
                  </Button>
                )}
              </h4>
              <ul className="m-0 list-none px-[6px] py-0">
                {each.entries.map((entry) => {
                  const path =
                    entry.path?.display ??
                    entry.oldPath?.display ??
                    "Unknown path";
                  const selected =
                    selectedEntry === entry.entryId &&
                    selectedSide === each.key;
                  const quick =
                    each.key === "staged"
                      ? canUnstage && {
                          label: `Unstage ${path}`,
                          icon: <Minus size={13} aria-hidden="true" />,
                          run: () => onUnstage([entry.entryId]),
                        }
                      : each.key === "unstaged" || each.key === "untracked"
                        ? writable && {
                            label: `Stage ${path}`,
                            icon: <Plus size={13} aria-hidden="true" />,
                            run: () => onStage([entry.entryId]),
                          }
                        : false;
                  return (
                    <li
                      key={entry.entryId}
                      /* The row carries the hover and the selection, so the
                         file button and its quick action sit side by side
                         inside one highlight instead of one over the other. */
                      className={cn(
                        "group/row flex items-center rounded-[5px] pr-[3px]",
                        selected ? "bg-(--git-tint)" : "hover:bg-accent",
                      )}
                      data-selected={selected || undefined}
                    >
                      <Button
                        variant="ghost"
                        aria-pressed={selected}
                        aria-describedby={`${statusId}-${entry.entryId}-${each.key}`}
                        onClick={() => onSelect(entry.entryId, each.key)}
                        className="h-[28px]! min-w-0 flex-1 justify-start gap-[8px] rounded-[5px]! border-0 bg-transparent pr-[6px]! pl-[10px]! text-left text-[12px] font-normal text-foreground hover:bg-transparent dark:hover:bg-transparent"
                      >
                        <GitStatusBadge
                          mark={changeMark(entry, each.key)}
                          decorative
                          className="git-change-mark"
                        />
                        <ChangePath path={path} />
                        {/* Named by its path; what happened is its description. */}
                        <span
                          id={`${statusId}-${entry.entryId}-${each.key}`}
                          hidden
                        >
                          {gitStatusName(changeMark(entry, each.key))}
                        </span>
                        {/* A rename keeps its origin visible. */}
                        {entry.oldPath &&
                          entry.path &&
                          entry.oldPath.bytesB64 !== entry.path.bytesB64 && (
                            <span className="git-change-origin min-w-0 flex-[0_1000_auto] truncate text-[11px] text-muted-foreground">
                              from {entry.oldPath.display}
                            </span>
                          )}
                      </Button>
                      {quick && (
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          aria-label={quick.label}
                          disabled={busy}
                          onClick={quick.run}
                          className={cn(
                            "h-[22px]! w-[22px] flex-none rounded-[4px]! px-0! text-muted-foreground opacity-0 group-hover/row:opacity-100 hover:bg-background hover:text-foreground focus-visible:opacity-100",
                            selected && "opacity-100",
                          )}
                        >
                          {quick.icon}
                        </Button>
                      )}
                    </li>
                  );
                })}
              </ul>
            </section>
          );
        })}
        {filtering && groups.length === 0 && (
          <div className="git-projects-empty git-filter-empty px-[16px] py-[28px] text-center">
            <h3 className="text-[12px]! font-semibold!">
              No files match your current filters
            </h3>
            <p className="mt-[4px]! text-[11px] text-muted-foreground">
              {filter.trim()
                ? `No changed file matches “${filter.trim()}”.`
                : "This group has no files."}
              {status.nextCursor ? " Only loaded files are filtered." : ""}
            </p>
            <Button
              className="mt-[12px]"
              onClick={() => {
                onFilter("");
                onGroup("all");
              }}
            >
              Clear filters
            </Button>
          </div>
        )}
        {status.nextCursor && onLoadMore && (
          <div className="px-[16px] pt-[6px]">
            <Button disabled={busy} onClick={onLoadMore}>
              Load more files
            </Button>
          </div>
        )}
      </div>
    </>
  );
}
