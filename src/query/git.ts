import { queryOptions, type QueryClient } from "@tanstack/react-query";
import type { GitProject } from "../domain/git";
import type { GitRepositoryClient } from "../api/gitRepository";
import { gitProjectsFor } from "../git/registry";
import { keys, type ServerScope } from "./keys";
import { readIPC } from "./client";
import { appendGitPage } from "../domain/gitResponses";

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
  status: (scope: ServerScope, repoId: string) =>
    [...gitKeys.repo(scope, repoId), "status"] as const,
  history: (scope: ServerScope, repoId: string, revision = "HEAD") =>
    [...gitKeys.repo(scope, repoId), "history", revision] as const,
  branches: (scope: ServerScope, repoId: string, cursor?: string) =>
    [...gitKeys.repo(scope, repoId), "branches", cursor ?? null] as const,
  remotes: (scope: ServerScope, repoId: string) =>
    [...gitKeys.repo(scope, repoId), "remotes"] as const,
  stashes: (scope: ServerScope, repoId: string, cursor?: string) =>
    [...gitKeys.repo(scope, repoId), "stashes", cursor ?? null] as const,
  tags: (scope: ServerScope, repoId: string, cursor?: string) =>
    [...gitKeys.repo(scope, repoId), "tags", cursor ?? null] as const,
  worktrees: (scope: ServerScope, repoId: string, cursor?: string) =>
    [...gitKeys.repo(scope, repoId), "worktrees", cursor ?? null] as const,
  worktreeList: (scope: ServerScope, repoId: string) =>
    [...gitKeys.repo(scope, repoId), "worktree-list"] as const,
  blob: (scope: ServerScope, repoId: string, oid: string) =>
    [...gitKeys.repo(scope, repoId), "blob", oid.toLowerCase()] as const,
  commitFiles: (scope: ServerScope, params: Params<"commitFiles">) =>
    [...gitKeys.repo(scope, params.repoId), "commit-files", params] as const,
  diff: (scope: ServerScope, params: Params<"diff">) =>
    [...gitKeys.repo(scope, params.repoId), "diff", params] as const,
  commitDiff: (scope: ServerScope, params: Params<"commitDiff">) =>
    [...gitKeys.repo(scope, params.repoId), "commit-diff", params] as const,
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
    readIPC(signal, () => run(repositories(scope)));
}

// Content-addressed reads cannot go stale: a blob or a commit diff is fixed by
// its object id, and a working diff is fixed by the snapshot it was read at.
const IMMUTABLE = { staleTime: Infinity } as const;

/**
 * Every worktree of a repository, every page of the listing joined into one.
 * Agents make many; a first page of a hundred would pass for the whole list.
 * Pages of one listing share its snapshot, which guards writes for them all.
 */
async function readWorktreeList(client: GitRepositoryClient, repoId: string) {
  let listing = await client.worktrees(repoId);
  // The agent caps a listing at a thousand rows; this cap only stops a
  // listing that never ends.
  for (let pages = 1; listing.nextCursor && pages < 20; pages++) {
    const cursor = listing.nextCursor;
    listing = appendGitPage(
      listing,
      await client.worktrees(repoId, cursor),
      cursor,
    );
  }
  return listing;
}

/** A checkout's branch, changed-file count and commits ahead, read now. */
async function readSummary(
  scope: ServerScope,
  project: GitProject,
  withWorktrees: boolean,
) {
  const projects = gitProjectsFor(scope);
  const repository = await projects.open(project);
  const status = await projects.repositories.status(repository.repoId);
  const head = status.metadata.head;
  return {
    // An unborn HEAD still names the branch it is on; only a detached HEAD is
    // genuinely on none.
    branch:
      head.detached || !head.name
        ? null
        : head.name.display.replace(/^refs\/heads\//, ""),
    // Prefer the agent's total. A page length is a total only when the listing
    // is complete; older agents may provide neither a total nor every row.
    changes:
      status.metadata.totalEntries ??
      (status.metadata.truncated || status.nextCursor
        ? null
        : status.entries.length),
    outgoing: status.metadata.ahead ?? null,
    // The repository's other checkouts, so the library can list them under
    // the project. An agent too old to list them lists none, rather than
    // failing the row.
    worktrees: withWorktrees
      ? await readWorktreeList(projects.repositories, repository.repoId)
          .then((page) =>
            page.entries.filter((row) => row.kind === "linked" && !row.current),
          )
          .catch(() => undefined)
      : undefined,
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
   * resolve the path, read status, and list the repository's other worktrees.
   * Nothing is held open to close afterwards. The library only runs this when
   * the user asks for that row.
   */
  summary: (scope: ServerScope, project: GitProject) =>
    queryOptions({
      queryKey: gitKeys.summary(scope, project.id),
      queryFn: ({ signal }) =>
        readIPC(signal, () => readSummary(scope, project, true)),
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
        readIPC(signal, () => readSummary(scope, target, false)),
      staleTime: Infinity,
    }),
  status: (scope: ServerScope, repoId: string) =>
    queryOptions({
      queryKey: gitKeys.status(scope, repoId),
      queryFn: read(scope, (client) => client.status(repoId)),
    }),
  history: (scope: ServerScope, repoId: string, revision = "HEAD") =>
    queryOptions({
      queryKey: gitKeys.history(scope, repoId, revision),
      queryFn: read(scope, (client) => client.history(repoId, revision)),
    }),
  branches: (scope: ServerScope, repoId: string, cursor?: string) =>
    queryOptions({
      queryKey: gitKeys.branches(scope, repoId, cursor),
      queryFn: read(scope, (client) => client.branches(repoId, cursor)),
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
  tags: (scope: ServerScope, repoId: string, cursor?: string) =>
    queryOptions({
      queryKey: gitKeys.tags(scope, repoId, cursor),
      queryFn: read(scope, (client) => client.tags(repoId, cursor)),
    }),
  worktrees: (scope: ServerScope, repoId: string, cursor?: string) =>
    queryOptions({
      queryKey: gitKeys.worktrees(scope, repoId, cursor),
      queryFn: read(scope, (client) => client.worktrees(repoId, cursor)),
    }),
  /** Every worktree, all pages: the picker, the branch hand-off, new ones. */
  worktreeList: (scope: ServerScope, repoId: string) =>
    queryOptions({
      queryKey: gitKeys.worktreeList(scope, repoId),
      queryFn: read(scope, (client) => readWorktreeList(client, repoId)),
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
  except: readonly string[] = [],
) {
  const prefix = gitKeys.repo(scope, repoId);
  return client.invalidateQueries({
    queryKey: prefix,
    predicate: (query) => {
      const kind = String(query.queryKey[prefix.length]);
      return !NEVER_INVALIDATED.has(kind) && !except.includes(kind);
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
