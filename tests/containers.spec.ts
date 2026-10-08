import { test, expect, type Page } from "@playwright/test";
import { installAppFixture } from "./fixtures/appFixture";

/**
 * Containers on the invented fixture host: an "atlas" project with an
 * unhealthy worker, a "ledger" project with an out-of-memory web container
 * and a crash-looping queue, and two standalone containers.
 */
async function openContainers(page: Page, fixture?: string) {
  await installAppFixture(page);
  await page.goto(fixture ? `/?fixture=${fixture}` : "/");
  await page.getByRole("tab", { name: "Containers", exact: true }).click();
  if (fixture !== "no-docker")
    await expect(
      page.getByRole("navigation", { name: "Containers" }),
    ).toBeVisible();
}
const row = (page: Page, name: string) =>
  page.getByRole("button", { name: new RegExp(`^${name}:`) });
const detail = (page: Page, name: string) =>
  page.getByRole("article", { name, exact: true });
/** The state line under a detail's title (the log footer is a status too). */
const state = (article: ReturnType<typeof detail>) =>
  article.getByRole("status").first();
const actions = (page: Page) =>
  page.evaluate(() => (window as any).__docker.actions as any[]);
const typed = (page: Page) =>
  page.evaluate(() =>
    new TextDecoder().decode(
      new Uint8Array((window as any).__terminalWrites.flat()),
    ),
  );

test("projects needing attention lead, with trouble marked in place", async ({
  page,
}) => {
  await openContainers(page);
  await expect(
    page.getByText("6 running · 2 stopped · 3 need attention"),
  ).toBeVisible();
  await expect(page.getByText(/^Docker 27\.3\.1 · Updated/)).toBeVisible();
  const projects = page.getByRole("list", { name: "Projects", exact: true });
  await expect(
    projects.getByRole("button", { name: /Compose project/ }),
  ).toHaveText([/atlas.*1 issue/, /ledger.*2 issues/]);
  await expect(
    page.getByRole("list", { name: "atlas containers" }).getByRole("button"),
  ).toHaveText([/api.*%/, /db.*%/, /worker.*unhealthy/, /1 one-off job/]);
  await expect(
    page.getByRole("list", { name: "ledger containers" }).getByRole("button"),
  ).toHaveText([/queue.*restart ×6/, /redis/, /web.*OOM/]);
  await expect(
    page.getByRole("list", { name: "Standalone" }).getByRole("button"),
  ).toHaveText([/buildkit.*21d ago/, /portainer/]);
  // Nothing is listed twice.
  await expect(row(page, "atlas-worker-1")).toHaveCount(1);
  // The first container in trouble is open without a click.
  await expect(detail(page, "ledger-queue-1")).toContainText(
    "Keeps exiting; restarted 6×",
  );

  // Folding a project keeps its issue count visible.
  await page.getByRole("button", { name: "Fold atlas" }).click();
  await expect(
    page.getByRole("list", { name: "atlas containers" }),
  ).toHaveCount(0);
  await expect(row(page, "atlas")).toContainText("1 issue");
  // Search reaches images and opens what matches.
  await page.getByLabel("Find a container").fill("postgres");
  await expect(
    page.getByRole("list", { name: "atlas containers" }).getByRole("button"),
  ).toHaveText([/db/]);
  await page.getByLabel("Find a container").fill("nothing-like-this");
  await expect(
    page.getByText("No projects or containers match."),
  ).toBeVisible();
});

