import { readFileSync } from "node:fs";
import { expect, it, vi } from "vitest";
import type { GitRequest, GitWriteAction } from "../domain/git";
import { GitRepositoryClient } from "./gitRepository";

// Generated only by the disposable SSH fixture. Ordinary unit runs need no SSH.
const tracePath = process.env.NEWPORT_GIT_TRACE_PATH;
it.skipIf(!tracePath)(
  "decodes the real SSH workflow through every typed Git client method",
  async () => {
    const rows = readFileSync(tracePath!, "utf8")
      .trim()
      .split("\n")
      .map(
        (line) =>
          JSON.parse(line) as {
            request: GitRequest;
            response?: unknown;
            error?: unknown;
          },
      );
    const covered = new Set<string>();
    const successfulActions = new Set<string>();
    const resolvingActions = new Set<string>();
    for (const row of rows) {
      if (!("response" in row)) continue;
      const session = {
        request: vi.fn().mockResolvedValue(row.response),
        forget: vi.fn().mockResolvedValue(undefined),
      };
      const client = new GitRepositoryClient(session);
      const r = row.request;
      try {
        switch (r.method) {
          case "repo.open":
            await client.open(r.params.path);
            break;
          case "repo.close":
            await client.close(r.params.repoId);
            break;
          case "repo.status":
            await client.status(r.params.repoId, r.params.cursor ?? undefined);
            break;
          case "repo.history":
            await client.history(
              r.params.repoId,
              r.params.revision,
              r.params.cursor ?? undefined,
            );
            break;
          case "repo.branches":
            await client.branches(
              r.params.repoId,
              r.params.cursor ?? undefined,
            );
            break;
          case "repo.remotes":
            await client.remotes(r.params.repoId);
            break;
          case "repo.remote_refs":
            await client.remoteRefs(r.params);
            break;
          case "repo.stashes":
            await client.stashes(r.params.repoId, r.params.cursor ?? undefined);
            break;
          case "repo.tags":
            await client.tags(r.params.repoId, r.params.cursor ?? undefined);
            break;
          case "repo.worktrees":
            await client.worktrees(
              r.params.repoId,
              r.params.cursor ?? undefined,
            );
            break;
          case "repo.blob":
            await client.blob(r.params.repoId, r.params.oid);
            break;
          case "repo.diff":
            await client.diff(r.params);
            break;
          case "repo.commit_diff":
            await client.commitDiff(r.params);
            break;
          case "repo.commit_files":
            await client.commitFiles(r.params);
            break;
          case "operation.start": {
            const result = await client.start(r.params);
            if (result.state === "succeeded")
              successfulActions.add(r.params.action.kind);
            if (result.state === "needs_resolution")
              resolvingActions.add(r.params.action.kind);
            break;
          }
          case "operation.get":
            await client.operation(r.params.operationId);
            break;
          case "repo.init":
          case "repo.clone":
            await client.bootstrap(r);
            break;
          default: {
            const unreachable: never = r;
            throw new Error(
              `Unhandled request: ${JSON.stringify(unreachable)}`,
            );
          }
        }
        expect(
          session.forget,
          `${r.method} must decode without terminating the session`,
        ).not.toHaveBeenCalled();
        expect(session.request).toHaveBeenCalledOnce();
        covered.add(r.method);
      } catch (cause) {
        throw new Error(`Real SSH response failed for ${r.method}`, { cause });
      }
    }
    // Exhaustive at compile time: a new action must acquire live coverage here.
    const expectedActions: Record<GitWriteAction["kind"], true> = {
      stage: true,
      unstage: true,
      discard: true,
      "conflict.resolve": true,
      reset: true,
      commit: true,
      "commit.amend": true,
      "branch.create": true,
      "branch.rename": true,
      "branch.delete": true,
      "branch.set_upstream": true,
      "branch.delete_remote": true,
      checkout: true,
      "remote.add": true,
      "remote.rename": true,
      "remote.set_url": true,
      "remote.remove": true,
      fetch: true,
      push: true,
      "push.with_lease": true,
      "pull.fast_forward": true,
      merge: true,
      "merge.fast_forward": true,
      "merge.abort": true,
      "integration.continue": true,
      "integration.abort": true,
      "integration.skip": true,
      cherry_pick: true,
      revert: true,
      rebase: true,
      "stash.save": true,
      "stash.apply": true,
      "stash.pop": true,
      "stash.drop": true,
      "tag.create": true,
      "tag.delete": true,
      "tag.push": true,
      "tag.delete_remote": true,
      "worktree.add": true,
      "worktree.remove": true,
      "worktree.prune": true,
      "worktree.repair": true,
      "worktree.lock": true,
      "worktree.unlock": true,
    };
    expect(
      [...new Set([...successfulActions, ...resolvingActions])].sort(),
    ).toEqual(Object.keys(expectedActions).sort());
    console.info(
      `SSH write coverage: ${successfulActions.size} succeeded; conflict workflows: ${[...resolvingActions].sort().join(", ")}`,
    );
    expect([...covered].sort()).toEqual(
      [
        "repo.open",
        "repo.close",
        "repo.status",
        "repo.history",
        "repo.branches",
        "repo.remotes",
        "repo.remote_refs",
        "repo.stashes",
        "repo.tags",
        "repo.worktrees",
        "repo.blob",
        "repo.diff",
        "repo.commit_diff",
        "repo.commit_files",
        "operation.start",
        "operation.get",
        "repo.init",
        "repo.clone",
      ].sort(),
    );
  },
);
