import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { gitPath, type GitWriteAction } from "../domain/git";
import {
  type decodeGitRemoteRefs,
  type GitRemotes,
} from "../domain/gitResponses";
import { refreshQuery } from "../query/client";
import { gitKeys, gitQueries, invalidateRepository } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button } from "./controls";
import { Pagination, PaginationContent, PaginationItem } from "./ui/pagination";

type Page = ReturnType<typeof decodeGitRemoteRefs>;
type Entry = Page["entries"][number];
type Source = { name: string; oid: string };
type Confirmation = { entry: Entry; snapshot: string } & (
  { kind: "delete" } | { kind: "push"; source: Source }
);
/**
 * Where the reader is in the paged listing. Each page is its own query, keyed
 * by its cursor; this only remembers which cursors were visited. It belongs to
 * one reading of the first page (`basis`, that read's time), so a fresh first
 * page starts the listing over, as a refresh always has.
 */
type Trail = {
  basis: number;
  cursors: string[];
  index: number;
  pending: boolean;
  error: string;
};
const startTrail = (basis: number): Trail => ({
  basis,
  cursors: [],
  index: 0,
  pending: false,
  error: "",
});
function deletable(entry: Entry) {
  const prefix =
    entry.kind === "branch"
      ? "refs/heads/"
      : entry.kind === "tag"
        ? "refs/tags/"
        : null;
  try {
    return (
      !!prefix &&
      !entry.symbolicTarget &&
      !/^0+$/.test(entry.oid.hex) &&
      entry.reference.display.startsWith(prefix) &&
      gitPath(entry.reference.display).bytesB64 === entry.reference.bytesB64
    );
  } catch {
    return false;
  }
}
/**
 * The one Git read that contacts the remote. Newport promises it has not done
 * so unless asked, so this listing is only ever read on an explicit request:
 * opening this view (the user chose "Remote branches and tags…"), "Refresh
 * remote references", "Next references", and the re-listing after the user's
 * own push or deletion here. Its queries stay disabled, so nothing else -- a
 * remount, a timer, a write that retires the repository's reads -- re-runs it.
 */
