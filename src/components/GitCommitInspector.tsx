import {
  useId,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { Check, Copy } from "lucide-react";
import { cn } from "cn";
import { useQueries, useQuery } from "@tanstack/react-query";
import type { GitPath } from "../domain/git";
import {
  appendGitPage,
  type GitCommitFiles,
  type GitHistory,
} from "../domain/gitResponses";
import { gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button, Select, SelectItem } from "./controls";
import { GitDiffView } from "./GitChangesPreview";
import { ListDiffPanels } from "./diff/GitChangesSplit";
import { GitStatusBadge, gitStatusName, markForStatus } from "./GitStatusBadge";
import { ChangePath } from "./GitChangeList";
import { DiffStat, GitDiffSettings } from "./diff/GitDiffSettings";
import { GitNotice } from "./GitNotice";

/**
 * 1100, measured: the prototype's history view keeps its files and diff side
 * by side at 1150 and stacks them at 1100. Stacked, a vertical handle has
 * nothing to divide, so there is none.
 */
const STACKED = "(max-width: 1100px)";
function subscribeStacked(listener: () => void) {
  if (typeof window.matchMedia !== "function") return () => {};
  const query = window.matchMedia(STACKED);
  query.addEventListener("change", listener);
  return () => query.removeEventListener("change", listener);
}
function useStacked() {
  return useSyncExternalStore(
    subscribeStacked,
    () =>
      typeof window.matchMedia === "function" &&
      window.matchMedia(STACKED).matches,
  );
}

function errorMessage(error: unknown) {
  return error && typeof error === "object" && "message" in error
    ? String(error.message)
    : String(error);
}
type Commit = GitHistory["entries"][number];

/** How long ago, in the words a history list uses; the exact time is the title. */
export function relativeTime(seconds: number, now = Date.now()) {
  const then = seconds * 1000;
  if (!Number.isFinite(then)) return "";
  const minutes = Math.round((now - then) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  if (days < 7) return days === 1 ? "yesterday" : `${days} days ago`;
  const date = new Date(then);
  return date.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    ...(date.getFullYear() === new Date(now).getFullYear()
      ? {}
      : { year: "numeric" }),
  });
}

/** The subject line and the body after it, as Git separates them. */
export function splitMessage(message: string) {
  const [subject = "", ...rest] = message.split("\n");
  return {
    subject,
    body: rest
      .join("\n")
      .replace(/^\s*\n/, "")
      .trimEnd(),
  };
}

/**
 * A commit body, shown as its first few lines. A long message -- a squash of
 * forty commits, a pasted log -- would otherwise push the files and the diff
 * off the screen; the rest is one click away, and the clamp is measured, so a
 * short body never offers to expand.
 */
function CommitBody({ body }: { body: string }) {
  const [expanded, setExpanded] = useState(false);
  const [overflowing, setOverflowing] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () =>
      setOverflowing(element.scrollHeight > element.clientHeight + 1);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [body]);
  return (
    <div className="mt-[6px]">
      {/* Collapsed, the body is cut to a few lines and fades out rather than
          ending on an ellipsis -- a clamp's "…" lands on whatever line is
          last, blank lines included. */}
      <div
        ref={ref}
        className={cn(
          "git-commit-message overflow-hidden text-[12px] leading-[18px] whitespace-pre-wrap text-muted-foreground [overflow-wrap:anywhere]",
          !expanded && "max-h-[72px]",
          !expanded &&
            overflowing &&
            "[mask-image:linear-gradient(to_bottom,black_55%,transparent)]",
        )}
      >
        {body}
      </div>
      {(overflowing || expanded) && (
        <Button
          variant="link"
          aria-expanded={expanded}
          onClick={() => setExpanded((value) => !value)}
          className="h-auto! px-0! py-[2px] text-[11px]! font-medium! text-primary"
        >
          {expanded ? "Show less" : "Show more"}
        </Button>
      )}
    </div>
  );
}

