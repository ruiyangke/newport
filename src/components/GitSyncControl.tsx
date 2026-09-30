import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, ChevronDown, RefreshCw } from "lucide-react";
import type { GitWriteAction } from "../domain/git";
import type { GitRepository, GitStatus } from "../domain/gitResponses";
import { gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { ButtonGroup, ButtonGroupText } from "./ui/button-group";
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

  /**
   * Resolve the remote's configuration token only when an action is actually
   * requested. Reading it on every status refresh would spend a round trip on
   * the server for something the user may never use. The read is the same
   * cached remotes query the Remotes dialog shows: a fresh copy answers without
   * a round trip, and a stale or retired one is read again rather than being
   * held for the life of the toolbar.
   */
  async function ensureToken(remote: string) {
    const value = await queryClient.fetchQuery(
      gitQueries.remotes(scope, repository.repoId),
    );
    return value.entries.find((entry) => entry.name === remote)?.token ?? null;
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
    const expectedToken = await ensureToken(upstream.remote);
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
    /*
     * One outlined group on the toolbar's trailing edge, as tall as the other
     * toolbar controls: the action, what is outstanding, and the other
     * transfers. The upstream and the "from stored refs" caveat moved off a
     * second line that truncated ("origin/main · from st…") into the tooltip
     * and the footer.
     *
     * `!` only where `src/styles.css` has an UNLAYERED `[data-slot="button"]`
     * claim (height, radius, padding-inline): that rule's 5px radius would
     * otherwise round the joined inner corners the group squares off.
     */
    <ButtonGroup
      className="git-sync h-[30px] flex-none"
      title={reason ?? (error || basis)}
    >
      <Button
        className="h-[30px]! min-w-0 gap-[6px] rounded-r-none! px-[10px]! text-[12px] font-medium text-foreground"
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
      </Button>
      {(ahead > 0 || behind > 0) && (
        <ButtonGroupText
          className="h-[30px] gap-[6px] rounded-none border-border bg-(--native-surface) px-[8px] text-[11px] font-medium text-muted-foreground tabular-nums"
          aria-label={`${ahead} ahead, ${behind} behind, from stored refs`}
        >
          {ahead > 0 && (
            <span className="flex items-center gap-[1px]">
              <ArrowUp size={11} aria-hidden="true" />
              {ahead}
            </span>
          )}
          {behind > 0 && (
            <span className="flex items-center gap-[1px]">
              <ArrowDown size={11} aria-hidden="true" />
              {behind}
            </span>
          )}
        </ButtonGroupText>
      )}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            className="h-[30px]! w-[26px] rounded-l-none! p-0! text-muted-foreground"
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
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </ButtonGroup>
  );
}
