import { lazy, Suspense, useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { appendGitBlobPage } from "../domain/gitResponses";
import { createBlobTextReader } from "../git/blobText";
import { gitErrorMessage } from "../git/errors";
import { gitProjectsFor } from "../git/registry";
import { useGitPageLoader } from "../hooks/useGitPageLoader";
import { gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button } from "./controls";
import { GitLoadMore } from "./GitLoadMore";
import { GitNotice } from "./GitNotice";
const GitBlobEditor = lazy(() => import("./GitBlobEditor"));

export function GitBlobPreview({
  repoId,
  oid,
  label,
}: {
  repoId: string;
  oid: string;
  label: string;
}) {
  const scope = useCurrentServerScope();
  const params = { repoId, oid, maxBytes: 65536 };
  const query = gitQueries.blobPage(scope, params);
  const blob = useQuery(query);
  const page = blob.data ?? null;
  const readText = useMemo(() => createBlobTextReader(), []);
  const text = useMemo(() => (page ? readText(page) : null), [page, readText]);
  const pages = useGitPageLoader({
    queryKey: query.queryKey,
    page,
    enabled: !!page && text !== null && !blob.isFetching && !blob.isError,
    read: (cursor, signal) =>
      gitProjectsFor(scope)
        .repositories.withSignal(signal)
        .blobPage({
          ...params,
          cursor,
          maxBytes: 524288,
        }),
    entryKey: (entry) => String(entry.offset),
    merge: appendGitBlobPage,
    prefetch: true,
  });
  return (
    <div
      className="git-diff-code max-h-[360px] overflow-auto"
      role="region"
      aria-label={label}
      tabIndex={0}
    >
      {blob.isError ? (
        <GitNotice tone="error">
          {gitErrorMessage(blob.error)}
          <Button onClick={() => void blob.refetch()}>Retry preview</Button>
        </GitNotice>
      ) : !page ? (
        <p className="git-projects-empty" role="status">
          Loading…
        </p>
      ) : text === null ? (
        <GitNotice tone="info">
          This side contains binary data. It can still be chosen.
        </GitNotice>
      ) : (
        <>
          <Suspense fallback={<p role="status">Preparing preview…</p>}>
            <GitBlobEditor text={text} label={label} />
          </Suspense>
          <GitLoadMore
            cursor={page.nextCursor}
            loading={pages.loading}
            error={pages.error}
            disabled={blob.isFetching}
            onLoad={() => void pages.load()}
            label="Load more content"
            endLabel="All content loaded"
          />
        </>
      )}
    </div>
  );
}
