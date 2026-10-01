import { test, expect, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

/** The four repository dialogs open from the grouped Git actions menu. */
async function openGitAction(page: Page, item: string) {
  if (item === "Worktrees") {
    await page.getByRole("button", { name: /^Worktrees:/ }).click();
    await page.getByRole("button", { name: "Manage…", exact: true }).click();
    return;
  }
  await page.getByRole("button", { name: "Git actions", exact: true }).click();
  await page.getByRole("menuitem", { name: item, exact: true }).click();
}

for (const bare of [false, true])
  test(`manages ${bare ? "bare" : "working"} worktrees with listing snapshots and exact branch guards`, async ({
    page,
  }, info) => {
    await page.addInitScript((bare: boolean) => {
      const path = (display: string) => ({ display, bytesB64: btoa(display) });
      const oid = { algorithm: "sha1", hex: "a".repeat(40) };
      const head = {
        name: path("refs/heads/main"),
        oid,
        unborn: false,
        detached: false,
      };
      const row = (name: string, state: string, current = false) => ({
        name: current ? null : path(name),
        kind: current ? (bare ? "bare" : "main") : "linked",
        state,
        path: path(current ? "/repo" : `/srv/${name}`),
        gitDir: path(current ? "/repo/.git" : `/repo/.git/worktrees/${name}`),
        current,
        head: current ? head : { ...head, name: path(`refs/heads/${name}`) },
        locked: false,
        lockReason: null,
        lockReasonUnavailable: false,
        prunable: state === "missing",
      });
      const state = {
        revision: 0,
        emptyBranches: false,
        rows: [
          row("main", "available", true),
          row("moved", "missing"),
          row("orphan", "missing"),
        ],
        receipts: [] as any[],
        actions: [] as any[],
      };
      Object.assign(window, {
        worktreeFixture: state,
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
                    features: ["worktrees.filter", "worktrees.snapshot_filter"],
                    methods: [
                      "repo.open",
                      "repo.status",
                      "repo.history",
                      "repo.close",
                      "repo.worktrees",
                      "repo.branches",
                      "operation.start",
                    ],
                    actions: [
                      "worktree.add",
                      "worktree.lock",
                      "worktree.unlock",
                      "worktree.remove",
                      "worktree.prune",
                      "worktree.repair",
                    ],
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
                bare,
                objectFormat: "sha1",
                head,
                operationState: "Clean",
                integration: null,
                capabilities: { readOnly: false, workingTree: !bare },
              };
            if (method === "repo.history")
              return {
                snapshot: "history",
                entries: [],
                nextCursor: null,
                metadata: { resolvedRevision: oid, truncated: false },
              };
            if (method === "repo.status" && bare)
              throw new Error("Bare worktrees must not read status");
            if (method === "repo.status")
              return {
                snapshot: `status${state.revision}`,
                entries: [],
                nextCursor: null,
                metadata: {
                  head,
                  operationState: "Clean",
                  ahead: null,
                  behind: null,
                  upstreamRef: null,
                  basis: "stored_refs",
                  integration: null,
                },
              };
            if (method === "repo.worktrees") {
              const offset = Number(params.cursor ?? 0);
              return {
                snapshot: `worktrees${state.revision}`,
                entries: state.rows.slice(offset, offset + 1),
                nextCursor:
                  offset + 1 < state.rows.length ? String(offset + 1) : null,
                metadata: { listToken: `token${state.revision}` },
              };
            }
            if (method === "repo.branches" && state.emptyBranches)
              return {
                snapshot: "empty-branches",
                metadata: {},
                nextCursor: null,
                entries: [],
              };
            if (method === "repo.branches")
              return {
                snapshot: "branches",
                metadata: {},
                nextCursor: null,
                entries: [
                  {
                    name: path("feature"),
                    reference: path("refs/heads/feature"),
                    oid,
                    remote: false,
                    current: bare,
                    upstream: null,
                    tracking: null,
                  },
                ],
              };
            if (method === "operation.start") {
              if (params.expectedSnapshot !== `worktrees${state.revision}`)
                throw new Error("Must use worktree snapshot, not status");
              const action = params.action;
              state.actions.push(action);
              const selected = state.rows.find(
                (item) => item.name?.display === action.name,
              );
              if (action.kind === "worktree.add") {
                if (
                  action.branch !== "feature" ||
                  action.expectedOid !== oid.hex
                )
                  throw new Error("Wrong branch guard");
                state.rows.push({
                  ...row(action.name, "available"),
                  path: action.path,
                  locked: action.locked,
                });
              } else if (!selected) throw new Error("Missing fixture worktree");
              else if (action.kind === "worktree.repair") {
                selected.path = action.path;
                selected.state = "available";
                selected.prunable = false;
              } else if (action.kind === "worktree.lock") {
                selected.locked = true;
                selected.lockReason = path(action.reason);
              } else if (action.kind === "worktree.unlock") {
                selected.locked = false;
                selected.lockReason = null;
              } else if (
                action.kind === "worktree.remove" ||
                action.kind === "worktree.prune"
              ) {
                if (selected.current || selected.locked)
                  throw new Error("Unsafe removal");
                state.rows = state.rows.filter((item) => item !== selected);
              } else throw new Error("Unexpected mutation");
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
    }, bare);
    await page.goto("/");
    await page.getByRole("tab", { name: "Projects", exact: true }).click();
    await page
      .getByRole("button", { name: "Application", exact: true })
      .click();
    await openGitAction(page, "Worktrees");
    const dialog = page.locator(".git-inspector-section");
    await expect(
      dialog.getByText("main · Current", { exact: true }),
    ).toBeVisible();
    const expand = async (name: string) => {
      const row = dialog.locator("details").filter({
        has: page.locator("summary strong", {
          hasText: new RegExp("^" + name + "$"),
        }),
      });
      await expect(async () => {
        await dialog.evaluate((el) => {
          for (let p: Element | null = el; p; p = p.parentElement) {
            if (p.scrollHeight > p.clientHeight) p.scrollTop = p.scrollHeight;
          }
        });
        await expect(row).toHaveCount(1);
      }).toPass({ timeout: 5000 });
      if (!(await row.getAttribute("open"))) {
        if (!(await row.evaluate((el) => (el as HTMLDetailsElement).open)))
          await row.locator("summary").click();
      }
      return row;
    };
    let moved = await expand("moved");
    await moved
      .getByRole("button", { name: "Locate moved worktree…", exact: true })
      .click();
    await dialog.getByLabel("New location on server").fill("/srv/relocated");
    await dialog
      .getByRole("button", { name: "Locate moved worktree", exact: true })
      .click();
    moved = await expand("moved");
    await expect(
      moved.getByText("/srv/relocated", { exact: true }),
    ).toBeVisible();
    await moved.getByRole("button", { name: "Lock…", exact: true }).click();
    await dialog.getByLabel("Reason (optional)").fill("Keep this checkout");
    await dialog
      .getByRole("button", { name: "Lock worktree", exact: true })
      .click();
    moved = await expand("moved");
    await expect(
      moved.getByText("Lock reason: Keep this checkout"),
    ).toBeVisible();
    await expect(
      moved.getByRole("button", { name: "Remove worktree…", exact: true }),
    ).toHaveCount(0);
    await moved.getByRole("button", { name: "Unlock…", exact: true }).click();
    await dialog
      .getByRole("button", { name: "Unlock worktree", exact: true })
      .click();
    moved = await expand("moved");
    await moved
      .getByRole("button", { name: "Remove worktree…", exact: true })
      .click();
    await dialog
      .getByRole("button", { name: "Remove worktree", exact: true })
      .click();
    const orphan = await expand("orphan");
    await orphan
      .getByRole("button", {
        name: "Remove missing registration…",
        exact: true,
      })
      .click();
    await dialog
      .getByRole("button", { name: "Remove missing registration", exact: true })
      .click();
    await dialog
      .getByRole("button", { name: "Add worktree", exact: true })
      .click();
    await dialog
      .getByLabel("Worktree name", { exact: true })
      .fill("feature-checkout");
    await dialog
      .getByLabel("Destination on server")
      .fill("/srv/feature-checkout");
    await dialog
      .getByRole("combobox", { name: "Local branch", exact: true })
      .click();
    await page.getByRole("option", { name: "feature", exact: true }).click();
    await dialog
      .getByRole("button", { name: "Add worktree", exact: true })
      .click();
    const added = await expand("feature-checkout");
    await expect(
      added.getByText("/srv/feature-checkout", { exact: true }),
    ).toBeVisible();
    expect(
      await page.evaluate(() =>
        (window as any).worktreeFixture.actions.map(
          (action: any) => action.kind,
        ),
      ),
    ).toEqual([
      "worktree.repair",
      "worktree.lock",
      "worktree.unlock",
      "worktree.remove",
      "worktree.prune",
      "worktree.add",
    ]);
    await page.evaluate(() => {
      (window as any).worktreeFixture.emptyBranches = true;
    });
    await dialog
      .getByRole("button", { name: "Add worktree", exact: true })
      .click();
    await expect(
      dialog.getByText("No available local branches in the loaded results.", {
        exact: false,
      }),
    ).toBeVisible();
    if (bare)
      await expect(dialog).not.toContainText(
        "The current branch cannot be used.",
      );
    else
      await expect(dialog).toContainText("The current branch cannot be used.");
    await page.screenshot({
      animations: "disabled",
      path: `.impeccable/screenshots/projects-worktree-empty-${bare ? "bare-" : ""}${info.project.name}.png`,
    });
  });

test("follows agents across worktrees: switch, branch hand-off, and a new worktree on a new branch", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const path = (display: string) => ({ display, bytesB64: btoa(display) });
    const oid = (c: string) => ({ algorithm: "sha1", hex: c.repeat(40) });
    const head = (branch: string, c = "a") => ({
      name: path(`refs/heads/${branch}`),
      oid: oid(c),
      unborn: false,
      detached: false,
    });
    const checkouts: Record<
      string,
      { name: string | null; branch: string; files: string[] }
    > = {
      "/srv/app": { name: null, branch: "main", files: ["README.md"] },
      "/srv/app-agent-fix": {
        name: "agent-fix",
        branch: "agent/fix",
        files: ["src/agent-change.ts", "src/agent-test.ts"],
      },
    };
    const state = {
      revision: 0,
      requests: [] as string[],
      actions: [] as any[],
      receipts: [] as any[],
    };
    const rows = (self: string) =>
      Object.entries(checkouts).map(([p, c]) => ({
        name: c.name ? path(c.name) : null,
        kind: c.name ? "linked" : "main",
        state: "available",
        path: path(p),
        gitDir: path(
          c.name ? `/srv/app/.git/worktrees/${c.name}` : "/srv/app/.git",
        ),
        current: p === self,
        head: head(c.branch),
        locked: false,
        lockReason: null,
        prunable: false,
      }));
    Object.assign(window, {
      agentFixture: state,
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
                path: path("/srv/app"),
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
              serverId: "server",
              info: {
                capabilities: {
                  methods: [
                    "repo.open",
                    "repo.status",
                    "repo.history",
                    "repo.worktrees",
                    "repo.branches",
                    "operation.start",
                    "operation.get",
                  ],
                  actions: ["worktree.add", "checkout"],
                  features: [
                    "worktree.new_branch",
                    "worktrees.filter",
                    "worktrees.snapshot_filter",
                  ],
                },
              },
            };
          if (cmd !== "git_request") return;
          const { method, params } = args.request;
          state.requests.push(
            `${method} ${params?.repoId ?? params?.path?.display ?? ""}`.trim(),
          );
          const self = (params?.repoId ?? "").replace(/^repo:/, "");
          if (method === "repo.open") {
            const p = params.path.display;
            const c = checkouts[p];
            if (!c) throw new Error(`No repository at ${p}`);
            return {
              repoId: `repo:${p}`,
              commonRepoId: "common",
              root: path(p),
              bare: false,
              objectFormat: "sha1",
              head: head(c.branch),
              operationState: "Clean",
              integration: null,
              capabilities: { readOnly: false, workingTree: true },
            };
          }
          if (method === "repo.status")
            return {
              snapshot: `status${state.revision}:${self}`,
              entries: checkouts[self].files.map((file, i) => ({
                entryId: `${self}#${i}`,
                path: path(file),
                oldPath: null,
                flags: 256,
                staged: false,
                unstaged: true,
                untracked: false,
                conflicted: false,
                conflict: null,
              })),
              nextCursor: null,
              metadata: {
                head: head(checkouts[self].branch),
                operationState: "Clean",
                integration: null,
                ahead: null,
                behind: null,
                upstreamRef: null,
                basis: "stored_refs",
              },
            };
          if (method === "repo.history")
            return {
              snapshot: "history",
              entries: [],
              nextCursor: null,
              metadata: { resolvedRevision: oid("a"), truncated: false },
            };
          if (method === "repo.worktrees")
            return {
              snapshot: `worktrees${state.revision}`,
              entries: rows(self).filter(
                (row) =>
                  !params.branch || row.head?.name?.display === params.branch,
              ),
              nextCursor: null,
              metadata: { listToken: `token${state.revision}` },
            };
          if (method === "repo.branches")
            return {
              snapshot: "branches",
              nextCursor: null,
              metadata: {},
              entries: Object.values(checkouts).map((c) => ({
                name: path(c.branch),
                reference: path(`refs/heads/${c.branch}`),
                oid: oid("a"),
                remote: false,
                current: c.branch === checkouts[self]?.branch,
                upstream: null,
                tracking: null,
              })),
            };
          if (method === "operation.start") {
            const { action } = params;
            if (params.expectedSnapshot !== `worktrees${state.revision}`)
              throw new Error("Must use the worktree listing snapshot");
            if (
              action.kind !== "worktree.add" ||
              !action.newBranch ||
              action.expectedOid !== "a".repeat(40)
            )
              throw new Error("Wrong worktree request");
            state.actions.push(action);
            checkouts[action.path.display] = {
              name: action.name,
              branch: action.branch,
              files: [],
            };
            state.revision++;
            const outcome = {
              operationId: params.operationId,
              repository: "common",
              payloadHash: "hash",
              seq: 1,
              state: "succeeded",
              result: {
                name: action.name,
                path: action.path,
                branchCreated: true,
              },
              error: null,
            };
            state.receipts.push({
              operationId: params.operationId,
              serverId: "server",
              action: action.kind,
              state: "succeeded",
            });
            return outcome;
          }
          throw new Error(`Unexpected ${method}`);
        },
      },
    });
  });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  const picker = (name: string) =>
    page.getByRole("button", { name: `Worktrees: ${name}`, exact: true });
  await expect(picker("Main worktree")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "README.md", exact: true }),
  ).toBeVisible();

  // Into an agent's checkout: the whole page is that worktree, read by its
  // own id.
  await picker("Main worktree").click();
  await expect(page.getByRole("option", { name: /agent-fix/ })).toBeVisible();
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-worktree-picker.png",
  });
  await page.getByRole("option", { name: /agent-fix/ }).click();
  await expect(picker("agent-fix")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "src/agent-change.ts", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "README.md", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Branches: agent/fix", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => (window as any).agentFixture.requests),
  ).toContain("repo.status repo:/srv/app-agent-fix");

  // A branch another worktree holds cannot be switched to here; choosing it
  // goes to that worktree instead.
  await page
    .getByRole("button", { name: "Branches: agent/fix", exact: true })
    .click();

  await page.getByRole("option", { name: /^main/ }).click();
  await expect(picker("Main worktree")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "README.md", exact: true }),
  ).toBeVisible();

  // A new worktree for a new task, on a new branch, in one step -- then open.
  await picker("Main worktree").click();
  await page.getByRole("button", { name: "New worktree", exact: true }).click();
  await page
    .getByRole("textbox", { name: "Branch name", exact: true })
    .fill("agent/new-task");
  await expect(
    page.getByRole("textbox", { name: "Location on server", exact: true }),
  ).toHaveValue("/srv/app-agent-new-task");
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-worktree-new.png",
  });
  await page
    .getByRole("button", { name: "Create worktree", exact: true })
    .click();
  await expect(picker("agent-new-task")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Branches: agent/new-task", exact: true }),
  ).toBeVisible();
  const actions = await page.evaluate(
    () => (window as any).agentFixture.actions,
  );
  expect(actions).toEqual([
    {
      kind: "worktree.add",
      name: "agent-new-task",
      path: {
        display: "/srv/app-agent-new-task",
        bytesB64: btoa("/srv/app-agent-new-task"),
      },
      branch: "agent/new-task",
      expectedOid: "a".repeat(40),
      locked: false,
      newBranch: true,
    },
  ]);
});

