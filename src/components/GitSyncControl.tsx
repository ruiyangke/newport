import { gitErrorMessage } from "../git/errors";
import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, ChevronDown, RefreshCw } from "lucide-react";
import type { GitWriteAction } from "../domain/git";
import type { GitRepository, GitStatus } from "../domain/gitResponses";
import { gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button } from "./controls";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";

/** `refs/remotes/origin/main` → `origin` and `main`. */
function upstreamParts(reference: string) {
  const rest = reference.replace(/^refs\/remotes\//, "");
  const slash = rest.indexOf("/");
  if (slash <= 0) return null;
  return { remote: rest.slice(0, slash), branch: rest.slice(slash + 1) };
}

/**
 * The toolbar's transfer control. Counts come from stored refs, so this never
 * claims the remote was contacted — it names the comparison instead.
 */
export function GitSyncControl({
  repository,
  status,
  busy,
  blockedReason,
  onAction,
}: {
  /** Still passed by the page; reads go through the session's registry. */
  repository: GitRepository;
  status: GitStatus | null;
  busy: boolean;
  blockedReason?: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
}) {
  const scope = useCurrentServerScope();
  const queryClient = useQueryClient();
  const [error, setError] = useState("");
  const pending = useRef(false);
  const metadata = status?.metadata;
  // Depend on the reference string, never on a parsed object: a fresh object
  // every render would re-run this read forever and never let the toolbar settle.
  const upstreamRef = metadata?.upstreamRef?.display ?? null;
  const upstream = upstreamRef ? upstreamParts(upstreamRef) : null;

  // Fetch only the selected remote when needed. Repository writes invalidate
  // this query together with the other repository reads.
  async function ensureToken(remote: string) {
    const value = await queryClient.fetchQuery(
      gitQueries.remote(scope, repository.repoId, remote),
    );
    return value.token;
  }

  const ahead = metadata?.ahead ?? 0;
  const behind = metadata?.behind ?? 0;
  const head = metadata?.head ?? repository.head;
  const branch = head.name?.display.replace(/^refs\/heads\//, "") ?? null;
  const writable =
    !!status &&
    !repository.capabilities.readOnly &&
    !repository.bare &&
    !!branch &&
    !head.detached &&
    !head.unborn;
  const reason =
    blockedReason ??
    (!upstream
      ? "This branch has no upstream to compare with."
      : !writable
        ? "This repository cannot be written from here."
        : undefined);
  const disabled = busy || !!reason;

  async function run(action: GitWriteAction) {
    if (disabled || pending.current) return;
    pending.current = true;
    setError("");
    try {
      await onAction(action);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      pending.current = false;
    }
  }
  async function transfer(kind: "fetch" | "pull" | "push") {
    if (!upstream) return;
    let expectedToken: string;
    try {
      expectedToken = await ensureToken(upstream.remote);
    } catch (cause) {
      setError(gitErrorMessage(cause));
      return;
    }
    if (!expectedToken) {
      setError("This remote’s configuration could not be read.");
      return;
    }
    if (kind === "fetch")
      return run({
        kind: "fetch",
        remote: upstream.remote,
        expectedToken,
      });
    if (kind === "pull")
      return run({
        kind: "pull.fast_forward",
        remote: upstream.remote,
        expectedToken,
        remoteBranch: upstream.branch,
      });
    if (!branch || !head.oid) return;
    return run({
      kind: "push",
      remote: upstream.remote,
      expectedToken,
      branch,
      expectedOid: head.oid.hex,
      destinationBranch: upstream.branch,
    });
  }

  // Lead with whatever the stored comparison says is outstanding.
  const primary =
    behind > 0
      ? ({ label: "Pull", kind: "pull" } as const)
      : ahead > 0
        ? ({ label: "Push", kind: "push" } as const)
        : ({ label: "Fetch", kind: "fetch" } as const);
  const target = upstream ? upstream.remote : "origin";

  // Where the counts come from, said once, on hover: they compare stored refs
  // and never mean the remote was contacted.
  const basis = upstream
    ? `Compared with ${upstream.remote}/${upstream.branch} as last fetched; Newport has not contacted the remote.`
    : undefined;

  return (
    <div
      role="group"
      aria-label="Remote transfers"
      className="git-sync inline-flex h-[30px] flex-none items-center rounded-[5px] border border-border bg-(--native-surface)"
      title={reason ?? (error || basis)}
    >
      <Button
        className="h-[28px]! min-w-0 gap-[6px] rounded-r-none! border-0 px-[10px]! text-[12px] font-medium text-foreground shadow-none"
        disabled={disabled}
        aria-label={`${primary.label} ${target}`}
        onClick={() => void transfer(primary.kind)}
      >
        {primary.label === "Pull" ? (
          <ArrowDown size={14} aria-hidden="true" />
        ) : primary.label === "Push" ? (
          <ArrowUp size={14} aria-hidden="true" />
        ) : (
          <RefreshCw size={14} aria-hidden="true" />
        )}
        <span className="truncate">
          {primary.label} {target}
        </span>
        {(behind > 0 || ahead > 0) && (
          <span
            className="ml-[2px] text-[11px] text-muted-foreground tabular-nums"
            aria-label={`${behind > 0 ? behind : ahead} commits to ${primary.kind}`}
          >
            {behind > 0 ? behind : ahead}
          </span>
        )}
      </Button>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            className="h-[28px]! w-[26px] rounded-l-none! border-0 p-0! text-muted-foreground shadow-none"
            disabled={busy}
            aria-label="Transfer options"
          >
            <ChevronDown size={13} aria-hidden="true" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-[240px]">
          <DropdownMenuItem
            disabled={disabled}
            onSelect={() => void transfer("fetch")}
          >
            <RefreshCw size={14} aria-hidden="true" />
            Fetch {target}
          </DropdownMenuItem>
          <DropdownMenuItem
            disabled={disabled}
            onSelect={() => void transfer("pull")}
          >
            <ArrowDown size={14} aria-hidden="true" />
            Pull {target} (fast-forward only)
          </DropdownMenuItem>
          <DropdownMenuItem
            disabled={disabled}
            onSelect={() => void transfer("push")}
          >
            <ArrowUp size={14} aria-hidden="true" />
            Push {branch ?? "branch"} to {target}
            {ahead > 0 ? ` (${ahead})` : ""}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