export function GitCommitInspector({
  repoId,
  commit,
  actions,
}: {
  repoId: string;
  commit: Commit;
  /** The commit's own actions, drawn in its header. */
  actions?: ReactNode;
}) {
  const [parentIndex, setParentIndex] = useState(0);
  const [copied, setCopied] = useState(false);
  const date = new Date(commit.time * 1000);
  const { subject, body } = splitMessage(commit.message.display);
  const exact = Number.isNaN(date.getTime())
    ? "Date unavailable"
    : date.toLocaleString();
  return (
    <section
      className="git-commit-inspector flex h-full min-h-0 flex-col"
      aria-label="Commit details"
    >
      <header className="git-commit-header flex-none border-b border-border px-[16px] pt-[12px] pb-[10px]">
        <div className="flex items-start gap-[12px]">
          <div className="min-w-0 flex-1">
            {/* The unlayered bare `h3` rule claims size, weight and margin. */}
            <h3 className="text-[14px]! leading-[20px] font-semibold! [overflow-wrap:anywhere]">
              {subject || "Empty commit message"}
            </h3>
            {body && <CommitBody key={commit.oid.hex} body={body} />}
            {commit.messageTruncated && (
              <p
                role="status"
                className="mt-[4px] text-[11px] text-muted-foreground"
              >
                The commit message is truncated.
              </p>
            )}
          </div>
          {actions}
        </div>
        <div className="git-commit-identity mt-[8px] flex flex-wrap items-center gap-x-[10px] gap-y-[4px] text-[11px] leading-[16px] text-muted-foreground">
          <span className="text-foreground" title={commit.author.email}>
            {commit.author.name}
          </span>
          <time dateTime={date.toISOString?.()} title={exact}>
            {Number.isNaN(date.getTime()) ? exact : relativeTime(commit.time)}
          </time>
          <span className="flex items-center gap-[2px]">
            <code title={commit.oid.hex}>{commit.oid.hex.slice(0, 7)}</code>
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label={copied ? "Commit ID copied" : "Copy commit ID"}
              className="h-[20px]! w-[20px] px-0! text-muted-foreground"
              onClick={() => {
                void navigator.clipboard
                  ?.writeText(commit.oid.hex)
                  .then(() => {
                    setCopied(true);
                    setTimeout(() => setCopied(false), 1500);
                  })
                  .catch(() => undefined);
              }}
            >
              {copied ? (
                <Check size={12} aria-hidden="true" />
              ) : (
                <Copy size={12} aria-hidden="true" />
              )}
            </Button>
          </span>
          {commit.parents.length > 1 ? (
            <Select
              aria-label="Compare with parent"
              className="h-[24px]! w-auto text-[11px]!"
              value={String(parentIndex)}
              onValueChange={(value) => setParentIndex(Number(value))}
            >
              {commit.parents.map((parent, index) => (
                <SelectItem
                  key={`${index}:${parent.hex}`}
                  value={String(index)}
                >
                  Parent {index + 1} · {parent.hex.slice(0, 7)}
                </SelectItem>
              ))}
            </Select>
          ) : (
            <span className="git-commit-base">
              {commit.parents.length
                ? `Compared with parent ${commit.parents[0].hex.slice(0, 7)}`
                : "Initial commit · compared with an empty tree"}
            </span>
          )}
        </div>
      </header>
      <CommitFiles
        key={`${commit.oid.hex}:${parentIndex}`}
        repoId={repoId}
        commitOid={commit.oid.hex}
        parentIndex={parentIndex}
      />
    </section>
  );
}

