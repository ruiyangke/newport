import { QueryClient } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { gitQueries } from "./git";
import { gitPath } from "../domain/git";
import { decodeGitStatus } from "../domain/gitResponses";

const projects = vi.hoisted(() => ({
  open: vi.fn(),
  repositories: { status: vi.fn() },
}));
vi.mock("../git/registry", () => ({ gitProjectsFor: () => projects }));
beforeEach(() => {
  vi.clearAllMocks();
  projects.open.mockResolvedValue({ repoId: "repo" });
});
it.each([
  { total: 320, next: "next", truncated: false, expected: 320 },
  { total: 32000, next: "next", truncated: true, expected: 32000 },
  { total: undefined, next: "next", truncated: false, expected: null },
  { total: undefined, next: null, truncated: true, expected: null },
  { total: undefined, next: null, truncated: false, expected: 1 },
])(
  "does not present a page length as the total: %j",
  async ({ total, next, truncated, expected }) => {
    projects.repositories.status.mockResolvedValue(
      decodeGitStatus({
        snapshot: "snapshot",
        nextCursor: next,
        entries: [
          {
            entryId: "entry",
            path: gitPath("file"),
            oldPath: null,
            flags: 256,
            staged: false,
            unstaged: true,
            untracked: false,
            conflicted: false,
            conflict: null,
          },
        ],
        metadata: {
          head: {
            name: gitPath("refs/heads/main"),
            oid: null,
            unborn: true,
            detached: false,
          },
          operationState: "Clean",
          integration: null,
          ahead: null,
          behind: null,
          upstreamRef: null,
          basis: "stored_refs",
          totalEntries: total,
          truncated,
        },
      }),
    );
    const queryClient = new QueryClient();
    try {
      const result = await queryClient.fetchQuery(
        gitQueries.checkout(
          { id: "server", connection: "server:0:", destination: "server" },
          {
            id: "project",
            serverId: "server",
            name: "Application",
            path: gitPath("/repo"),
          },
        ),
      );
      expect(result.changes).toBe(expected);
      expect(projects.repositories.status).toHaveBeenCalledTimes(1);
    } finally {
      queryClient.clear();
    }
  },
);