test("an unhealthy container shows its failing check, logs and settings", async ({
  page,
}) => {
  await openContainers(page);
  await row(page, "atlas-worker-1").click();
  const worker = detail(page, "atlas-worker-1");
  await expect(worker).toContainText("Unhealthy");
  await expect(worker).toContainText("Failing its health check.");
  await expect(worker).toContainText(
    "curl: (7) Failed to connect to localhost port 8081",
  );
  const log = worker.getByRole("log");
  await expect(log).toContainText("ERROR connection refused to db:5432");
  await expect(log).toContainText("GET /health 200 (1)", { timeout: 6_000 });
  // No levels in container logs: the words decide, filtered here.
  await worker.getByRole("radio", { name: "Errors" }).click();
  await expect(log).not.toContainText("WARN slow query");
  await expect(log).toContainText("ERROR connection refused");
  await worker.getByRole("radio", { name: "All" }).click();

  await worker.getByRole("tab", { name: "Inspect" }).click();
  await expect(worker).toContainText(
    "/usr/local/bin/entrypoint serve --port 8080",
  );
  const env = worker.getByRole("region", { name: "Environment" });
  await expect(env).not.toContainText("production");
  await env.getByRole("button", { name: "Show values" }).click();
  await expect(env).toContainText("production");
  // Secret-looking names need their own click.
  await expect(env).not.toContainText("example-only");
  await env.getByRole("button", { name: "Show POSTGRES_PASSWORD" }).click();
  await expect(env).toContainText("example-only");
  await expect(worker.getByRole("region", { name: "Mounts" })).toContainText(
    "/data ← volume atlas_pgdata",
  );
  await expect(
    worker.getByRole("region", { name: "Health check" }),
  ).toContainText("failed (7)");
});

test("lifecycle controls act at once, and stopping or removing asks first", async ({
  page,
}) => {
  await openContainers(page);
  await row(page, "atlas-api-1").click();
  const api = detail(page, "atlas-api-1");
  await expect(api).toContainText("180.0 MiB of 512.0 MiB");
  await api.getByRole("button", { name: "Restart", exact: true }).click();
  await expect(page.getByText("Restarted api")).toBeVisible();

  await api.getByRole("button", { name: "Stop", exact: true }).click();
  const confirm = page.getByRole("dialog", { name: "Stop api?" });
  await expect(confirm).toContainText("Anything using it stops working");
  await confirm.getByRole("button", { name: "Stop" }).click();
  await expect(state(api)).toHaveText(/^Stopped/);
  await expect(row(page, "atlas-api-1")).toBeVisible();
  await api.getByRole("button", { name: "Start", exact: true }).click();
  await expect(state(api)).toHaveText(/^Starting|^Running/);

  await api
    .getByRole("button", { name: "More actions for atlas-api-1" })
    .click();
  await page.getByRole("menuitem", { name: "Pause" }).click();
  await expect(state(api)).toHaveText(/^Paused/);
  await api.getByRole("button", { name: "Resume" }).click();
  await expect(state(api)).not.toHaveText(/^Paused/);

  expect((await actions(page)).map((a) => a.verb)).toEqual([
    "restart",
    "stop",
    "start",
    "pause",
    "unpause",
  ]);

  // A container killed for memory explains itself, and can go.
  await row(page, "ledger-web-1").click();
  const web = detail(page, "ledger-web-1");
  await expect(state(web)).toHaveText(/^Out of memory/);
  await expect(web).toContainText("Killed for running out of memory.");
  await web.getByRole("button", { name: "Remove" }).click();
  await page
    .getByRole("dialog", { name: "Remove ledger-web-1?" })
    .getByRole("button", { name: "Remove" })
    .click();
  await expect(row(page, "ledger-web-1")).toHaveCount(0);
  await expect(page.getByText("That container is gone.")).toBeVisible();
});

