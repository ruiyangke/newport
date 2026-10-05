import { test, expect, type Page } from "@playwright/test";
import { forward, listeners, openPorts } from "./fixtures/ports";

const region = (page: Page, name: string) =>
  page.getByRole("region", { name: `${name} project` });
const row = (page: Page, name: string) =>
  page.getByRole("listitem", { name, exact: true });
const saved = (page: Page) =>
  page
    .evaluate(() => (window as any).__TAURI_INTERNALS__.invoke("snapshot"))
    .then((snapshot) => snapshot.config.tunnels as Record<string, unknown>[]);

test("listening ports group into projects, worktrees and other services", async ({
  page,
}) => {
  await openPorts(page);
  await expect(page.getByRole("region", { name: /project$/ })).toHaveCount(4);
  await expect(
    page.getByText("0 forwarding · 10 listening in 3 projects"),
  ).toBeVisible();

  // A saved Git project names its repository; both checkouts belong to it.
  const atlas = region(page, "Atlas");
  await expect(atlas).toContainText("~/work/atlas");
  await expect(atlas.getByText("main", { exact: true })).toBeVisible();
  await expect(atlas.getByText("feat/search", { exact: true })).toBeVisible();
  await expect(atlas).toContainText("worktree · ../atlas-search/");
  await expect(atlas.getByRole("listitem")).toHaveCount(4);
  // Services are named by what they are, with where and how they run below.
  await expect(row(page, "port 4173 (Vite)")).toContainText("app/ · Node");
  await expect(row(page, "port 9000 (Uvicorn)")).toContainText(
    "api/ · Python · all interfaces",
  );
  // Process identity is a hover away rather than part of the scan.
  const vite = row(page, "port 4173 (Vite)");
  await expect(vite).not.toContainText("201");
  await expect(vite.locator("[title]").first()).toHaveAttribute(
    "title",
    /PID 201 · ci/,
  );

  // Compose services are named by service, not by container.
  const ledger = region(page, "ledger");
  await expect(row(page, "port 5432 (db)")).toContainText("Postgres · Docker");
  await expect(ledger).not.toContainText("ledger-db-1");
  await expect(region(page, "notes").getByRole("listitem")).toHaveCount(1);
  await expect(row(page, "port 3030 (Next.js)")).toBeVisible();

  // Everything else waits, collapsed, until asked for.
  const other = region(page, "Other services");
  await expect(other.getByRole("listitem")).toHaveCount(0);
  await other.getByRole("button", { name: "Expand Other services" }).click();
  await expect(other.getByRole("listitem")).toHaveCount(3);
  await expect(row(page, "port 22 (SSH)")).toContainText(
    "sshd · all interfaces",
  );
});

test("a row's switch forwards its port on the same local port, then stops it", async ({
  page,
}) => {
  await openPorts(page);
  const vite = row(page, "port 4173 (Vite)");
  const toggle = vite.getByRole("switch", { name: "Forward port 4173 (Vite)" });
  await expect(toggle).not.toBeChecked();
  await toggle.click();
  await expect(toggle).toBeChecked();
  await expect(
    vite.getByRole("button", { name: "Open port 4173 (Vite) in browser" }),
  ).toHaveText("localhost:4173");
  expect(await saved(page)).toEqual([
    expect.objectContaining({
      name: "Vite",
      localPort: 4173,
      remotePort: 4173,
      remoteHost: "127.0.0.1",
      direction: "local",
      project: "/home/ci/work/atlas",
    }),
  ]);
  await toggle.click();
  await expect(toggle).not.toBeChecked();
  // Stopping keeps the forward; the address stays, no longer a link.
  await expect(vite).toContainText("localhost:4173");
  await expect(vite.getByRole("button", { name: /in browser/ })).toHaveCount(0);
  expect(await saved(page)).toHaveLength(1);
});

