import { queryOptions, type QueryClient } from "@tanstack/react-query";
import type {
  GitBranchOptions,
  GitStatusFilter,
  GitProject,
  GitWorktreeOptions,
} from "../domain/git";
import type { GitRepositoryClient } from "../api/gitRepository";
import { gitProjectsFor } from "../git/registry";
import { keys, type ServerScope } from "./keys";
import { readIPC } from "./client";
import { type GitStatus } from "../domain/gitResponses";

type Params<M extends keyof GitRepositoryClient> =
  GitRepositoryClient[M] extends (params: infer P) => unknown ? P : never;

/*
 * Every Git read for a repository sits under `…/git/repo/<repoId>`, so a write
 * can retire all of them with one prefix: the status the write was checked
 * against, and whatever panel happens to be showing branches, stashes or tags.
 * Before this, each of those panels carried a `reload` counter of its own and
 * the page re-read status by hand, and they did not always agree.
 *
 * Repository ids are stateless tokens (docs/git-protocol.md §3a): they name
 * the repository itself, not a slot in one connection, so a cached read stays
 * valid across reconnects.
 */
export function normalizeStatusFilter(
  filter?: GitStatusFilter,
): GitStatusFilter | undefined {
  const text = filter?.text.trim().toLowerCase() ?? "";
  const group = filter?.group ?? "all";
  return text || group !== "all" ? { text, group } : undefined;
}

export const gitKeys = {
  all: (scope: ServerScope) => [...keys.server(scope), "git"] as const,
  projects: (scope: ServerScope) =>
    [...gitKeys.all(scope), "projects"] as const,
  receipts: (scope: ServerScope) =>
    [...gitKeys.all(scope), "receipts"] as const,
  summaries: (scope: ServerScope) =>
    [...gitKeys.all(scope), "summary"] as const,
  summary: (scope: ServerScope, projectId: string) =>
    [...gitKeys.summaries(scope), projectId] as const,
  repos: (scope: ServerScope) => [...gitKeys.all(scope), "repo"] as const,
  repo: (scope: ServerScope, repoId: string) =>
    [...gitKeys.repos(scope), repoId] as const,
  status: (scope: ServerScope, repoId: string, filter?: GitStatusFilter) =>
    [
      ...gitKeys.repo(scope, repoId),
      "status",
      ...(normalizeStatusFilter(filter) ? [normalizeStatusFilter(filter)] : []),
    ] as const,
  history: (scope: ServerScope, repoId: string, revision = "HEAD") =>
    [...gitKeys.repo(scope, repoId), "history", revision] as const,
  commit: (scope: ServerScope, repoId: string, commitOid: string) =>
    [
      ...gitKeys.repo(scope, repoId),
      "commit",
      commitOid.toLowerCase(),
    ] as const,
  branchRoot: (scope: ServerScope, repoId: string) =>
    [...gitKeys.repo(scope, repoId), "branches"] as const,
  branches: (
    scope: ServerScope,
    repoId: string,
    cursor?: string,
    options?: GitBranchOptions,
  ) =>
    [
      ...gitKeys.branchRoot(scope, repoId),
      cursor ?? null,
      ...(options
        ? [
            {
              filter: options.filter ?? "",
              branchKind: options.branchKind ?? "all",
              pageSize: options.pageSize ?? 100,
            },
          ]
        : []),
    ] as const,
  remote: (scope: ServerScope, repoId: string, name: string) =>
    [...gitKeys.repo(scope, repoId), "remote", name] as const,
  remotes: (scope: ServerScope, repoId: string) =>
    [...gitKeys.repo(scope, repoId), "remotes"] as const,
  stashes: (scope: ServerScope, repoId: string, cursor?: string) =>
    [...gitKeys.repo(scope, repoId), "stashes", cursor ?? null] as const,
  tag: (scope: ServerScope, repoId: string, oid: string) =>
    [...gitKeys.repo(scope, repoId), "tag", oid.toLowerCase()] as const,
  tags: (scope: ServerScope, repoId: string, cursor?: string) =>
    [...gitKeys.repo(scope, repoId), "tags", cursor ?? null] as const,
  worktrees: (
    scope: ServerScope,
    repoId: string,
    cursor?: string,
    options?: GitWorktreeOptions,
  ) =>
    [
      ...gitKeys.repo(scope, repoId),
      "worktrees",
      cursor ?? null,
      options ?? {},
    ] as const,
  blobPage: (scope: ServerScope, params: Params<"blobPage">) =>
    [
      ...gitKeys.repo(scope, params.repoId),
      "blob-page",
      { ...params, oid: params.oid.toLowerCase() },
    ] as const,
  blob: (scope: ServerScope, repoId: string, oid: string) =>
    [...gitKeys.repo(scope, repoId), "blob", oid.toLowerCase()] as const,
  commitFiles: (scope: ServerScope, params: Params<"commitFiles">) =>
    [...gitKeys.repo(scope, params.repoId), "commit-files", params] as const,
  diff: (scope: ServerScope, params: Params<"diff">) =>
    [...gitKeys.repo(scope, params.repoId), "diff", params] as const,
  diffPage: (scope: ServerScope, params: Params<"diffPage">) =>
    [...gitKeys.repo(scope, params.repoId), "diff-page", params] as const,
  commitDiffPage: (scope: ServerScope, params: Params<"commitDiffPage">) =>
    [
      ...gitKeys.repo(scope, params.repoId),
      "commit-diff-page",
      params,
    ] as const,
  commitDiff: (scope: ServerScope, params: Params<"commitDiff">) =>
    [...gitKeys.repo(scope, params.repoId), "commit-diff", params] as const,
  remoteNames: (scope: ServerScope, params: Params<"remoteNames">) =>
    [...gitKeys.repo(scope, params.repoId), "remote-names", params] as const,
  remoteRefs: (scope: ServerScope, params: Params<"remoteRefs">) =>
    [...gitKeys.repo(scope, params.repoId), "remote-refs", params] as const,
};