test("a shell opens in a new Terminal tab and runs", async ({ page }) => {
  await openContainers(page);
  await row(page, "atlas-api-1").click();
  await detail(page, "atlas-api-1")
    .getByRole("button", { name: "Shell" })
    .click();
  await expect(
    page.getByRole("tab", { name: "Terminal", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect
    .poll(() => typed(page))
    .toContain(
      "docker exec -it atlas-api-1 sh -c 'command -v bash >/dev/null && exec bash || exec sh'\r",
    );
});

test("a Compose project starts, stops and tidies up as a whole", async ({
  page,
}) => {
  // Engine-level controls, as on a server without the Compose CLI.
  await openContainers(page, "no-compose");
  await row(page, "atlas").click();
  const atlas = detail(page, "atlas project");
  await expect(atlas).toContainText("/srv/atlas");
  await expect(atlas).toContainText("Needs attention");
  await expect(atlas).toContainText("3 of 3 services running");
  await expect(
    atlas.getByRole("table", { name: "atlas services" }).getByRole("row"),
  ).toHaveCount(5);
  // Merged logs name the service each line came from.
  await expect(atlas.getByRole("log")).toContainText("worker: WARN slow query");

  await atlas.getByRole("button", { name: "Stop all" }).click();
  const confirm = page.getByRole("dialog", { name: "Stop atlas?" });
  await expect(confirm.getByRole("listitem")).toHaveCount(3);
  await confirm.getByRole("button", { name: "Stop all" }).click();
  await expect(atlas).toContainText("0 of 3 services running");
  // Starting asks nothing.
  await atlas.getByRole("button", { name: "Start all" }).click();
  await expect(atlas).toContainText("3 of 3 services running");
  const [stop, start] = await actions(page);
  expect(stop).toMatchObject({ project: "atlas", verb: "stop" });
  expect(stop.expectedIds).toHaveLength(3);
  expect(start).toMatchObject({ project: "atlas", verb: "start" });

  await atlas.getByRole("button", { name: "Remove 1 finished job" }).click();
  await page
    .getByRole("dialog", { name: "Remove 1 finished job?" })
    .getByRole("button", { name: "Remove" })
    .click();
  await expect(page.getByText("1 one-off job")).toHaveCount(0);

  // A member opens from the table.
  await atlas.getByRole("button", { name: "db" }).click();
  await expect(detail(page, "atlas-db-1")).toBeVisible();
  await detail(page, "atlas-db-1")
    .getByRole("button", { name: "atlas", exact: true })
    .click();
  await expect(detail(page, "atlas project")).toBeVisible();
});

test("a published port forwards to this computer and opens", async ({ page }) => {
  await openContainers(page);
  await row(page, "atlas-api-1").click();
  const api = detail(page, "atlas-api-1");
  await api.getByRole("button", { name: "Forward and open port 8080" }).click();
  await expect(
    api.getByRole("button", { name: "Open port 8080 in browser" }),
  ).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(() => ((window as any).__openedTunnels ?? []).length),
    )
    .toBe(1);
  const tunnels = await page.evaluate(async () =>
    (
      await (window as any).__TAURI_INTERNALS__.invoke("snapshot")
    ).config.tunnels.filter((t: any) => t.name === "api"),
  );
  expect(tunnels).toEqual([
    expect.objectContaining({
      remoteHost: "127.0.0.1",
      remotePort: 8080,
      direction: "local",
    }),
  ]);
  // An internal port and a stopped container's port are not forwardable.
  await row(page, "atlas-db-1").click();
  await expect(detail(page, "atlas-db-1")).toContainText("internal 5432");
  await row(page, "ledger-web-1").click();
  await expect(
    // A saved forward already reaches it: connect that, don't add another.
    detail(page, "ledger-web-1").getByRole("button", {
      name: "Connect and open port 3000",
    }),
  ).toBeDisabled();
});

test("disk usage shows what can go, and removing it asks first", async ({
  page,
}) => {
  await openContainers(page);
  await page.getByRole("button", { name: "Disk usage" }).click();
  const disk = detail(page, "Disk usage");
  await expect(disk).toContainText("Docker uses 4.3 GiB on this server");
  await expect(
    disk.getByRole("region", { name: "Images" }).first(),
  ).toContainText("625.0 MiB can be freed");
  await expect(disk).toContainText("untagged d4e5f6a7b8c9");

  await disk.getByRole("button", { name: "Clean up" }).click();
  await page.getByRole("menuitem", { name: /Remove dangling images/ }).click();
  await page
    .getByRole("dialog", { name: "Remove dangling images?" })
    .getByRole("button", { name: "Remove dangling" })
    .click();
  await expect(page.getByText("Freed 220.0 MiB")).toBeVisible();
  await expect(disk).not.toContainText("untagged d4e5f6a7b8c9");

  await disk.getByRole("button", { name: "Remove unused" }).click();
  const volumes = page.getByRole("dialog", {
    name: "Remove unused anonymous volumes?",
  });
  await expect(volumes).toContainText("deleted for good");
  await volumes.getByRole("button", { name: "Cancel" }).click();
  expect((await actions(page)).map((a) => a.what)).toEqual(["dangling-images"]);
});

test("without Docker access, the page says why and how to fix it", async ({
  page,
}) => {
  await openContainers(page, "no-docker");
  await expect(page.getByText("This account can’t use Docker")).toBeVisible();
  await page
    .getByRole("button", { name: "Add me to the docker group…" })
    .click();
  await expect
    .poll(() => typed(page))
    .toContain("sudo usermod -aG docker $USER");
  // Typed for the person to confirm, never run.
  expect(await typed(page)).not.toContain("$USER\r");
});

test("arrow keys walk the list; narrow windows show one side", async ({
  page,
}) => {
  await openContainers(page);
  await row(page, "atlas").focus();
  await page.keyboard.press("ArrowDown");
  await expect(detail(page, "atlas-api-1")).toBeVisible();
  await page.keyboard.press("End");
  await expect(detail(page, "portainer")).toBeVisible();
  await page.keyboard.press("Home");
  await expect(detail(page, "Disk usage")).toBeVisible();

  await page.setViewportSize({ width: 700, height: 700 });
  await expect(page.getByLabel("Find a container")).toBeHidden();
  await page.getByRole("button", { name: "All containers" }).click();
  await expect(page.getByLabel("Find a container")).toBeVisible();
  await row(page, "portainer").click();
  await expect(detail(page, "portainer")).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

const composeCalls = (page: Page) =>
  page.evaluate(() => (window as any).__docker.compose?.calls ?? []);
const openProject = async (page: Page, name: string, fixture?: string) => {
  await openContainers(page, fixture);
  await row(page, name).click();
  return detail(page, `${name} project`);
};

test("Compose actions stream their output, and the files are readable", async ({
  page,
}) => {
  const atlas = await openProject(page, "atlas");
  await expect(atlas).toContainText("Compose 2.29.7");
  // Services the files define but nothing created yet are listed too.
  await expect(
    atlas.getByRole("row").filter({ hasText: "docs" }),
  ).toContainText("Not created · profile docs");
  await atlas.getByRole("button", { name: "More" }).click();
  await page.getByRole("menuitem", { name: "Pull images & up" }).click();
  // Starting a command opens its output.
  await expect(atlas.getByRole("tab", { name: "Activity" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(atlas).toContainText(
    "docker compose -p atlas up -d --pull always",
  );
  await expect(atlas.getByText(/^Running · /)).toBeVisible();
  // One command per project: the rest wait.
  await expect(
    atlas.getByRole("button", { name: "Up", exact: true }),
  ).toBeDisabled();
  await expect(atlas.getByRole("log")).toContainText("api Pulling");
  await expect(atlas.getByText(/Succeeded in/)).toBeVisible();
  await expect(page.getByText("atlas: done")).toBeVisible();
  await expect(atlas.getByRole("log")).toContainText(
    "WARN[0000] the attribute `version` is obsolete",
  );

  await atlas.getByRole("tab", { name: "Compose file" }).click();
  await expect(atlas).toContainText("/srv/atlas/compose.yaml");
  await expect(atlas.getByText("image: postgres:16")).toBeVisible();
});

test("a service recreates, comes up or scales on its own", async ({ page }) => {
  const atlas = await openProject(page, "atlas");
  await atlas.getByRole("button", { name: "Up docs" }).click();
  await expect(atlas.getByText(/Succeeded in/)).toBeVisible();
  await expect(
    atlas.getByRole("row").filter({ hasText: "docs" }),
  ).toContainText("Running");

  await atlas
    .getByRole("button", { name: "Compose actions for worker" })
    .click();
  await page.getByRole("menuitem", { name: "Scale…" }).click();
  const scale = page.getByRole("dialog", { name: "Scale worker" });
  await scale.getByLabel("Containers").fill("3");
  await scale.getByRole("button", { name: "Run 3" }).click();
  await expect(atlas).toContainText("--scale worker=3 worker");
  await expect(
    atlas.getByRole("row").filter({ hasText: /worker#3/ }),
  ).toHaveCount(1, {
    timeout: 6_000,
  });

  await atlas.getByRole("button", { name: "Compose actions for api" }).click();
  await page.getByRole("menuitem", { name: "Rebuild" }).click();
  await expect(atlas).toContainText(
    "up -d --build --force-recreate --no-deps api",
  );
  expect((await composeCalls(page)).map((c: any) => c.action.kind)).toEqual([
    "up",
    "scale",
    "up",
  ]);
});

test("a container's menu recreates it through Compose, with a way to the output", async ({
  page,
}) => {
  await openContainers(page);
  await row(page, "atlas-api-1").click();
  const api = detail(page, "atlas-api-1");
  await api
    .getByRole("button", { name: "More actions for atlas-api-1" })
    .click();
  await page.getByRole("menuitem", { name: "Pull & recreate" }).click();
  await expect(page.getByText("Pulling and recreating api…")).toBeVisible();
  await expect(api.getByText("Compose is working on atlas…")).toBeVisible();
  await api.getByRole("button", { name: "View output" }).click();
  const atlas = detail(page, "atlas project");
  await expect(atlas.getByRole("tab", { name: "Activity" })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(atlas).toContainText(
    "up -d --pull always --force-recreate --no-deps api",
  );
});

test("Down asks, and deleting volumes needs the project's name", async ({
  page,
}) => {
  const atlas = await openProject(page, "atlas");
  await atlas.getByRole("button", { name: "More" }).click();
  await page.getByRole("menuitem", { name: "Down…" }).click();
  const down = page.getByRole("dialog", { name: "Take atlas down?" });
  await down
    .getByRole("switch", { name: "Delete the project’s volumes" })
    .click();
  await expect(down).toContainText("deleted for good");
  const confirm = down.getByRole("button", { name: "Down & delete volumes" });
  await expect(confirm).toBeDisabled();
  await down.getByLabel("Project name").fill("atlas");
  await confirm.click();
  await expect(atlas).toContainText("docker compose -p atlas down --volumes");
  // The project is gone when it finishes, and so is its volume.
  await expect(row(page, "atlas")).toHaveCount(0, { timeout: 6_000 });
  expect(
    await page.evaluate(() =>
      (window as any).__docker.volumes.map((v: any) => v.name),
    ),
  ).not.toContain("atlas_pgdata");
});

test("a running Compose command can be cancelled, and failures say so", async ({
  page,
}) => {
  const atlas = await openProject(page, "atlas");
  await atlas.getByRole("button", { name: "More" }).click();
  await page.getByRole("menuitem", { name: "Pull images only" }).click();
  await atlas.getByRole("button", { name: "Cancel" }).click();
  await expect(atlas.getByText(/Cancelled after/)).toBeVisible();
  await expect(page.getByText("atlas: cancelled")).toBeVisible();

  const ledger = await openProject(page, "ledger", "compose-fail");
  await ledger.getByRole("button", { name: "Up", exact: true }).click();
  await expect(ledger.getByText(/Failed \(exit 1\)/)).toBeVisible();
  await expect(page.getByText("ledger: failed (exit 1)")).toBeVisible();
  await expect(ledger.getByRole("log")).toContainText(
    "Error response from daemon",
  );
});

test("without the Compose CLI, the project keeps start, stop and restart", async ({
  page,
}) => {
  const atlas = await openProject(page, "atlas", "no-compose");
  await expect(atlas).toContainText(
    "Up, Pull, Rebuild and Down need the Compose CLI on the server: The Compose CLI isn't installed on this server.",
  );
  await expect(
    atlas.getByRole("button", { name: "Up", exact: true }),
  ).toHaveCount(0);
  await expect(atlas.getByRole("button", { name: "More" })).toHaveCount(0);
  await expect(
    atlas.getByRole("button", { name: "Restart all" }),
  ).toBeVisible();
});

test("with Compose, existing containers can still start without recreating", async ({
  page,
}) => {
  const atlas = await openProject(page, "atlas");
  await atlas.getByRole("button", { name: "Stop all" }).click();
  await page
    .getByRole("dialog", { name: "Stop atlas?" })
    .getByRole("button", { name: "Stop all" })
    .click();
  await expect(atlas).toContainText("0 of 3 services running");
  await atlas.getByRole("button", { name: "More" }).click();
  await page
    .getByRole("menuitem", { name: "Start existing containers" })
    .click();
  await expect(atlas).toContainText("3 of 3 services running");
  expect(await composeCalls(page)).toEqual([]);
});
