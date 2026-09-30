import { gitErrorMessage } from "../git/errors";
import {
  useId,
  useMemo,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { Check, Copy } from "lucide-react";
import { cn } from "cn";
import { useQuery } from "@tanstack/react-query";
import type { GitPath } from "../domain/git";
import { appendGitDiffPage } from "../domain/gitResponses";
import { createDiffRenderer } from "../git/historicalDiff";
import type { GitHistory } from "../domain/gitResponses";
import { useGitPageLoader } from "../hooks/useGitPageLoader";
import { GitLoadMore } from "./GitLoadMore";
import { gitProjectsFor } from "../git/registry";
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
  commit: preview,
  actions,
}: {
  repoId: string;
  commit: Commit;
  /** The commit's own actions, drawn in its header. */
  actions?: ReactNode | ((commit: Commit) => ReactNode);
}) {
  const scope = useCurrentServerScope();
  const detail = useQuery({
    ...gitQueries.commit(scope, repoId, preview.oid.hex),
    enabled: preview.messageTruncated,
  });
  const commit = detail.data ?? preview;
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
                {detail.isFetching
                  ? "Loading full commit message…"
                  : "The commit message is truncated."}
              </p>
            )}
            {detail.isError && !detail.isFetching && (
              <div
                role="alert"
                className="mt-[6px] text-[12px] text-destructive"
              >
                {gitErrorMessage(detail.error)}
                <Button onClick={() => void detail.refetch()}>
                  Retry commit message
                </Button>
              </div>
            )}
          </div>
          {typeof actions === "function" ? actions(commit) : actions}
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
  const stacked = useStacked();
  const statusId = useId();
  const params = { repoId, commitOid, parentIndex };
  const read = useQuery(gitQueries.commitFiles(scope, params));
  const page = read.data ?? null;
  const busy = read.isFetching;
  const error = read.isError && !busy ? gitErrorMessage(read.error) : "";
  const pages = useGitPageLoader({
    queryKey: gitQueries.commitFiles(scope, params).queryKey,
    page,
    enabled: !busy && !read.isError,
    read: (cursor, signal) =>
      gitProjectsFor(scope)
        .repositories.withSignal(signal)
        .commitFiles({ ...params, cursor }),
    entryKey: (file) =>
      JSON.stringify([file.oldPath?.bytesB64, file.newPath?.bytesB64]),
    prefetch: true,
  });
  // Appending pages preserves the selection and does not request another diff.
  // A refreshed comparison retires the selection with its old snapshot.
  const [picked, setPicked] = useState<{
    snapshot: string;
    path: GitPath;
  } | null>(null);
  const first = page?.entries.find((file) => file.newPath ?? file.oldPath);
  const selected =
    picked?.snapshot === page?.snapshot
      ? (picked?.path ?? null)
      : (first?.newPath ?? first?.oldPath ?? null);
  const setSelected = (path: GitPath | null) =>
    setPicked(path && page ? { snapshot: page.snapshot, path } : null);
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
          <li>
            <GitLoadMore
              cursor={page.nextCursor}
              loading={pages.loading}
              error={pages.error}
              disabled={busy}
              onLoad={() => void pages.load()}
              label="Load more files"
              endLabel="All commit files loaded"
            />
          </li>
        </ul>
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
              setSelected(null);
              void read.refetch();
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
  const params = {
    repoId,
    commitOid,
    parentIndex,
    path,
    pageSize: 5000,
    maxBytes: 65536,
  };
  const query = gitQueries.commitDiffPage(scope, params);
  const diff = useQuery(query);
  const page = diff.data ?? null;
  const pages = useGitPageLoader({
    queryKey: query.queryKey,
    page,
    enabled: !diff.isFetching && !diff.isError,
    read: (cursor, signal) =>
      gitProjectsFor(scope)
        .repositories.withSignal(signal)
        .commitDiffPage({
          ...params,
          cursor,
          maxBytes: 524288,
        }),
    entryKey: (file) => String(file.fileIndex),
    merge: appendGitDiffPage,
    prefetch: true,
  });
  const renderDiff = useMemo(() => createDiffRenderer(), []);
  const rendered = useMemo(
    () => (page ? renderDiff(page) : null),
    [page, renderDiff],
  );
  const footer = page && (
    <GitLoadMore
      cursor={page.nextCursor}
      loading={pages.loading}
      error={pages.error}
      disabled={diff.isFetching}
      onLoad={() => void pages.load()}
      label="Load more changes"
      endLabel="All changes loaded"
    />
  );
  const error =
    diff.isError && !diff.isFetching ? errorMessage(diff.error) : "";
  const stat = rendered?.files.reduce(
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
      ) : rendered ? (
        <>
          {page?.entries.some(
            (file) => file.omissionReason === "file_size_limit",
          ) && (
            <GitNotice tone="info">
              This file exceeds the agent’s text diff size limit.
            </GitNotice>
          )}
          {page?.entries.at(-1)?.hunks.at(-1)?.lines.at(-1)?.lineComplete ===
            false && (
            <p
              role="status"
              className="px-[12px] text-[11px] text-muted-foreground"
            >
              A long line continues on the next page.
            </p>
          )}
          {page?.entries.every(
            (file) => file.omissionReason === "file_size_limit",
          ) ? (
            footer
          ) : (
            <GitDiffView diff={rendered} footer={footer} />
          )}
        </>
      ) : (
        <p role="status" className="git-projects-empty">
          Loading historical diff…
        </p>
      )}
    </section>
  );
}