test("forward all covers one worktree or a whole project, and stop all ends it", async ({
  page,
}) => {
  await openPorts(page, {
    // A saved forward already holds 9001 on this Mac.
    tunnels: [forward("Elsewhere", 9500, { localPort: 9001 })],
  });
  const atlas = region(page, "Atlas");
  const on = atlas.getByRole("switch", { checked: true });
  await atlas
    .getByRole("button", { name: "Forward all ports in feat/search" })
    .click();
  await expect(on).toHaveCount(2);
  // A linked worktree's forwards carry its branch; a taken local port moves.
  await expect(row(page, "port 4174 (Vite)")).toContainText(
    "Vite · feat/search",
  );
  await expect(row(page, "port 9001 (Uvicorn)")).toContainText(
    "localhost:9002",
  );
  await atlas
    .getByRole("button", { name: "Forward all ports in Atlas" })
    .click();
  await expect(on).toHaveCount(4);
  await expect(page.getByText(/^4 forwarding/)).toBeVisible();
  await atlas
    .getByRole("button", { name: "Stop all forwards in Atlas" })
    .click();
  await expect(on).toHaveCount(0);
  // Forwarding all again reconnects what exists rather than adding more.
  await atlas
    .getByRole("button", { name: "Forward all ports in Atlas" })
    .click();
  await expect(on).toHaveCount(4);
  expect(await saved(page)).toHaveLength(5);
  // One-row projects need no batch control; their switch is enough.
  await expect(
    region(page, "notes").getByRole("button", { name: /^Forward all/ }),
  ).toHaveCount(0);
});

