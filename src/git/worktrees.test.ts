import { expect, it } from "vitest";
import { gitPath } from "../domain/git";
import {
  defaultWorktreePath,
  openable,
  worktreeBranch,
  worktreeByBranch,
  worktreeLabel,
  worktreeNameFor,
  type WorktreeRow,
} from "./worktrees";

const oid = { algorithm: "sha1" as const, hex: "b".repeat(40) };
function row(overrides: Partial<WorktreeRow> = {}): WorktreeRow {
  return {
    name: gitPath("agent-fix"),
    kind: "linked",
    state: "available",
    path: gitPath("/srv/app-agent-fix"),
    gitDir: gitPath("/srv/app/.git/worktrees/agent-fix"),
    current: false,
    head: {
      name: gitPath("refs/heads/agent/fix"),
      oid,
      unborn: false,
      detached: false,
    },
    locked: false,
    lockReason: null,
    lockReasonUnavailable: false,
    prunable: false,
    ...overrides,
  } as WorktreeRow;
}

it("names a worktree for its branch without the slashes Git forbids there", () => {
  expect(worktreeNameFor("agent/fix-login")).toBe("agent-fix-login");
  expect(worktreeNameFor("  feat/ünicode name ")).toBe("feat-nicode-name");
  expect(worktreeNameFor("/leading/and/trailing/")).toBe(
    "leading-and-trailing",
  );
  expect(worktreeNameFor("x".repeat(80))).toHaveLength(64);
});

it("places a new worktree beside the main checkout", () => {
  expect(defaultWorktreePath("/srv/app", "agent-fix")).toBe(
    "/srv/app-agent-fix",
  );
  expect(defaultWorktreePath("/srv/app/", "agent-fix")).toBe(
    "/srv/app-agent-fix",
  );
  // A checkout at the root of the filesystem still gets an absolute sibling.
  expect(defaultWorktreePath("/app", "x")).toBe("/app-x");
});

it("describes what a worktree has checked out, and names the main one", () => {
  expect(worktreeLabel(row())).toBe("agent-fix");
  expect(worktreeLabel(row({ name: null, kind: "main" }))).toBe(
    "Main worktree",
  );
  expect(worktreeBranch(row())).toBe("agent/fix");
  expect(
    worktreeBranch(
      row({ head: { name: null, oid, unborn: false, detached: true } }),
    ),
  ).toBe("detached at bbbbbbb");
});

it("knows which worktree holds a branch, and which can be opened", () => {
  const rows = [
    row(),
    row({
      name: gitPath("gone"),
      state: "missing",
      head: {
        name: gitPath("refs/heads/spike"),
        oid,
        unborn: false,
        detached: false,
      },
    }),
  ];
  const holders = worktreeByBranch(rows);
  expect(holders.get("agent/fix")?.name?.display).toBe("agent-fix");
  expect(holders.get("spike")?.name?.display).toBe("gone");
  expect(openable(rows[0])).toBe(true);
  // A missing checkout has nothing to open, and a bare repository no files.
  expect(openable(rows[1])).toBe(false);
  expect(openable(row({ kind: "bare" }))).toBe(false);
});
