import { gitPath, type GitPath } from "../domain/git";
import type { GitWorktrees } from "../domain/gitResponses";

export type WorktreeRow = GitWorktrees["entries"][number];

/**
 * What a worktree is called. A linked worktree has the name Git registered it
 * under; the main one has none, so it is named for what it is.
 */
export function worktreeLabel(row: WorktreeRow) {
  if (row.name) return row.name.display;
  return row.kind === "bare" ? "Bare repository" : "Main worktree";
}

/** The branch a worktree has checked out, in the words a list uses. */
export function worktreeBranch(row: WorktreeRow) {
  const head = row.head;
  if (!head) return null;
  if (head.detached)
    return head.oid ? `detached at ${head.oid.hex.slice(0, 7)}` : "detached";
  return head.name?.display.replace(/^refs\/heads\//, "") ?? null;
}

/** The short branch name, only when a branch (not a detached HEAD) is out. */
export function worktreeBranchName(row: WorktreeRow) {
  const head = row.head;
  if (!head || head.detached || !head.name) return null;
  return head.name.display.replace(/^refs\/heads\//, "");
}

/** Whether Newport can open this worktree as a workspace. */
export function openable(row: WorktreeRow) {
  if (row.kind === "bare" || row.state !== "available" || !row.path)
    return false;
  try {
    return gitPath(row.path.display).bytesB64 === row.path.bytesB64;
  } catch {
    return false;
  }
}

/**
 * A worktree name for a branch: Git keeps worktree registrations in one flat
 * directory, so the name cannot contain a slash. `agent/auth-fix` becomes
 * `agent-auth-fix`.
 */
export function worktreeNameFor(branch: string) {
  return branch
    .trim()
    .replace(/[^A-Za-z0-9._-]+/g, "-")
    .replace(/^[-.]+|[-.]+$/g, "")
    .slice(0, 64);
}

/**
 * Where a new worktree goes unless the user says otherwise: beside the main
 * checkout, named after it -- `/srv/app` and `agent-auth-fix` give
 * `/srv/app-agent-auth-fix`. Beside rather than inside, so the checkout is not
 * an untracked directory in the main one, and in a parent that already exists,
 * which the agent requires.
 */
export function defaultWorktreePath(mainRoot: string, name: string) {
  const root = mainRoot.replace(/\/+$/, "") || "/";
  const cut = root.lastIndexOf("/");
  const parent = cut <= 0 ? "" : root.slice(0, cut);
  const base = root.slice(cut + 1) || "worktree";
  return `${parent}/${base}-${name || "worktree"}`;
}

/**
 * The repository a worktree listing describes, by the main worktree's
 * location, for joining rows with branches and bookmarks.
 */
export function mainWorktree(rows: readonly WorktreeRow[]) {
  return rows.find((row) => row.kind === "main" || row.kind === "bare") ?? null;
}

/** Which worktree has a branch checked out, by short branch name. */
export function worktreeByBranch(rows: readonly WorktreeRow[]) {
  const map = new Map<string, WorktreeRow>();
  for (const row of rows) {
    const branch = worktreeBranchName(row);
    if (branch) map.set(branch, row);
  }
  return map;
}

/** A stable key for a worktree across reads: its administrative directory. */
export function worktreeKey(row: { gitDir: GitPath }) {
  return row.gitDir.bytesB64;
}