test("checking many agent worktrees never blocks or fails opening the project", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const path = (display: string) => ({ display, bytesB64: btoa(display) });
    const oid = { algorithm: "sha1", hex: "a".repeat(40) };
    const head = (branch: string) => ({
      name: path(`refs/heads/${branch}`),
      oid,
      unborn: false,
      detached: false,
    });
    // Thirty agents, and a listing paged at twenty, as the agent pages at a
    // hundred: the picker must show them all, not the first page.
    const agents = Array.from({ length: 30 }, (_, i) => `agent-${i}`);
    const rows = (self: string) => [
      {
        name: null,
        kind: "main",
        state: "available",
        path: path("/srv/app"),
        gitDir: path("/srv/app/.git"),
        current: self === "/srv/app",
        head: head("main"),
        locked: false,
        lockReason: null,
        prunable: false,
      },
      ...agents.map((name) => ({
        name: path(name),
        kind: "linked",
        state: "available",
        path: path(`/srv/${name}`),
        gitDir: path(`/srv/app/.git/worktrees/${name}`),
        current: self === `/srv/${name}`,
        head: head(`agent/${name}`),
        locked: false,
        lockReason: null,
        prunable: false,
      })),
    ];
    const state = { requests: [] as string[] };
    const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));
    Object.assign(window, {
      manyFixture: state,
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
                path: path("/srv/app"),
              },
            ];
          if (cmd === "git_pending_operations") return [];
          if (cmd === "git_connect")
            return {
              serverId: "server",
              info: {
                capabilities: {
                  features: ["worktrees.filter", "worktrees.snapshot_filter"],
                  methods: [
                    "repo.open",
                    "repo.status",
                    "repo.history",
                    "repo.worktrees",
                    "repo.branches",
                  ],
                  actions: [],
                  features: ["worktrees.filter", "worktrees.snapshot_filter"],
                },
              },
            };
          if (cmd !== "git_request") return;
          const { method, params } = args.request;
          const self = (params?.repoId ?? "").replace(/^repo:/, "");
          state.requests.push(`${method} ${self || params?.path?.display}`);
          if (method === "repo.open")
            return {
              repoId: `repo:${params.path.display}`,
              commonRepoId: "common",
              root: params.path,
              bare: false,
              objectFormat: "sha1",
              head: head("main"),
              operationState: "Clean",
              integration: null,
              capabilities: { readOnly: false, workingTree: true },
            };
          if (method === "repo.status") {
            // An agent's checkout is slow to read, as a big tree is.
            if (self !== "/srv/app") await wait(250);
            return {
              snapshot: `status:${self}`,
              entries: [],
              nextCursor: null,
              metadata: {
                head: head("main"),
                operationState: "Clean",
                integration: null,
                ahead: null,
                behind: null,
                upstreamRef: null,
                basis: "stored_refs",
              },
            };
          }
          if (method === "repo.worktrees") {
            const all = rows(self);
            const offset = Number(params.cursor ?? 0);
            return {
              snapshot: "worktrees",
              entries: all.slice(offset, offset + 20),
              nextCursor: offset + 20 < all.length ? String(offset + 20) : null,
              metadata: { listToken: "token" },
            };
          }
          if (method === "repo.branches")
            return {
              snapshot: "branches",
              nextCursor: null,
              metadata: {},
              entries: [
                {
                  name: path("main"),
                  reference: path("refs/heads/main"),
                  oid,
                  remote: false,
                  current: true,
                  upstream: null,
                  tracking: null,
                },
              ],
            };
          if (method === "repo.history")
            return {
              snapshot: "history",
              entries: [],
              nextCursor: null,
              metadata: { resolvedRevision: oid, truncated: false },
            };
          throw new Error(`Unexpected ${method}`);
        },
      },
    });
  });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Check status", exact: true }).click();
  const reads = () =>
    page.evaluate(
      () =>
        (window as any).manyFixture.requests.filter((r: string) =>
          /^repo\.status \/srv\/agent-/.test(r),
        ).length,
    );
  // The project library does not eagerly inspect every worktree.
  await expect(
    page.getByRole("button", { name: "Application", exact: true }),
  ).toBeVisible();
  expect(await reads()).toBe(0);
  await expect(
    page.getByRole("button", { name: "30 worktrees", exact: true }),
  ).toHaveCount(0);
  // ...and opening the project mid-run stops it.
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Worktrees: Main worktree", exact: true }),
  ).toBeVisible();
  const atOpen = await reads();
  expect(atOpen).toBeLessThan(30);
  await page.waitForTimeout(1200);
  // At most the read already in flight finished; no more were started.
  expect(await reads()).toBeLessThanOrEqual(atOpen + 1);
  // A cancelled read is not a failure.
  await expect(page.getByText(/Cannot read/)).toHaveCount(0);
  // The connection is free: branches load at once.
  await page
    .getByRole("button", { name: "Branches: main", exact: true })
    .click();
  await expect(page.getByRole("option", { name: /^main/ })).toBeVisible({
    timeout: 1000,
  });
  await page.keyboard.press("Escape");
  // Every worktree is listed, past the listing's first page.
  await page
    .getByRole("button", { name: "Worktrees: Main worktree", exact: true })
    .click();
  await expect(page.getByRole("option")).toHaveCount(20);
  await page.getByRole("option").last().scrollIntoViewIfNeeded();
  await expect(page.getByRole("option")).toHaveCount(31);
  await expect(page.getByRole("option", { name: /agent-29/ })).toBeAttached();
});
