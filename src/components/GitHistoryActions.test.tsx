// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { GitHistoryActions } from "./GitHistoryActions";
import { TooltipProvider } from "./ui/tooltip";
import { decodeGitRepository, decodeGitHistory } from "../domain/gitResponses";
import { gitPath } from "../domain/git";

for (const operation of ["reset", "amend", "checkout"])
  it.each(
    operation === "amend"
      ? ["snapshot", "head", "selection", "conflicts", "truncated", "encoding"]
      : ["snapshot", "head", "selection", "conflicts"],
  )(`%s protection for ${operation}`, async (change) => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
    const oid = (character: string) => ({
      algorithm: "sha1",
      hex: character.repeat(40),
    });
    const repository = decodeGitRepository({
      repoId: "repo",
      commonRepoId: "common",
      root: gitPath("/repo"),
      bare: false,
      objectFormat: "sha1",
      head: {
        name: gitPath("refs/heads/main"),
        oid: oid("a"),
        detached: false,
        unborn: false,
      },
      operationState: "Clean",
      integration: null,
      capabilities: { readOnly: false, workingTree: true },
    });
    const commit = decodeGitHistory({
      snapshot: "history",
      entries: [
        {
          oid: oid(operation === "amend" ? "a" : "b"),
          parents: [],
          message:
            change === "encoding"
              ? { display: "�", bytesB64: "/w==" }
              : gitPath("Target"),
          messageTruncated: change === "truncated",
          author: { name: "Dev", email: "dev@example.test" },
          time: 0,
          offsetMinutes: 0,
        },
      ],
      nextCursor: null,
      metadata: { resolvedRevision: oid("a"), truncated: false },
    }).entries[0];
    const host = document.createElement("div");
    document.body.append(host);
    const root = createRoot(host);
    const onAction = vi.fn().mockResolvedValue(true);
    const render = (changed: boolean) =>
      act(async () =>
        root.render(
          <TooltipProvider>
            <GitHistoryActions
              commit={
                changed && change === "selection"
                  ? {
                      ...commit,
                      oid: { algorithm: "sha1", hex: "d".repeat(40) },
                    }
                  : commit
              }
              repository={
                changed && change === "head"
                  ? {
                      ...repository,
                      head: {
                        ...repository.head,
                        oid: { algorithm: "sha1", hex: "c".repeat(40) },
                      },
                    }
                  : repository
              }
              snapshot={changed && change === "snapshot" ? "new" : "original"}
              conflicted={changed && change === "conflicts"}
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
      await render(false);
      await act(async () =>
        button("Commit actions").dispatchEvent(
          new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
        ),
      );
      await act(async () =>
        [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')]
          .find(
            (item) =>
              item.textContent ===
              (operation === "checkout"
                ? "Check out commit…"
                : operation === "amend"
                  ? "Amend latest commit…"
                  : "Reset to commit…"),
          )!
          .click(),
      );
      if (change === "truncated" || change === "encoding") {
        expect(document.querySelector("textarea")?.value).toBe("");
        expect(document.body.textContent).toContain(
          "Enter a complete replacement message",
        );
        expect(button("Replace latest commit").disabled).toBe(true);
        expect(onAction).not.toHaveBeenCalled();
        return;
      }
      expect(
        button(
          operation === "checkout"
            ? "Check out commit"
            : operation === "amend"
              ? "Replace latest commit"
              : "Reset to commit",
        ).disabled,
      ).toBe(false);
      await render(true);
      expect(document.body.textContent).toContain(
        change === "conflicts"
          ? operation === "checkout"
            ? "Resolve conflicts before checking out."
            : operation === "amend"
              ? "Resolve conflicts before amending."
              : "Resolve conflicts before resetting."
          : operation === "checkout"
            ? "The repository or selected commit changed"
            : "The repository changed",
      );
      expect(
        button(
          operation === "checkout"
            ? "Check out commit"
            : operation === "amend"
              ? "Replace latest commit"
              : "Reset to commit",
        ).disabled,
      ).toBe(true);
      await act(async () =>
        button(
          operation === "checkout"
            ? "Check out commit"
            : operation === "amend"
              ? "Replace latest commit"
              : "Reset to commit",
        ).click(),
      );
      expect(onAction).not.toHaveBeenCalled();
      expect(button("Cancel").disabled).toBe(false);
    } finally {
      await act(async () => root.unmount());
      host.remove();
    }
  });

it("sends a fast-forward-only merge when that strategy is chosen", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const oid = (character: string) => ({
    algorithm: "sha1",
    hex: character.repeat(40),
  });
  const repository = decodeGitRepository({
    repoId: "repo",
    commonRepoId: "common",
    root: gitPath("/repo"),
    bare: false,
    objectFormat: "sha1",
    head: {
      name: gitPath("refs/heads/main"),
      oid: oid("a"),
      detached: false,
      unborn: false,
    },
    operationState: "Clean",
    integration: null,
    capabilities: { readOnly: false, workingTree: true },
  });
  const commit = decodeGitHistory({
    snapshot: "history",
    entries: [
      {
        oid: oid("b"),
        parents: [],
        message: gitPath("Target"),
        messageTruncated: false,
        author: { name: "Dev", email: "dev@example.test" },
        time: 0,
        offsetMinutes: 0,
      },
    ],
    nextCursor: null,
    metadata: { resolvedRevision: oid("a"), truncated: false },
  }).entries[0];
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const onAction = vi.fn().mockResolvedValue(true);
  const button = (name: string) =>
    [...document.querySelectorAll("button")].find(
      (item) => item.textContent === name,
    )!;
  try {
    await act(async () =>
      root.render(
        <TooltipProvider>
          <GitHistoryActions
            commit={commit}
            repository={repository}
            snapshot="original"
            conflicted={false}
            disabled={false}
            busy={false}
            error=""
            onAction={onAction}
          />
        </TooltipProvider>,
      ),
    );
    await act(async () =>
      button("Commit actions").dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      ),
    );
    await act(async () =>
      [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')]
        .find((item) => item.textContent === "Merge into current branch…")!
        .click(),
    );
    expect(button("Merge")).toBeDefined();
    const checkbox = [
      ...document.querySelectorAll<HTMLInputElement>("input[type=checkbox]"),
    ][0];
    await act(async () => checkbox.click());
    expect(document.body.textContent).toContain("no merge commit is created");
    await act(async () => button("Fast-forward").click());
    expect(onAction).toHaveBeenCalledWith({
      kind: "merge.fast_forward",
      targetOid: "b".repeat(40),
    });
  } finally {
    await act(async () => root.unmount());
    host.remove();
  }
});