function CommitFiles({
  repoId,
  commitOid,
  parentIndex,
}: {
  repoId: string;
  commitOid: string;
  parentIndex: number;
}) {
  const scope = useCurrentServerScope();
  // Each page is its own read, keyed by the cursor that asked for it; these
  // are the cursors of the pages after the first, in the order visited.
  const [cursors, setCursors] = useState<string[]>([]);
  // The page asked for. It is on screen once it, and each page before it,
  // has arrived and continues the one before.
  const [requested, setRequested] = useState(0);
  const stacked = useStacked();
  const statusId = useId();
  // A selection belongs to the page it was made on, so it lapses when another
  // page arrives.
  const [picked, setPicked] = useState<{
    page: number;
    path: GitPath;
  } | null>(null);
  const reads = useQueries({
    queries: [undefined, ...cursors].map((cursor) =>
      gitQueries.commitFiles(
        scope,
        cursor === undefined
          ? { repoId, commitOid, parentIndex }
          : { repoId, commitOid, parentIndex, cursor },
      ),
    ),
  });
  const pages: GitCommitFiles[] = [];
  let failure: unknown = null;
  for (const [index, read] of reads.slice(0, requested + 1).entries()) {
    if (read.isError && !read.isFetching) failure = read.error;
    if (!read.data || failure) break;
    if (index > 0)
      try {
        appendGitPage(pages[index - 1], read.data, cursors[index - 1]);
      } catch (error) {
        failure = error;
        break;
      }
    pages.push(read.data);
  }
  const busy = reads.slice(0, requested + 1).some((read) => read.isFetching);
  const error = failure === null ? "" : errorMessage(failure);
  const pageIndex = Math.max(0, pages.length - 1);
  const page = pages[pageIndex];
  // The first file is shown until another is chosen, so the pane opens on a
  // diff rather than on an instruction to pick one.
  const first = page?.entries.find((file) => file.newPath ?? file.oldPath);
  const selected =
    picked?.page === pageIndex
      ? picked.path
      : (first?.newPath ?? first?.oldPath ?? null);
  const setSelected = (path: GitPath | null) =>
    setPicked(path && { page: pageIndex, path });
  function next() {
    if (!page?.nextCursor || busy) return;
    const target = pageIndex + 1;
    if (cursors[pageIndex] !== page.nextCursor)
      setCursors([...cursors.slice(0, pageIndex), page.nextCursor]);
    else if (reads[target]?.isError) void reads[target].refetch();
    setRequested(target);
  }
  const offset = pages
    .slice(0, pageIndex)
    .reduce((count, page) => count + page.entries.length, 0);
  const fileList = page && (
    <section
      className="flex h-full min-h-0 flex-1 flex-col overflow-hidden"
      aria-label="Commit files"
      aria-busy={busy}
    >
      <p className="git-projects-summary flex-none px-[16px] pt-[8px] pb-[4px] text-[11px] font-semibold text-muted-foreground">
        {page.metadata.totalFiles}{" "}
        {page.metadata.totalFiles === 1 ? "changed file" : "changed files"}
      </p>
      {!page.entries.length ? (
        <p className="git-projects-empty">
          No changed files against this parent.
        </p>
      ) : (
        <ul className="git-commit-files m-0 min-h-0 flex-1 list-none overflow-y-auto px-[6px] pb-[6px]">
          {page.entries.map((file, index) => {
            const path = file.newPath ?? file.oldPath;
            const chosen = !!path && selected?.bytesB64 === path.bytesB64;
            return (
              <li
                data-selected={chosen}
                className={cn(
                  "flex items-center rounded-[5px]",
                  chosen ? "bg-(--git-tint)" : "hover:bg-accent",
                )}
                key={`${file.oldPath?.bytesB64}:${file.newPath?.bytesB64}:${index}`}
              >
                <Button
                  variant="ghost"
                  disabled={!path}
                  aria-pressed={chosen}
                  aria-describedby={`${statusId}-${index}`}
                  onClick={() => setSelected(path)}
                  className="h-[28px]! min-w-0 flex-1 justify-start gap-[8px] rounded-[5px]! border-0 bg-transparent px-[10px]! text-left text-[12px] font-normal text-foreground hover:bg-transparent dark:hover:bg-transparent"
                >
                  <GitStatusBadge
                    mark={markForStatus(file.status)}
                    decorative
                  />
                  <ChangePath path={path?.display ?? "Unknown file"} />
                  {/* Named by its path; the status is its description. */}
                  <span id={`${statusId}-${index}`} hidden>
                    {gitStatusName(markForStatus(file.status))}
                  </span>
                </Button>
              </li>
            );
          })}
        </ul>
      )}
      {(page.nextCursor || pageIndex > 0) && (
        <nav
          className="git-commit-file-pages flex flex-none flex-wrap items-center gap-[8px] border-t border-border px-[12px] py-[8px] text-[12px]"
          aria-label="Commit file pages"
        >
          <span>
            {offset + 1}–{offset + page.entries.length}
          </span>
          <Button
            disabled={busy || pageIndex === 0}
            onClick={() => {
              setRequested(pageIndex - 1);
              setSelected(null);
            }}
          >
            Previous files
          </Button>
          <Button disabled={busy || !page.nextCursor} onClick={next}>
            Next files
          </Button>
        </nav>
      )}
    </section>
  );
  const fileDiff = (
    <>
      {selected ? (
        <CommitDiff
          key={selected.bytesB64}
          repoId={repoId}
          commitOid={commitOid}
          parentIndex={parentIndex}
          path={selected}
        />
      ) : (
        <p className="git-projects-empty">
          Select a file to inspect this commit.
        </p>
      )}
    </>
  );
  return (
    <>
      {error && (
        <GitNotice tone="error">
          {error}
          <Button
            onClick={() => {
              setCursors([]);
              setRequested(0);
              setSelected(null);
              void reads[0].refetch();
            }}
            disabled={busy}
          >
            Reload commit files
          </Button>
        </GitNotice>
      )}
      {!page ? (
        busy && (
          <p role="status" className="git-projects-empty">
            Loading commit files…
          </p>
        )
      ) : (
        <>
          {page.metadata.truncated && (
            <GitNotice tone="info">
              The commit’s file list exceeds the agent’s limit. Some files are
              not listed.
            </GitNotice>
          )}
          {/* The design gives this split a handle too, not only the Changes
              view: measured in the prototype as a resizer between the
              commit's file list and its diff. */}
          {stacked ? (
            <div className="flex min-h-[320px] flex-1 flex-col">
              {/* Stacked above the diff, the list stays a header rather than
                  becoming the view: the design caps it at 130px, and the
                  list inside scrolls. */}
              <div className="flex max-h-[130px] min-h-0 flex-col overflow-hidden border-b border-border">
                {fileList}
              </div>
              <div className="flex min-h-0 min-w-0 flex-1 flex-col">
                {fileDiff}
              </div>
            </div>
          ) : (
            <ListDiffPanels
              label="Resize the commit file list"
              className="min-h-[320px] flex-1"
              list={fileList}
              detail={fileDiff}
              listClassName="flex h-full flex-col"
              detailClassName="h-full min-w-0 overflow-y-auto"
            />
          )}
        </>
      )}
    </>
  );
}

