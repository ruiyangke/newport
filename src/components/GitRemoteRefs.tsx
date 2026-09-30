import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { gitPath, type GitWriteAction } from "../domain/git";
import {
  type decodeGitRemoteRefs,
  type GitRemotes,
} from "../domain/gitResponses";
import { refreshQuery, readSharedQuery } from "../query/client";
import { gitKeys, gitQueries, invalidateRepository } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button, Input } from "./controls";
import { GitLoadMore } from "./GitLoadMore";
import { useGitPageLoader } from "../hooks/useGitPageLoader";
import { gitErrorMessage } from "../git/errors";

type Page = ReturnType<typeof decodeGitRemoteRefs>;
type Entry = Page["entries"][number];
type Source = { name: string; oid: string };
type Confirmation = { entry: Entry; snapshot: string } & (
  { kind: "delete" } | { kind: "push"; source: Source }
);
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
 * remote references", scrolling through the captured listing, and the re-listing after the user's
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
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => setFilter(search.trim().toLowerCase()), 200);
    return () => clearTimeout(timer);
  }, [search]);
  const filterPending = search.trim().toLowerCase() !== filter;
  const base = {
    repoId,
    remote: remote.name,
    expectedToken: remote.token,
    forPush: true,
    ...(filter ? { filter } : {}),
  };
  const first = useQuery({
    ...gitQueries.remoteRefs(scope, base),
    enabled: false,
  });
  /** Lists the remote afresh from its first page. Only ever on request. */
  function list() {
    setConfirmation(null);
    void refreshQuery(queryClient, gitQueries.remoteRefs(scope, base)).catch(
      () => undefined,
    );
  }
  // Opening this view is the request to list the remote; so is pointing it at
  // a different remote or configuration.
  useEffect(() => {
    setConfirmation(null);
    const controller = new AbortController();
    void readSharedQuery(
      queryClient,
      {
        ...gitQueries.remoteRefs(scope, {
          repoId,
          remote: remote.name,
          expectedToken: remote.token,
          forPush: true,
          ...(filter ? { filter } : {}),
        }),
        staleTime: 0,
      },
      controller.signal,
    ).catch(() => undefined);
    return () => controller.abort();
  }, [queryClient, scope, repoId, remote.name, remote.token, filter]);
  // A listing being (re)read is not shown, as before: nothing is offered from
  // it until the read that replaces it has landed.
  const reading = filterPending || first.isFetching || !first.isSuccess;
  const page = reading ? null : (first.data ?? null);
  const pages = useGitPageLoader({
    queryKey: gitKeys.remoteRefs(scope, base),
    page,
    enabled: !reading && !first.isError && !confirmation,
    read: (cursor, signal) =>
      readSharedQuery(
        queryClient,
        { ...gitQueries.remoteRefs(scope, { ...base, cursor }), staleTime: 0 },
        signal,
      ),
    entryKey: (entry) => entry.reference.bytesB64,
    prefetch: true,
  });
  const loading = filterPending || first.isFetching;
  const error =
    !loading && first.error
      ? gitErrorMessage(first.error, "Could not load remote references.")
      : "";
  const stale =
    !!confirmation &&
    (confirmation.snapshot !== snapshot ||
      (confirmation.kind === "push" &&
        (confirmation.source.name !== source?.name ||
          confirmation.source.oid !== source?.oid)));
  const blocked = disabled || loading || !snapshot || stale;
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
          <Input
            aria-label="Filter remote references"
            placeholder="Filter remote branches and tags"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="none"
          />
          {!source && (
            <p>Switch to a local branch with a commit to push with lease.</p>
          )}
          {loading && <p role="status">Loading remote references…</p>}
          <div className="git-branch-list">
            <ul>
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
            {page && page.entries.length === 0 && (
              <p>
                {filter
                  ? "No matching remote references."
                  : "No remote references."}
              </p>
            )}
            {page?.metadata.truncated && (
              <p role="status">This listing is incomplete.</p>
            )}
            {page && (
              <GitLoadMore
                cursor={page.nextCursor}
                loading={pages.loading}
                error={pages.error}
                disabled={loading}
                onLoad={() => void pages.load()}
                label="Load more references"
                endLabel={
                  filter
                    ? "All matching references loaded"
                    : "All remote references loaded"
                }
              />
            )}
          </div>
          <footer className="git-remote-refs-footer flex-wrap gap-[8px]">
            <div className="git-remote-refs-actions flex flex-wrap gap-[8px]">
              <Button disabled={busy} onClick={onBack}>
                Back to remotes
              </Button>
              <Button disabled={loading} onClick={list}>
                Refresh remote references
              </Button>
            </div>
          </footer>
        </>
      )}
    </div>
  );
}
