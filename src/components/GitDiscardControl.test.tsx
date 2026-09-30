// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { GitDiscardControl } from "./GitDiscardControl";
import { TooltipProvider } from "./ui/tooltip";
import { decodeGitStatus } from "../domain/gitResponses";
import { gitPath } from "../domain/git";

it("invalidates an open discard confirmation when its status snapshot changes", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const status = decodeGitStatus({
    snapshot: "original",
    nextCursor: null,
    entries: [
      {
        entryId: "file",
        path: gitPath("file.txt"),
        oldPath: null,
        flags: 256,
        staged: true,
        unstaged: true,
        untracked: false,
        conflicted: false,
        conflict: null,
      },
    ],
    metadata: {
      head: { name: null, oid: null, unborn: true, detached: false },
      operationState: "Clean",
      integration: null,
      ahead: null,
      behind: null,
      upstreamRef: null,
      basis: "stored_refs",
    },
  });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const onAction = vi.fn().mockResolvedValue(true);
  const render = (snapshot: string) =>
    act(async () =>
      root.render(
        <TooltipProvider>
          <GitDiscardControl
            entry={status.entries[0]}
            status={{ ...status, snapshot }}
            disabled={false}
            busy={false}
            error=""
            onAction={onAction}
          />
        </TooltipProvider>,
      ),
    );
  const button = (name: string) =>
    [...document.querySelectorAll("button")].find(
      (button) => button.textContent === name,
    )!;
  try {
    await render("original");
    await act(async () => button("Discard changes…").click());
    expect(button("Discard changes").disabled).toBe(false);
    // Only one of the two buttons throws work away, and it has to look it.
    // Matched on the variant's own fill: every button carries `destructive`
    // somewhere in its aria-invalid classes, so the bare word proves nothing.
    expect(button("Discard changes").className).toContain("bg-destructive/10");
    expect(button("Cancel").className).not.toContain("bg-destructive/10");
    await render("new snapshot");
    expect(document.body.textContent).toContain("The selection changed");
    expect(button("Discard changes").disabled).toBe(true);
    await act(async () => button("Discard changes").click());
    expect(onAction).not.toHaveBeenCalled();
    expect(button("Cancel").disabled).toBe(false);
    await act(async () => button("Cancel").click());
    await act(async () => button("Discard changes…").click());
    await act(async () => button("Discard changes").click());
    expect(onAction).toHaveBeenCalledExactlyOnceWith({
      kind: "discard",
      entryIds: ["file"],
      source: "index",
    });
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});