function CommitDiff({
  repoId,
  commitOid,
  parentIndex,
  path,
}: {
  repoId: string;
  commitOid: string;
  parentIndex: number;
  path: GitPath;
}) {
  const scope = useCurrentServerScope();
  // A historical diff is fixed by the commit, so it is read once per file.
  const diff = useQuery(
    gitQueries.commitDiff(scope, { repoId, commitOid, parentIndex, path }),
  );
  const error =
    diff.isError && !diff.isFetching ? errorMessage(diff.error) : "";
  const stat = diff.data?.files.reduce(
    (total, file) => ({
      additions: total.additions + file.additions,
      deletions: total.deletions + file.deletions,
    }),
    { additions: 0, deletions: 0 },
  );
  return (
    <section
      aria-label="Historical file diff"
      className="flex h-full min-h-0 flex-col"
    >
      <header className="git-diff-toolbar flex h-[40px] flex-none items-center gap-[8px] border-b border-border bg-background px-[12px]">
        <ChangePath
          path={path.display}
          className="text-[13px] leading-[18px] font-medium"
        />
        {stat && (
          <DiffStat additions={stat.additions} deletions={stat.deletions} />
        )}
        <span className="flex-1" />
        <GitDiffSettings />
      </header>
      {error ? (
        <GitNotice tone="error">
          {error}
          <Button onClick={() => void diff.refetch()}>Retry diff</Button>
        </GitNotice>
      ) : diff.data ? (
        <GitDiffView diff={diff.data} />
      ) : (
        <p role="status" className="git-projects-empty">
          Loading historical diff…
        </p>
      )}
    </section>
  );
}