test("a failing forward says what is wrong and offers the fix", async ({
  page,
}) => {
  await openPorts(page, {
    tunnels: [forward("cache", 6379, { project: "/srv/ledger" })],
    states: {
      cache: {
        status: "error",
        errorMessage: "Cannot bind local port 6379: Address already in use",
        reconnectAttempt: 0,
      },
    },
  });
  const cache = row(page, "port 6379 (cache)");
  await expect(cache).toContainText(
    "Cannot bind local port 6379: Address already in use",
  );
  await expect(page.getByText("1 needs attention")).toBeVisible();
  await page.getByRole("radio", { name: "Needs attention" }).click();
  await expect(page.getByRole("listitem")).toHaveCount(1);
  await cache.getByRole("button", { name: "Edit forward" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit forward" });
  await expect(dialog.getByLabel("Local port or range")).toHaveValue("6379");
  await dialog.getByLabel("Local port or range").fill("16379");
  await dialog.getByRole("button", { name: "Save forward" }).click();
  await expect(dialog).toHaveCount(0);
  expect(await saved(page)).toEqual([
    expect.objectContaining({ localPort: 16379, remotePort: 6379 }),
  ]);
});

test("unreachable destinations are reported until they recover", async ({
  page,
}) => {
  await openPorts(page, {
    tunnels: [forward("API", 9000, { project: "/home/ci/work/atlas" })],
    states: {
      API: { status: "connected", errorMessage: null, reconnectAttempt: 0 },
    },
    health: {
      API: [
        {
          localPort: 9000,
          remotePort: 9000,
          status: "unavailable",
          message: "The server could not connect to this destination.",
          checkedAt: 1,
        },
      ],
    },
  });
  const api = row(page, "port 9000 (Uvicorn)");
  await expect(api).toContainText(
    "Destination unavailable. The server could not connect",
  );
  await expect(page.getByText("1 needs attention")).toBeVisible();
  await page.evaluate(() => {
    (window as any).__ports.health.API[0] = {
      ...(window as any).__ports.health.API[0],
      status: "reachable",
      message: null,
    };
  });
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  await expect(api).not.toContainText("Destination unavailable");
  await expect(
    api.getByRole("button", { name: "Open port 9000 (Uvicorn) in browser" }),
  ).toBeVisible();
  await expect(page.getByText(/needs attention/)).toHaveCount(0);
});

test("forward with options starts from what discovery found", async ({
  page,
}) => {
  await openPorts(page);
  const db = row(page, "port 5432 (db)");
  await db.getByRole("button", { name: "Actions for port 5432 (db)" }).click();
  await page.getByRole("menuitem", { name: "Forward with options…" }).click();
  const dialog = page.getByRole("dialog", { name: "New forward" });
  await expect(dialog.getByLabel("Name")).toHaveValue("db");
  await expect(dialog.getByLabel("Local port or range")).toHaveValue("5432");
  await expect(dialog.getByLabel("Remote port or range")).toHaveValue("5432");
  await expect(dialog.getByRole("combobox", { name: "Project" })).toHaveText(
    "ledger",
  );
  await dialog.getByLabel("Local port or range").fill("15432");
  await dialog.getByRole("button", { name: "Save forward" }).click();
  await expect(db).toContainText("localhost:15432");
  await expect(
    db.getByRole("switch", { name: "Forward port 5432 (db)" }),
  ).not.toBeChecked();
});

test("a reverse forward shares a port on this Mac with the server", async ({
  page,
}) => {
  await openPorts(page);
  await page.getByRole("button", { name: "New forward", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "New forward" });
  await dialog.getByLabel("Name").fill("Webhook receiver");
  await dialog
    .getByRole("radio", { name: "This Mac → server port", exact: true })
    .click();
  await expect(dialog.getByLabel(/^Listen address on the server/)).toHaveValue(
    "127.0.0.1",
  );
  await dialog.getByLabel("Local port or range").fill("4000");
  await dialog.getByLabel("Remote port or range").fill("9400");
  await dialog.getByRole("combobox", { name: "Project" }).click();
  await page.getByRole("option", { name: "Atlas", exact: true }).click();
  await dialog.getByRole("button", { name: "Save forward" }).click();

  const atlas = region(page, "Atlas");
  await expect(atlas.getByText("From this Mac", { exact: true })).toBeVisible();
  const receiver = row(page, "Webhook receiver");
  await expect(receiver).toContainText("This Mac's port 4000");
  await expect(receiver).toContainText("server:9400");
  // There is nothing on this Mac's side to open in a browser from here.
  await expect(
    receiver.getByRole("button", { name: /in browser/ }),
  ).toHaveCount(0);
  await receiver
    .getByRole("switch", { name: "Forward Webhook receiver" })
    .click();
  await expect(
    receiver.getByRole("switch", { name: "Forward Webhook receiver" }),
  ).toBeChecked();
  expect(await saved(page)).toEqual([
    expect.objectContaining({
      direction: "remote",
      localPort: 4000,
      remotePort: 9400,
      remoteHost: "127.0.0.1",
      project: "/home/ci/work/atlas",
    }),
  ]);
});

test("processes the server cannot, or can no longer, place say so", async ({
  page,
}) => {
  await openPorts(page);
  await page.getByRole("button", { name: "Expand Other services" }).click();
  const unknown = row(page, "port 8443");
  await expect(unknown).toContainText("Owner not reported");
  await expect(unknown).toContainText("all interfaces");
  const preview = row(page, "port 7070 (node · preview)");
  await expect(preview).toContainText(
    "folder deleted · /tmp/ci-build/0f3a/worktrees/preview · Node",
  );
  await expect(preview.locator("[title]").first()).toHaveAttribute(
    "title",
    /working directory was deleted while it kept running/,
  );
});

test("search and the forwarded filter narrow every project", async ({
  page,
}) => {
  await openPorts(page, {
    tunnels: [forward("cache", 6379, { project: "/srv/ledger" })],
  });
  const search = page.getByLabel("Filter ports");
  // Branches, kinds and project names are all searchable.
  await search.fill("feat/search");
  await expect(page.getByRole("listitem")).toHaveCount(2);
  await search.fill("postgres");
  await expect(page.getByRole("listitem")).toHaveCount(1);
  await expect(row(page, "port 5432 (db)")).toBeVisible();
  // A search opens collapsed groups, so a match is never hidden.
  await search.fill("sshd");
  await expect(row(page, "port 22 (SSH)")).toBeVisible();
  await search.fill("nothing-like-this");
  await expect(page.getByText(/No matches/)).toBeVisible();
  await search.fill("");
  await page.getByRole("radio", { name: "Forwarded", exact: true }).click();
  await expect(page.getByRole("listitem")).toHaveCount(1);
  await expect(row(page, "port 6379 (cache)")).toBeVisible();
});

test("the Ports view sorts every port in one table and keeps the choice", async ({
  page,
}) => {
  await openPorts(page, {
    tunnels: [forward("cache", 6379, { project: "/srv/ledger" })],
    states: {
      cache: { status: "connected", errorMessage: null, reconnectAttempt: 0 },
    },
  });
  await page.getByRole("radio", { name: "Ports view" }).click();
  const table = page.getByRole("table", { name: "Ports" });
  const order = () =>
    table
      .getByRole("row")
      .evaluateAll((rows) =>
        rows.slice(1).map((r) => r.getAttribute("aria-label")),
      );
  expect(await order()).toEqual([
    "port 22 (SSH)",
    "port 3030 (Next.js)",
    "port 4173 (Vite)",
    "port 4174 (Vite)",
    "port 5432 (db)",
    "port 6379 (cache)",
    "port 7070 (node · preview)",
    "port 8443",
    "port 9000 (Uvicorn)",
    "port 9001 (Uvicorn)",
  ]);
  const search = table.getByRole("row", { name: "port 4174 (Vite)" });
  await expect(search).toContainText("Atlas");
  await expect(search).toContainText("feat/search");
  await expect(table.getByRole("row", { name: "port 22 (SSH)" })).toContainText(
    "Other",
  );
  // Status sorts forwarded rows first on its first click.
  await table.getByRole("button", { name: "Status", exact: true }).click();
  await expect(
    table.getByRole("columnheader", { name: "Status" }),
  ).toHaveAttribute("aria-sort", "ascending");
  expect((await order())[0]).toBe("port 6379 (cache)");
  await table.getByRole("button", { name: "Service", exact: true }).click();
  expect((await order()).slice(0, 2)).toEqual([
    "port 6379 (cache)",
    "port 5432 (db)",
  ]);
  // The same switch forwards from the table.
  await search
    .getByRole("switch", { name: "Forward port 4174 (Vite)" })
    .click();
  await expect(
    search.getByRole("cell", { name: "Connected", exact: true }),
  ).toBeVisible();
  // The view and its sort survive leaving the page.
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  await expect(
    table.getByRole("columnheader", { name: "Service" }),
  ).toHaveAttribute("aria-sort", "ascending");
  await page.getByRole("radio", { name: "Projects view" }).click();
  await expect(table).toHaveCount(0);
});

test("both views fit the minimum window without sideways scrolling", async ({
  page,
}) => {
  await openPorts(page, {
    tunnels: [forward("Vite", 4173, { project: "/home/ci/work/atlas" })],
    states: {
      Vite: { status: "connected", errorMessage: null, reconnectAttempt: 0 },
    },
  });
  await page.setViewportSize({ width: 640, height: 480 });
  const fits = () =>
    page.evaluate(() => document.documentElement.scrollWidth <= innerWidth);
  expect(await fits()).toBe(true);
  await expect(
    page.getByRole("button", { name: "Open port 4173 (Vite) in browser" }),
  ).toBeInViewport();
  await page.getByRole("radio", { name: "Ports view" }).click();
  expect(await fits()).toBe(true);
  // The address column folds under the service name.
  await expect(
    page.getByRole("button", { name: "Open port 4173 (Vite) in browser" }),
  ).toBeVisible();
});

test("discovery refreshes while the page is open and recovers from failure", async ({
  page,
}) => {
  await page.clock.install();
  await openPorts(page, { ports: new Error("SSH unavailable") });
  await expect(page.getByRole("alert")).toContainText(
    "Could not refresh ports: SSH unavailable",
  );
  await page.evaluate((ports) => {
    (window as any).__ports.ports = ports;
  }, listeners);
  await page
    .getByRole("button", { name: "Discover ports", exact: true })
    .click();
  await expect(row(page, "port 4173 (Vite)")).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  const polls = () => page.evaluate(() => (window as any).__ports.discoveries);
  const before = await polls();
  await page.clock.fastForward(31_000);
  await expect.poll(polls).toBe(before + 1);
  // Discovery pauses once the page is left.
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await page.clock.fastForward(31_000);
  expect(await polls()).toBe(before + 1);
});

test("an idle server says there is nothing to forward yet", async ({
  page,
}) => {
  await openPorts(page, { ports: [] });
  await expect(page.getByText("Nothing to forward yet")).toBeVisible();
  await expect(
    page.getByRole("button", { name: "New forward", exact: true }),
  ).toBeVisible();
});

test("a refused connection is reported and the switch can try again", async ({
  page,
}) => {
  await openPorts(page, {
    tunnels: [forward("App", 4173, { project: "/home/ci/work/atlas" })],
  });
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    (window as any).__refuse = true;
    bridge.invoke = (cmd: string, args: any) =>
      cmd === "set_tunnel_connected" && (window as any).__refuse
        ? Promise.reject(new Error("SSH connection was refused"))
        : original(cmd, args);
  });
  const toggle = page.getByRole("switch", { name: "Forward port 4173 (Vite)" });
  await toggle.click();
  await expect(page.getByRole("alert")).toContainText(
    "SSH connection was refused",
  );
  await expect(toggle).not.toBeChecked();
  await page.evaluate(() => ((window as any).__refuse = false));
  await toggle.click();
  await expect(toggle).toBeChecked();
  await expect(page.getByRole("alert")).toHaveCount(0);
});