export function GitRemoteRefs({
  repoId,
  remote,
  snapshot,
  disabled,
  busy,
  source,
  onAction,
  onBack,
}: {
  /** Still passed by the page; reads go through the session's registry. */
  repoId: string;
  remote: GitRemotes["entries"][number];
  snapshot?: string;
  disabled: boolean;
  busy: boolean;
  source?: Source;
  onAction: (action: GitWriteAction) => Promise<boolean>;
  onBack: () => void;
}) {
  const scope = useCurrentServerScope();
  const queryClient = useQueryClient();
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const base = {
    repoId,
    remote: remote.name,
    expectedToken: remote.token,
    forPush: true,
  };
  const first = useQuery({
    ...gitQueries.remoteRefs(scope, base),
    enabled: false,
  });
  const [storedTrail, setTrail] = useState(() => startTrail(0));
  const trail =
    storedTrail.basis === first.dataUpdatedAt
      ? storedTrail
      : startTrail(first.dataUpdatedAt);
  const cursor = trail.index === 0 ? undefined : trail.cursors[trail.index - 1];
  const later = useQuery({
    ...gitQueries.remoteRefs(scope, { ...base, cursor }),
    enabled: false,
  });
  /** Lists the remote afresh from its first page. Only ever on request. */
  function list() {
    setTrail(startTrail(-1));
    setConfirmation(null);
    void refreshQuery(queryClient, gitQueries.remoteRefs(scope, base)).catch(
      () => undefined,
    );
  }
  // Opening this view is the request to list the remote; so is pointing it at
  // a different remote or configuration.
  useEffect(() => {
    setTrail(startTrail(-1));
    setConfirmation(null);
    void refreshQuery(
      queryClient,
      gitQueries.remoteRefs(scope, {
        repoId,
        remote: remote.name,
        expectedToken: remote.token,
        forPush: true,
      }),
    ).catch(() => undefined);
  }, [queryClient, scope, repoId, remote.name, remote.token]);
  // A listing being (re)read is not shown, as before: nothing is offered from
  // it until the read that replaces it has landed.
  const reading = first.isFetching || !first.isSuccess;
  const page = reading
    ? undefined
    : trail.index === 0
      ? first.data
      : later.data;
  const loading = first.isFetching || trail.pending;
  const error =
    trail.error ||
    (!first.isFetching && first.error ? String(first.error) : "");
  const stale =
    !!confirmation &&
    (confirmation.snapshot !== snapshot ||
      (confirmation.kind === "push" &&
        (confirmation.source.name !== source?.name ||
          confirmation.source.oid !== source?.oid)));
  const blocked = disabled || loading || !snapshot || stale;
  async function next() {
    if (!page?.nextCursor || loading || disabled) return;
    const visited = trail.cursors[trail.index];
    if (
      visited !== undefined &&
      queryClient.getQueryData(
        gitKeys.remoteRefs(scope, { ...base, cursor: visited }),
      )
    ) {
      setTrail({ ...trail, index: trail.index + 1 });
      return;
    }
    const nextCursor = page.nextCursor;
    const requested: Trail = { ...trail, pending: true, error: "" };
    setTrail(requested);
    let outcome: Partial<Trail>;
    try {
      const result = await refreshQuery(
        queryClient,
        gitQueries.remoteRefs(scope, { ...base, cursor: nextCursor }),
      );
      if (
        result.snapshot !== page.snapshot ||
        JSON.stringify(result.metadata) !== JSON.stringify(page.metadata)
      )
        throw new Error(
          "Remote listing changed. Refresh the remote references.",
        );
      outcome = {
        cursors: [...trail.cursors.slice(0, trail.index), nextCursor],
        index: trail.index + 1,
      };
    } catch (reason) {
      outcome = { error: String(reason) };
    }
    // A page that lands after the listing started over belongs to a listing
    // that is no longer shown. It stays cached under its own cursor, unused.
    setTrail((current) =>
      current === requested
        ? { ...current, ...outcome, pending: false }
        : current,
    );
  }
  return (
    <div className="git-project-form git-remote-refs min-w-0">
      <p>
        Remote <strong>{remote.name}</strong> ·{" "}
        {remote.pushUrl ?? remote.url ?? "Address unavailable"}
      </p>
      {error && <p role="alert">{error}</p>}
      {confirmation ? (
        <>
          {confirmation.kind === "push" ? (
            <>
              <p>
                Replace remote branch{" "}
                <strong>{confirmation.entry.reference.display}</strong> with
                local branch <strong>{confirmation.source.name}</strong>?
              </p>
              <p>
                This can remove commits from shared remote history. Other people
                may need to reconcile their work. Your local branch stays
                unchanged.
              </p>
              <p>
                Local commit: <code>{confirmation.source.oid}</code>
              </p>
              <p>
                Expected remote commit:{" "}
                <code>{confirmation.entry.oid.hex}</code>. Push with lease
                refuses the replacement if the remote branch has changed.
              </p>
            </>
          ) : (
            <>
              <p>
                Delete remote {confirmation.entry.kind}{" "}
                <strong>{confirmation.entry.reference.display}</strong>?
              </p>
              <p>
                This removes the reference from the shared remote repository.
                Other people may rely on it. Your local branches and tags are
                kept.
              </p>
              <p>
                Expected object: <code>{confirmation.entry.oid.hex}</code>.
                Deletion is refused if the remote reference has changed.
              </p>
            </>
          )}
          {stale && (
            <p role="alert">
              The repository changed. Go back and inspect the remote reference
              again.
            </p>
          )}
          <footer className="flex-wrap">
            <Button disabled={busy} onClick={() => setConfirmation(null)}>
              Back to remote references
            </Button>
            <Button
              variant="destructive"
              disabled={blocked}
              onClick={async () => {
                if (blocked || !deletable(confirmation.entry)) return;
                const entry = confirmation.entry;
                const guard = {
                  remote: remote.name,
                  expectedToken: remote.token,
                  expectedOid: entry.oid.hex,
                };
                const action: GitWriteAction =
                  confirmation.kind === "push"
                    ? {
                        kind: "push.with_lease",
                        remote: remote.name,
                        expectedToken: remote.token,
                        branch: confirmation.source.name,
                        expectedOid: confirmation.source.oid,
                        destinationBranch: entry.reference.display.slice(
                          "refs/heads/".length,
                        ),
                        expectedRemoteOid: entry.oid.hex,
                      }
                    : entry.kind === "branch"
                      ? {
                          kind: "branch.delete_remote",
                          ...guard,
                          branch: entry.reference.display.slice(
                            "refs/heads/".length,
                          ),
                        }
                      : {
                          kind: "tag.delete_remote",
                          ...guard,
                          name: entry.reference.display.slice(
                            "refs/tags/".length,
                          ),
                        };
                if (await onAction(action)) {
                  // The user changed the remote: retire what this repository
                  // had read, and list the remote again as this view always
                  // has after its own push or deletion.
                  void invalidateRepository(queryClient, scope, repoId);
                  list();
                }
              }}
            >
              {confirmation.kind === "push"
                ? "Replace remote branch"
                : `Delete remote ${confirmation.entry.kind}`}
            </Button>
          </footer>
        </>
      ) : (
        <>
          <p>
            Live references from the push destination. Refresh to inspect
            changes made since this listing was captured.
          </p>
          {!source && (
            <p>Switch to a local branch with a commit to push with lease.</p>
          )}
          {loading && <p role="status">Loading remote references…</p>}
          <ul className="git-branch-list">
            {page?.entries.map((entry) => (
              <li key={entry.reference.bytesB64}>
                <div>
                  <strong>{entry.reference.display}</strong>
                  <small>
                    {entry.kind.replace("_", " ")} ·{" "}
                    {entry.oid.hex.slice(0, 12)}
                    {entry.symbolicTarget
                      ? ` → ${entry.symbolicTarget.display}`
                      : ""}
                  </small>
                </div>
                {deletable(entry) && (
                  <>
                    {entry.kind === "branch" && (
                      <Button
                        disabled={blocked || !source}
                        onClick={() =>
                          snapshot &&
                          source &&
                          setConfirmation({
                            kind: "push",
                            entry,
                            snapshot,
                            source: { ...source },
                          })
                        }
                      >
                        Push with lease…
                      </Button>
                    )}
                    <Button
                      disabled={blocked}
                      onClick={() =>
                        snapshot &&
                        setConfirmation({ kind: "delete", entry, snapshot })
                      }
                    >
                      Delete…
                    </Button>
                  </>
                )}
              </li>
            ))}
          </ul>
          {page && page.entries.length === 0 && <p>No remote references.</p>}
          {page?.metadata.truncated && (
            <p role="status">This listing is incomplete.</p>
          )}
          {/*
           * Paging and actions are grouped separately so a footer too narrow
           * for all four does not strand "Next references" on a row of its own,
           * away from the "Previous" it belongs with.
           */}
          {/* No justify-content here: `.git-project-form footer` in projects.css
              is unlayered and has always set flex-end, so a utility would be
              inert and only suggest a layout that is not drawn. */}
          <footer className="git-remote-refs-footer flex-wrap gap-[8px]">
            <Pagination
              aria-label="Remote reference pages"
              className="git-remote-refs-pages mx-0 flex w-auto flex-wrap justify-normal gap-[8px]"
            >
              <PaginationContent className="flex-wrap gap-[8px]">
                <PaginationItem>
                  <Button
                    disabled={disabled || loading || trail.index === 0}
                    onClick={() =>
                      setTrail({ ...trail, index: trail.index - 1 })
                    }
                  >
                    Previous references
                  </Button>
                </PaginationItem>
                <PaginationItem>
                  <Button
                    disabled={disabled || loading || !page?.nextCursor}
                    onClick={() => void next()}
                  >
                    Next references
                  </Button>
                </PaginationItem>
              </PaginationContent>
            </Pagination>
            <div className="git-remote-refs-actions flex flex-wrap gap-[8px]">
              <Button disabled={busy} onClick={onBack}>
                Back to remotes
              </Button>
              <Button disabled={disabled || loading} onClick={list}>
                Refresh remote references
              </Button>
            </div>
          </footer>
        </>
      )}
    </div>
  );
}
