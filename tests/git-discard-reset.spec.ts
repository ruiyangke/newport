import { test, expect } from "@playwright/test";
import { mkdirSync } from "node:fs";

test("confirms discard, reset, amend and detached checkout with exact guards", async ({
  page,
}, info) => {
  await page.addInitScript(() => {
    const path = (display: string) => ({ display, bytesB64: btoa(display) });
    const oid = (char: string) => ({ algorithm: "sha1", hex: char.repeat(40) });
    const state = {
      revision: 0,
      conflict: false,
      amended: false,
      commitMessage: "Earlier commit",
      staged: true,
      unstaged: true,
      receipts: [] as any[],
      actions: [] as any[],
      head: {
        name: path("refs/heads/main") as ReturnType<typeof path> | null,
        oid: oid("a"),
        unborn: false,
        detached: false,
      },
    };
    Object.assign(window, {
      resetFixture: state,
      isTauri: true,
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: "main" } },
        transformCallback: () => 1,
        invoke: async (cmd: string, args: any) => {
          if (cmd === "snapshot")
            return {
              config: {
                servers: [
                  {
                    id: "server",
                    name: "Development",
                    sshUser: "dev",
                    sshHost: "example.test",
                    sshPort: 22,
                    authMethod: "publicKey",
                  },
                ],
                tunnels: [],
              },
              runtime: { tunnels: {}, clipboard: {}, health: {} },
              loadError: null,
            };
          if (cmd === "get_sidebar_width") return 200;
          if (cmd === "git_projects_list")
            return [
              {
                id: "project",
                serverId: "server",
                name: "Application",
                path: path("/repo"),
              },
            ];
          if (cmd === "git_pending_operations") return state.receipts;
          if (cmd === "git_acknowledge_operation") {
            state.receipts = state.receipts.filter(
              (r) => r.operationId !== args.operationId,
            );
            return;
          }
          if (cmd === "git_connect")
            return {
              sessionId: "session",
              serverId: "server",
              info: {
                capabilities: {
                  methods: [
                    "repo.open",
                    "repo.status",
                    "repo.diff",
                    "repo.history",
                    "repo.commit_files",
                    "operation.start",
                  ],
                  actions: ["discard", "reset", "commit.amend", "checkout"],
                },
              },
            };
          if (cmd !== "git_request") return;
          const { method, params } = args.request;
          if (method === "repo.open")
            return {
              repoId: "repo",
              commonRepoId: "common",
              root: path("/repo"),
              bare: false,
              objectFormat: "sha1",
              head: state.head,
              operationState: "Clean",
              integration: null,
              capabilities: { readOnly: false, workingTree: true },
            };
          if (method === "repo.status")
            return {
              snapshot: `s${state.revision}`,
              entries:
                state.staged || state.unstaged
                  ? [
                      {
                        entryId: `file${state.revision}`,
                        path: path("src/app.ts"),
                        oldPath: null,
                        flags: 256,
                        staged: state.staged,
                        unstaged: state.unstaged,
                        untracked: false,
                        conflicted: state.conflict,
                        conflict: null,
                      },
                    ]
                  : [],
              nextCursor: null,
              metadata: {
                head: state.head,
                operationState: "Clean",
                integration: null,
                ahead: null,
                behind: null,
                upstreamRef: null,
                basis: "stored_refs",
              },
            };
          if (method === "repo.diff")
            return {
              snapshot: params.snapshot,
              diff: { truncated: false, readOnly: true, files: [] },
            };
          if (method === "repo.history")
            return {
              snapshot: `history${state.revision}`,
              entries: [
                {
                  oid: oid(state.amended ? "c" : "b"),
                  parents: [],
                  message: path(state.commitMessage),
                  messageTruncated: false,
                  author: { name: "Developer", email: "dev@example.test" },
                  time: 1789983160,
                  offsetMinutes: 0,
                },
              ],
              nextCursor: null,
              metadata: { resolvedRevision: state.head.oid, truncated: false },
            };
          if (method === "repo.commit_files")
            return {
              snapshot: "files",
              entries: [],
              nextCursor: null,
              metadata: {
                commitOid: oid(state.amended ? "c" : "b"),
                parents: [],
                parentIndex: null,
                parentOid: null,
                totalFiles: 0,
                truncated: false,
              },
            };
          if (method === "operation.start") {
            if (params.expectedSnapshot !== `s${state.revision}`)
              throw new Error("Wrong status snapshot");
            const action = params.action;
            if (action.kind === "discard") {
              if (
                JSON.stringify(action.entryIds) !==
                JSON.stringify([`file${state.revision}`])
              )
                throw new Error("Wrong discard selection");
              state.unstaged = false;
              if (action.source === "head") state.staged = false;
            } else if (action.kind === "reset") {
              if (
                action.expectedOid !== state.head.oid.hex ||
                action.targetOid !== oid("b").hex
              )
                throw new Error("Wrong reset guard");
              state.head = { ...state.head, oid: oid("b") };
            } else if (action.kind === "commit.amend") {
              if (action.expectedOid !== state.head.oid.hex)
                throw new Error("Wrong amendment guard");
              state.head = { ...state.head, oid: oid("c") };
              state.amended = true;
              state.commitMessage = action.message;
              state.staged = false;
            } else if (action.kind === "checkout") {
              if (
                action.target.kind !== "detached" ||
                action.target.oid !== oid("c").hex
              )
                throw new Error("Wrong checkout target");
              state.head = {
                ...state.head,
                name: null,
                detached: true,
                oid: oid("c"),
              };
            } else throw new Error("Unexpected action");
            state.actions.push(action);
            state.revision++;
            state.receipts.push({
              operationId: params.operationId,
              serverId: "server",
              action: action.kind,
              state: "succeeded",
            });
            return {
              operationId: params.operationId,
              repository: "common",
              payloadHash: "hash",
              seq: 1,
              state: "succeeded",
              result: {},
              error: null,
            };
          }
          throw new Error(`Unexpected method ${method}`);
        },
      },
    });
  });
  await page.goto("/");
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await page
    .getByRole("button", { name: "src/app.ts", exact: true })
    .first()
    .click();
  await page
    .getByRole("button", { name: "Discard changes…", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("combobox", { name: "Restore from", exact: true }),
  ).toContainText("Staged version");
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-discard-${info.project.name}.png`,
  });
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(
    await page.evaluate(() => (window as any).resetFixture.actions.length),
  ).toBe(0);
  await page
    .getByRole("button", { name: "Discard changes…", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "Discard changes", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  await page
    .getByRole("button", { name: "src/app.ts", exact: true })
    .first()
    .click();
  await page
    .getByRole("button", { name: "Discard changes…", exact: true })
    .click();
  await expect(
    dialog.getByRole("combobox", { name: "Restore from", exact: true }),
  ).toContainText("Last commit");
  await dialog
    .getByRole("button", { name: "Discard changes", exact: true })
    .click();
  await expect(
    page.getByText("No local changes", { exact: true }),
  ).toBeVisible();
  await page.getByRole("tab", { name: "History", exact: true }).click();
  for (const mode of ["Soft", "Mixed", "Hard"]) {
    await page.getByRole("button", { name: /Earlier commit/ }).click();
    await page
      .getByRole("button", { name: "Commit actions", exact: true })
      .click();
    await page
      .getByRole("menuitem", { name: "Reset to commit…", exact: true })
      .click();
    await dialog
      .getByRole("combobox", { name: "Reset mode", exact: true })
      .click();
    await page.getByRole("option", { name: new RegExp(`^${mode}`) }).click();
    await page.screenshot({
      animations: "disabled",
      path: `.impeccable/screenshots/projects-reset-${mode.toLowerCase()}-${info.project.name}.png`,
    });
    await dialog
      .getByRole("button", {
        name: mode === "Hard" ? "Discard changes and reset" : "Reset to commit",
        exact: true,
      })
      .click();
    await expect(dialog).toHaveCount(0);
  }
  expect(
    await page.evaluate(() =>
      (window as any).resetFixture.actions.map((action: any) =>
        action.kind === "discard" ? action.source : action.mode,
      ),
    ),
  ).toEqual(["index", "head", "soft", "mixed", "hard"]);
  await page.getByRole("button", { name: /Earlier commit/ }).click();
  await page
    .getByRole("button", { name: "Commit actions", exact: true })
    .click();
  await page
    .getByRole("menuitem", { name: "Amend latest commit…", exact: true })
    .click();
  await expect(
    dialog.getByLabel("Replacement commit message", { exact: true }),
  ).toHaveValue("Earlier commit");
  await dialog
    .getByLabel("Replacement commit message", { exact: true })
    .fill("Updated commit message");
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-amend-${info.project.name}.png`,
  });
  await dialog
    .getByRole("button", { name: "Replace latest commit", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: /Updated commit message/ }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => (window as any).resetFixture.actions.at(-1)),
  ).toEqual({
    kind: "commit.amend",
    expectedOid: "b".repeat(40),
    message: "Updated commit message",
  });
  await page.getByRole("button", { name: /Updated commit message/ }).click();
  await page
    .getByRole("button", { name: "Commit actions", exact: true })
    .click();
  await page
    .getByRole("menuitem", { name: "Check out commit…", exact: true })
    .click();
  await expect(dialog).toContainText("detached HEAD");
  await expect(dialog).toContainText("create a branch before");
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-checkout-${info.project.name}.png`,
  });
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(
    await page.evaluate(() => (window as any).resetFixture.actions.at(-1).kind),
  ).toBe("commit.amend");
  await page
    .getByRole("button", { name: "Commit actions", exact: true })
    .click();
  await page
    .getByRole("menuitem", { name: "Check out commit…", exact: true })
    .click();
  await dialog
    .getByRole("button", { name: "Check out commit", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Branches: Detached HEAD", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => (window as any).resetFixture.actions.at(-1)),
  ).toEqual({
    kind: "checkout",
    target: { kind: "detached", oid: "c".repeat(40) },
  });
  await page.evaluate(() => {
    const state = (window as any).resetFixture;
    state.conflict = true;
    state.staged = true;
    state.revision++;
  });
  await page.getByRole("tab", { name: "Changes", exact: true }).click();
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "src/app.ts", exact: true }),
  ).toBeVisible();
  await page.getByRole("tab", { name: "History", exact: true }).click();
  await page.getByRole("button", { name: /Updated commit message/ }).click();
  await page
    .getByRole("button", { name: "Commit actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", {
      name: "Reset unavailable: Resolve conflicts before resetting.",
      exact: true,
    }),
  ).toHaveAttribute("aria-disabled", "true");
  await expect(
    page.getByRole("menuitem", {
      name: "Amend unavailable: resolve conflicts first",
      exact: true,
    }),
  ).toHaveAttribute("aria-disabled", "true");
  await expect(
    page.getByRole("menuitem", {
      name: "Checkout unavailable: resolve conflicts first",
      exact: true,
    }),
  ).toHaveAttribute("aria-disabled", "true");
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-reset-conflicts-${info.project.name}.png`,
  });
});
