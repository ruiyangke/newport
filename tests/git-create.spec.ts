import { test, expect, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

/** Creation actions moved into the library's "Add project" menu. */
async function libraryAction(page: Page, item: string) {
  await page.getByRole("button", { name: "Add project", exact: true }).click();
  await page.getByRole("menuitem", { name: item, exact: true }).click();
}

/** The repository picker replaced the back control; "All projects" returns. */
async function backToProjects(page: Page) {
  await page.getByRole("button", { name: /^Current repository:/ }).click();
  await page
    .getByRole("menuitem", { name: "All projects", exact: true })
    .click();
}

test("creates, clones, and recovers a created repository without replay", async ({
  page,
}, info) => {
  await page.addInitScript(() => {
    const path = (display: string) => ({ display, bytesB64: btoa(display) });
    const head = {
      name: path("refs/heads/main"),
      oid: null,
      unborn: true,
      detached: false,
    };
    const state = {
      projects: [] as any[],
      receipts: [] as any[],
      requests: [] as any[],
      failSave: false,
      lostReply: false,
    };
    Object.assign(window, {
      creationFixture: state,
      isTauri: true,
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: "main" } },
        invoke: async (cmd: string, args: any) => {
          if (cmd === "snapshot")
            return {
              config: {
                servers: [
                  {
                    id: "server",
                    name: "Development",
                    sshUser: "dev",
                    sshHost: "dev.example.com",
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
          if (cmd === "git_projects_list") return state.projects;
          if (cmd === "git_projects_save") {
            if (state.failSave) {
              state.failSave = false;
              throw new Error("Cannot save bookmark");
            }
            state.projects = [...state.projects, args.project];
            return args.project;
          }
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
                    "repo.init",
                    "repo.clone",
                    "repo.open",
                    "repo.status",
                    "repo.close",
                  ],
                },
              },
            };
          if (cmd === "git_request") {
            const { method, params } = args.request;
            state.requests.push(args.request);
            if (method === "repo.init" || method === "repo.clone") {
              if (params.initialBranch === "bad..branch") {
                state.receipts.push({
                  operationId: params.operationId,
                  serverId: "server",
                  action: method,
                  state: "rejected",
                });
                throw {
                  code: "INVALID_REQUEST",
                  message: "Supply a valid local branch name.",
                };
              }
              state.receipts.push({
                operationId: params.operationId,
                serverId: "server",
                action: method,
                state: state.lostReply ? "pending" : "succeeded",
              });
              if (state.lostReply)
                throw { code: "TRANSPORT", message: "Connection lost" };
              return {
                operationId: params.operationId,
                repository: "opaque",
                payloadHash: "hash",
                state: "succeeded",
                seq: 1,
                result: { path: params.path },
                error: null,
              };
            }
            if (method === "repo.open")
              return {
                repoId: "repo",
                commonRepoId: "common",
                root: params.path,
                bare: false,
                objectFormat: "sha1",
                head,
                operationState: "Clean",
                integration: null,
                capabilities: { readOnly: false, workingTree: true },
              };
            if (method === "repo.status")
              return {
                snapshot: "s",
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
            if (method === "repo.close") return { closed: true };
            throw new Error(`Unexpected method ${method}`);
          }
        },
        transformCallback: () => 1,
      },
    });
  });
  await page.goto("/");
  await page.getByRole("tab", { name: "Projects", exact: true }).click();

  await libraryAction(page, "New repository");
  let dialog = page.getByRole("dialog");
  await dialog.getByLabel("Project name").fill("New service");
  // The destination can be browsed rather than typed.
  await dialog.getByRole("button", { name: "Browse…", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Choose the directory to initialize" }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await dialog
    .getByLabel("Existing directory on server")
    .fill("/srv/new-service");
  await dialog.getByLabel("Initial branch").fill("bad..branch");
  await dialog
    .getByRole("button", { name: "Create repository", exact: true })
    .click();
  await expect(dialog.getByRole("alert")).toContainText("valid local branch");
  await expect(
    dialog.getByRole("button", { name: "Create repository", exact: true }),
  ).toBeEnabled();
  await dialog.getByLabel("Initial branch").fill("main");
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-create-${info.project.name}.png`,
  });
  await dialog
    .getByRole("button", { name: "Create repository", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "New service", exact: true }),
  ).toBeVisible();
  const initializationIds = await page.evaluate(() =>
    (window as any).creationFixture.requests
      .filter((r: any) => r.method === "repo.init")
      .map((r: any) => r.params.operationId),
  );
  expect(initializationIds).toHaveLength(2);
  expect(new Set(initializationIds).size).toBe(2);
  await backToProjects(page);

  await libraryAction(page, "Clone repository");
  dialog = page.getByRole("dialog");
  await dialog.getByLabel("Project name").fill("Cloned service");
  await dialog
    .getByLabel("Repository URL")
    .fill("git@example.com:team/service.git");
  await dialog.getByLabel("Destination on server").fill("/srv/cloned-service");
  await dialog.getByLabel("Branch (optional)").fill("develop");
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-clone-${info.project.name}.png`,
  });
  await page.evaluate(() => {
    (window as any).creationFixture.failSave = true;
  });
  await dialog
    .getByRole("button", { name: "Clone repository", exact: true })
    .click();
  await expect(
    dialog.getByRole("heading", { name: "Add repository", exact: true }),
  ).toBeVisible();
  await expect(dialog.getByRole("alert")).toContainText(
    "repository was created",
  );
  await expect(dialog.getByLabel("Repository path on server")).toHaveValue(
    "/srv/cloned-service",
  );
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-create-recovery-${info.project.name}.png`,
  });
  await dialog
    .getByRole("button", { name: "Add repository", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Cloned service", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() =>
      (window as any).creationFixture.requests.filter(
        (r: any) => r.method === "repo.clone",
      ),
    ),
  ).toMatchObject([
    { params: { url: "git@example.com:team/service.git", branch: "develop" } },
  ]);
  await backToProjects(page);

  await libraryAction(page, "Clone repository");
  dialog = page.getByRole("dialog");
  await dialog.getByLabel("Project name").fill("Uncertain clone");
  await dialog
    .getByLabel("Repository URL")
    .fill("https://example.com/public.git");
  await dialog.getByLabel("Destination on server").fill("/srv/uncertain");
  await page.evaluate(() => {
    (window as any).creationFixture.lostReply = true;
  });
  await dialog
    .getByRole("button", { name: "Clone repository", exact: true })
    .click();
  await expect(
    dialog.getByRole("button", { name: "Clone repository", exact: true }),
  ).toBeDisabled();
  await expect(dialog.getByRole("status")).toContainText("saved operation");
  await expect(
    dialog.getByRole("button", { name: "Cancel", exact: true }),
  ).toBeEnabled();
  expect(
    await page.evaluate(
      () =>
        (window as any).creationFixture.requests.filter(
          (r: any) => r.method === "repo.clone",
        ).length,
    ),
  ).toBe(2);
});