/** The session's repository client, resolved when the read runs. */
function repositories(scope: ServerScope) {
  return gitProjectsFor(scope).repositories;
}
function read<T>(
  scope: ServerScope,
  run: (client: GitRepositoryClient) => Promise<T>,
) {
  return ({ signal }: { signal: AbortSignal }) =>
    readIPC(signal, () => run(repositories(scope).withSignal(signal)));
}

// Content-addressed reads cannot go stale: a blob or a commit diff is fixed by
// its object id, and a working diff is fixed by the snapshot it was read at.
const IMMUTABLE = { staleTime: Infinity } as const;

/** A checkout's branch, changed-file count and commits ahead, read now. */
async function readSummary(
  scope: ServerScope,
  project: GitProject,
  signal: AbortSignal,
) {
  if (project.serverId !== scope.id)
    throw new Error("Select this project's server before reading its summary.");
  const status = await gitProjectsFor(scope)
    .repositories.withSignal(signal)
    .statusSummary(project.path);
  const head = status.head;
  return {
    // An unborn HEAD still names the branch it is on; only a detached HEAD is
    // genuinely on none.
    branch:
      head.detached || !head.name
        ? null
        : head.name.display.replace(/^refs\/heads\//, ""),
    changes: status.totalEntries,
    outgoing: status.ahead ?? null,
  };
}

export const gitQueries = {
  projects: (scope: ServerScope) =>
    queryOptions({
      queryKey: gitKeys.projects(scope),
      queryFn: ({ signal }) =>
        readIPC(signal, () => gitProjectsFor(scope).list()),
    }),
  // Saved outcomes gate every write, so a write never trusts a cached copy:
  // mutations refresh this explicitly before and after they run.
  receipts: (scope: ServerScope) =>
    queryOptions({
      queryKey: gitKeys.receipts(scope),
      queryFn: ({ signal }) =>
        readIPC(signal, () => gitProjectsFor(scope).mutations.receipts()),
      staleTime: 0,
    }),
  /**
   * One bookmark's branch and working-tree summary, for the library row:
   * resolve the path and read status without waiting for worktree discovery.
   * Nothing is held open to close afterwards. The library only runs this when
   * the user asks for that row.
   */
  summary: (scope: ServerScope, project: GitProject) =>
    queryOptions({
      queryKey: gitKeys.summary(scope, project.id),
      queryFn: ({ signal }) =>
        readIPC(signal, () => readSummary(scope, project, signal)),
      staleTime: Infinity,
    }),
  /**
   * The same summary for one worktree, without listing worktrees again: its
   * project's listing already did. Keyed apart from bookmarks by the id the
   * caller gives it (`worktree:<git dir>`).
   */
  checkout: (scope: ServerScope, target: GitProject) =>
    queryOptions({
      queryKey: gitKeys.summary(scope, target.id),
      queryFn: ({ signal }) =>
        readIPC(signal, () => readSummary(scope, target, signal)),
      staleTime: Infinity,
    }),
  status: (scope: ServerScope, repoId: string, filter?: GitStatusFilter) =>
    queryOptions({
      queryKey: gitKeys.status(scope, repoId, filter),
      queryFn: read(scope, (client) =>
        filter
          ? client.status(repoId, undefined, normalizeStatusFilter(filter))
          : client.status(repoId),
      ),
      // Poll only the first page. An unchanged snapshot and matching prefix
      // preserve already loaded pages and their continuation cursor.
      structuralSharing: (previous: unknown, incoming: unknown) => {
        const old = previous as GitStatus | undefined;
        const next = incoming as GitStatus;
        if (
          !old ||
          old.snapshot !== next.snapshot ||
          old.metadata.totalEntries !== next.metadata.totalEntries ||
          JSON.stringify(old.metadata.groupCounts) !==
            JSON.stringify(next.metadata.groupCounts) ||
          old.entries.length <= next.entries.length ||
          !next.nextCursor ||
          JSON.stringify(old.entries.slice(0, next.entries.length)) !==
            JSON.stringify(next.entries)
        )
          return next;
        if (JSON.stringify(old.metadata) === JSON.stringify(next.metadata))
          return old;
        return { ...next, entries: old.entries, nextCursor: old.nextCursor };
      },
    }),
  history: (scope: ServerScope, repoId: string, revision = "HEAD") =>
    queryOptions({
      queryKey: gitKeys.history(scope, repoId, revision),
      queryFn: read(scope, (client) => client.history(repoId, revision)),
    }),
  commit: (scope: ServerScope, repoId: string, commitOid: string) =>
    queryOptions({
      queryKey: gitKeys.commit(scope, repoId, commitOid),
      queryFn: read(scope, (client) => client.commit(repoId, commitOid)),
      ...IMMUTABLE,
    }),
  branches: (
    scope: ServerScope,
    repoId: string,
    cursor?: string,
    options?: GitBranchOptions,
  ) =>
    queryOptions({
      queryKey: gitKeys.branches(scope, repoId, cursor, options),
      queryFn: read(scope, (client) =>
        options
          ? client.branches(repoId, cursor, options)
          : client.branches(repoId, cursor),
      ),
    }),
  remote: (scope: ServerScope, repoId: string, name: string) =>
    queryOptions({
      queryKey: gitKeys.remote(scope, repoId, name),
      queryFn: read(scope, (client) => client.remote(repoId, name)),
    }),
  remotes: (scope: ServerScope, repoId: string) =>
    queryOptions({
      queryKey: gitKeys.remotes(scope, repoId),
      queryFn: read(scope, (client) => client.remotes(repoId)),
    }),
  stashes: (scope: ServerScope, repoId: string, cursor?: string) =>
    queryOptions({
      queryKey: gitKeys.stashes(scope, repoId, cursor),
      queryFn: read(scope, (client) => client.stashes(repoId, cursor)),
    }),
  tag: (scope: ServerScope, repoId: string, oid: string) =>
    queryOptions({
      queryKey: gitKeys.tag(scope, repoId, oid),
      queryFn: read(scope, (client) => client.tag(repoId, oid)),
      ...IMMUTABLE,
    }),
  tags: (scope: ServerScope, repoId: string, cursor?: string) =>
    queryOptions({
      queryKey: gitKeys.tags(scope, repoId, cursor),
      queryFn: read(scope, (client) => client.tags(repoId, cursor)),
    }),
  worktrees: (
    scope: ServerScope,
    repoId: string,
    cursor?: string,
    options?: GitWorktreeOptions,
  ) =>
    queryOptions({
      queryKey: gitKeys.worktrees(scope, repoId, cursor, options),
      queryFn: read(scope, (client) =>
        client.worktrees(repoId, cursor, options),
      ),
    }),
  blobPage: (scope: ServerScope, params: Params<"blobPage">) =>
    queryOptions({
      queryKey: gitKeys.blobPage(scope, params),
      queryFn: read(scope, (client) => client.blobPage(params)),
      ...IMMUTABLE,
    }),
  blob: (scope: ServerScope, repoId: string, oid: string) =>
    queryOptions({
      queryKey: gitKeys.blob(scope, repoId, oid),
      queryFn: read(scope, (client) => client.blob(repoId, oid)),
      ...IMMUTABLE,
    }),
  commitFiles: (scope: ServerScope, params: Params<"commitFiles">) =>
    queryOptions({
      queryKey: gitKeys.commitFiles(scope, params),
      queryFn: read(scope, (client) => client.commitFiles(params)),
      ...IMMUTABLE,
    }),
  diff: (scope: ServerScope, params: Params<"diff">) =>
    queryOptions({
      queryKey: gitKeys.diff(scope, params),
      queryFn: read(scope, (client) => client.diff(params)),
      ...IMMUTABLE,
    }),
  diffPage: (scope: ServerScope, params: Params<"diffPage">) =>
    queryOptions({
      queryKey: gitKeys.diffPage(scope, params),
      queryFn: read(scope, (client) => client.diffPage(params)),
      ...IMMUTABLE,
    }),
  commitDiffPage: (scope: ServerScope, params: Params<"commitDiffPage">) =>
    queryOptions({
      queryKey: gitKeys.commitDiffPage(scope, params),
      queryFn: read(scope, (client) => client.commitDiffPage(params)),
      ...IMMUTABLE,
    }),
  commitDiff: (scope: ServerScope, params: Params<"commitDiff">) =>
    queryOptions({
      queryKey: gitKeys.commitDiff(scope, params),
      queryFn: read(scope, (client) => client.commitDiff(params)),
      ...IMMUTABLE,
    }),
  /**
   * The one read that contacts the remote. It is never refreshed behind the
   * user's back -- not on mount, not when it ages -- because the footer
   * promises that Newport has not contacted the remote unless asked to. The
   * component that shows it decides when it runs.
   */
  remoteNames: (scope: ServerScope, params: Params<"remoteNames">) =>
    queryOptions({
      queryKey: gitKeys.remoteNames(scope, params),
      queryFn: read(scope, (client) => client.remoteNames(params)),
    }),
  remoteRefs: (scope: ServerScope, params: Params<"remoteRefs">) =>
    queryOptions({
      queryKey: gitKeys.remoteRefs(scope, params),
      queryFn: read(scope, (client) => client.remoteRefs(params)),
      staleTime: Infinity,
      refetchOnMount: false,
    }),
};

/*
 * Reads a write can never make stale, and so must never be re-run by one.
 * The content-addressed ones are fixed by their key: re-asking for a diff at a
 * snapshot the write has just replaced would only be refused. And remote refs
 * contact the remote, which Newport does only when the user asks.
 */
const NEVER_INVALIDATED = new Set([
  "tag",
  "commit",
  "blob",
  "diff",
  "commit-diff",
  "commit-files",
  "remote-refs",
]);

/**
 * Retires every read of one repository after a write to it. Active reads
 * refetch; inactive ones are marked stale and refetch when next shown.
 * `except` names kinds the caller has already re-read itself.
 */
export function invalidateRepository(
  client: QueryClient,
  scope: ServerScope,
  repoId: string,
  except: readonly (string | readonly unknown[])[] = [],
) {
  const prefix = gitKeys.repo(scope, repoId);
  return client.invalidateQueries({
    queryKey: prefix,
    predicate: (query) => {
      const kind = String(query.queryKey[prefix.length]);
      return (
        !NEVER_INVALIDATED.has(kind) &&
        !except.some((skip) =>
          typeof skip === "string"
            ? skip === kind
            : JSON.stringify(skip) === JSON.stringify(query.queryKey),
        )
      );
    },
  });
}

/**
 * Forgets every repository read for this connection -- for when cached reads
 * must not be shown again, not because their ids stopped working.
 */
export function forgetRepositories(client: QueryClient, scope: ServerScope) {
  client.removeQueries({ queryKey: gitKeys.repos(scope) });
}
