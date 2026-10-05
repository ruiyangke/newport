import { test, expect, type Page } from "@playwright/test";
import { installAppFixture } from "./fixtures/appFixture";

/**
 * Services on the invented fixture host: two Atlas units in trouble, a few
 * of the administrator's own services, a user service, two timers and the
 * usual operating-system plumbing.
 */
async function openServices(
  page: Page,
  fixture?: string,
  {
    delays = {},
    wait = true,
  }: { delays?: Record<string, number>; wait?: boolean } = {},
) {
  await installAppFixture(page);
  // Tests slow a command down by setting window.__delays[cmd] = ms.
  await page.addInitScript((delays) => {
    (window as any).__delays = delays;
    const wait = () => {
      const bridge = (window as any).__TAURI_INTERNALS__;
      if (!bridge?.invoke) return setTimeout(wait, 0);
      const original = bridge.invoke;
      bridge.invoke = async (cmd: string, args: any) => {
        const ms = (window as any).__delays?.[cmd];
        if (ms) await new Promise((resolve) => setTimeout(resolve, ms));
        return original(cmd, args);
      };
    };
    wait();
  }, delays);
  await page.goto(fixture ? `/?fixture=${fixture}` : "/");
  await page.getByRole("tab", { name: "Services", exact: true }).click();
  if (wait)
    await expect(
      page.getByRole("navigation", { name: "Services" }),
    ).toBeVisible();
}
const list = (page: Page, name: string) =>
  page.getByRole("list", { name, exact: true });
const detail = (page: Page, name: string) =>
  page.getByRole("article", { name, exact: true });
const actions = (page: Page) =>
  page.evaluate(() => (window as any).__services.actions as unknown[]);

test("problems lead, the person's own services follow, plumbing folds away", async ({
  page,
}) => {
  await openServices(page);
  await expect(
    page.getByText("8 running · 1 failed · 1 restarting · 2 timers"),
  ).toBeVisible();
  await expect(list(page, "Problems").getByRole("button")).toHaveText([
    /atlas-api.*exit 1/,
    /atlas-worker.*restart ×14/,
  ]);
  await expect(list(page, "Services").getByRole("button")).toHaveText([
    /dev-tunnel.*user/,
    /nginx.*2d/,
    /postgresql@16-main.*9d/,
    /redis-server.*2d/,
  ]);
  await expect(list(page, "Timers").getByRole("button")).toHaveText([
    /certbot.*in (3h|2h 59m)/,
    /apt-daily.*in (14h|13h 59m)/,
  ]);
  // The OS's own services are counted but closed until asked for.
  await expect(list(page, "System")).toHaveCount(0);
  await page.getByRole("button", { name: /^System\s*6$/ }).click();
  await expect(list(page, "System").getByRole("button")).toHaveCount(6);

  // Searching opens everything that matches, plumbing included.
  await page.getByRole("button", { name: /^System\s*6$/ }).click();
  await page.getByLabel("Find a service").fill("cert");
  await expect(list(page, "System").getByRole("button")).toHaveText([
    /certbot/,
  ]);
  await expect(list(page, "Timers").getByRole("button")).toHaveText([
    /certbot/,
  ]);
  await expect(list(page, "Services")).toHaveCount(0);
});

test("a failed service explains itself and highlights its errors", async ({
  page,
}) => {
  await openServices(page);
  // The first problem is open without a click.
  const api = detail(page, "atlas-api.service");
  await expect(api.getByRole("heading", { level: 2 })).toHaveText(
    "atlas-api.service",
  );
  await expect(api).toContainText("Failed");
  await expect(api).toContainText(
    "Exited with exit code 1. Restarted 3× so far.",
  );
  await expect(
    api.getByRole("button", { name: "Start", exact: true }),
  ).toBeVisible();
  await expect(
    api.getByRole("button", { name: "Restart", exact: true }),
  ).toBeVisible();

  const log = api.getByRole("log");
  await expect(log).toContainText("database connection refused");
  await expect(log).toContainText("Slow query took 1.8 s");
  // Live by default: new lines keep arriving after the first page.
  await expect(log).toContainText("live line 1", { timeout: 6_000 });
  await api.getByLabel("Follow new lines").click();
  await expect(api).toContainText("paused");

  await api.getByRole("radio", { name: "Errors" }).click();
  await expect(log).not.toContainText("Slow query");
  await expect(log).toContainText("database connection refused");
  await api.getByRole("radio", { name: "All" }).click();
  await api.getByLabel("Search logs").fill("listening");
  await expect(log.getByRole("listitem")).toHaveCount(1);
  await expect(api).toContainText("1 of");
});

test("actions run directly, and stopping or disabling asks first", async ({
  page,
}) => {
  await openServices(page);
  await list(page, "Services")
    .getByRole("button", { name: /^nginx\.service/ })
    .click();
  const nginx = detail(page, "nginx.service");
  await expect(nginx).toContainText("Running");
  await expect(nginx).toContainText("www-data");

  await nginx.getByRole("button", { name: "Restart", exact: true }).click();
  await expect(page.getByText("Restarted nginx")).toBeVisible();

  await nginx.getByRole("button", { name: "Stop", exact: true }).click();
  const confirm = page.getByRole("dialog", { name: "Stop nginx?" });
  await expect(confirm).toContainText("Anything relying on it stops working");
  await confirm.getByRole("button", { name: "Cancel" }).click();
  expect(await actions(page)).toHaveLength(1);

  await nginx.getByRole("button", { name: "Stop", exact: true }).click();
  await page
    .getByRole("dialog", { name: "Stop nginx?" })
    .getByRole("button", { name: "Stop", exact: true })
    .click();
  await expect(nginx).toContainText("Stopped");
  await expect(
    nginx.getByRole("button", { name: "Start", exact: true }),
  ).toBeVisible();
  await expect(
    list(page, "Services").getByRole("button", { name: /^nginx\.service/ }),
  ).toContainText("stopped");

  await nginx.getByRole("switch", { name: "Start at boot" }).click();
  await page
    .getByRole("dialog", { name: "Don’t start at boot nginx?" })
    .getByRole("button", { name: "Don’t start at boot" })
    .click();
  await expect(nginx).toContainText("Doesn’t start");
  await nginx.getByRole("switch", { name: "Start at boot" }).click();
  await expect(nginx).toContainText("Starts");

  expect(await actions(page)).toEqual([
    {
      id: expect.any(String),
      unit: "nginx.service",
      scope: "system",
      verb: "restart",
    },
    {
      id: expect.any(String),
      unit: "nginx.service",
      scope: "system",
      verb: "stop",
    },
    {
      id: expect.any(String),
      unit: "nginx.service",
      scope: "system",
      verb: "disable",
    },
    {
      id: expect.any(String),
      unit: "nginx.service",
      scope: "system",
      verb: "enable",
    },
  ]);
});

test("a sudo password hands the command to the Terminal, typed but not run", async ({
  page,
}) => {
  await openServices(page, "sudo-password");
  await expect(
    page.getByText(/System services need your sudo password/),
  ).toBeVisible();
  const api = detail(page, "atlas-api.service");
  await api.getByRole("button", { name: "Start", exact: true }).click();
  const handover = api.getByRole("alert");
  await expect(handover).toContainText(
    "sudo systemctl start atlas-api.service",
  );
  await handover.getByRole("button", { name: "Open in Terminal" }).click();

  await expect(
    page.getByRole("tab", { name: "Terminal", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect
    .poll(() =>
      page.evaluate(() =>
        new TextDecoder().decode(
          new Uint8Array((window as any).__terminalWrites.flat()),
        ),
      ),
    )
    .toContain("sudo systemctl start atlas-api.service");
  // Nothing presses Enter for the person: the password prompt is theirs.
  const typed = await page.evaluate(() =>
    (window as any).__terminalWrites.flat(),
  );
  expect(typed).not.toContain(13);
});

test("user services change without sudo and say whose they are", async ({
  page,
}) => {
  await openServices(page, "sudo-password");
  await list(page, "Services")
    .getByRole("button", { name: /^dev-tunnel\.service \(user\)/ })
    .click();
  const tunnel = detail(page, "dev-tunnel.service");
  await expect(tunnel).toContainText("user service");
  await expect(tunnel).toContainText("Runs as");
  await expect(tunnel).toContainText("you");
  await tunnel.getByRole("button", { name: "Restart", exact: true }).click();
  await expect(page.getByText("Restarted dev-tunnel")).toBeVisible();
  expect(await actions(page)).toEqual([
    expect.objectContaining({ scope: "user", verb: "restart" }),
  ]);
});

test("timers show their schedule and lead to the service they run", async ({
  page,
}) => {
  await openServices(page);
  await list(page, "Timers")
    .getByRole("button", { name: /^certbot\.timer/ })
    .click();
  const timer = detail(page, "certbot.timer");
  await expect(timer).toContainText("in 3h");
  await expect(timer).toContainText("9h ago");
  await expect(timer.getByRole("log")).toContainText("Starting service…");

  await timer
    .getByRole("button", { name: "certbot.service · stopped" })
    .click();
  const service = detail(page, "certbot.service");
  await expect(service).toContainText("Started by another unit");
  await expect(service).toContainText("Triggered by");
  await service.getByRole("button", { name: /certbot timer · next/ }).click();
  await expect(detail(page, "certbot.timer")).toBeVisible();
});

test("the unit file opens read-only, and the journal covers everything", async ({
  page,
}) => {
  await openServices(page);
  const api = detail(page, "atlas-api.service");
  await api.getByRole("tab", { name: "Unit file" }).click();
  await expect(api.getByText("Restart=on-failure")).toBeVisible();
  await expect(api).toContainText(
    "/etc/systemd/system/atlas-api.service + 1 drop-in",
  );
  // The tab is remembered for the next unit.
  await page.getByRole("button", { name: /^nginx\.service/ }).click();
  await expect(
    detail(page, "nginx.service").getByRole("tab", { name: "Unit file" }),
  ).toHaveAttribute("aria-selected", "true");

  await page.getByRole("button", { name: "System journal" }).click();
  const journal = detail(page, "System journal");
  await expect(journal.getByRole("log")).toContainText(
    "database connection refused",
  );
});

test("narrow windows show the list, then one service with a way back", async ({
  page,
}) => {
  await page.setViewportSize({ width: 760, height: 760 });
  await openServices(page);
  await page.getByRole("button", { name: /^redis-server\.service/ }).click();
  await expect(detail(page, "redis-server.service")).toBeVisible();
  await expect(page.getByLabel("Find a service")).toBeHidden();
  await page.getByRole("button", { name: "All services" }).click();
  await expect(page.getByLabel("Find a service")).toBeVisible();
});

const delay = (page: Page, delays: Record<string, number>) =>
  page.evaluate((delays) => ((window as any).__delays = delays), delays);

test("a slow server shows the page's shape, and polls never look busy", async ({
  page,
}) => {
  await openServices(page, undefined, {
    delays: { services_snapshot: 1500 },
  });
  const refresh = page.getByRole("button", { name: /^Refresh/ });
  // A 5 s poll is in flight for 1.5 s of every cycle; the button stays still.
  for (let i = 0; i < 14; i++) {
    await expect(refresh).toHaveText("Refresh");
    await page.waitForTimeout(500);
  }
  await refresh.click();
  await expect(refresh).toHaveText("Refreshing…");
  await expect(refresh).toHaveText("Refresh");
});

test("the first reading shows skeletons in place of the list and detail", async ({
  page,
}) => {
  await openServices(page, undefined, {
    delays: { services_snapshot: 60_000 },
    wait: false,
  });
  await expect(
    page.getByRole("list", { name: "Reading services" }),
  ).toBeVisible();
  await expect(page.getByText("Reading services…")).toBeVisible();
  await expect(page.getByLabel("Find a service")).toBeVisible();
});

test("arrow keys walk the list and the detail follows", async ({ page }) => {
  await openServices(page);
  const first = page.getByRole("button", { name: /^atlas-api\.service:/ });
  await first.focus();
  await page.keyboard.press("ArrowDown");
  await expect(detail(page, "atlas-worker.service")).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^atlas-worker\.service:/ }),
  ).toBeFocused();
  await page.keyboard.press("End");
  await expect(detail(page, "apt-daily.timer")).toBeVisible();
  await page.keyboard.press("Home");
  await expect(detail(page, "System journal")).toBeVisible();
  await page.keyboard.press("ArrowUp");
  await expect(detail(page, "System journal")).toBeVisible();
});

test("a choice far down a long list keeps both sides in view", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1200, height: 760 });
  await openServices(page, "large-lists");
  const row = page.getByRole("button", { name: /^worker-0003\.service:/ });
  await row.click();
  await page.keyboard.press("End");
  await expect(detail(page, "apt-daily.timer")).toBeInViewport();
  await expect(
    page.getByRole("button", { name: /^apt-daily\.timer:/ }),
  ).toBeInViewport();
  // The page itself never scrolls; each side does.
  expect(
    await page.evaluate(() => {
      const pane = document.querySelector("[data-slot=workspace-scroll]")!;
      return pane.scrollHeight <= pane.clientHeight;
    }),
  ).toBe(true);
});

test("logs follow the newest line, and wait below when scrolled up", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1200, height: 800 });
  await openServices(page, "long-logs");
  const api = detail(page, "atlas-api.service");
  const log = api.getByRole("log");
  await expect(log).toContainText("GET /health 200 (119)");
  const atBottom = () =>
    log.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight < 24);
  await expect.poll(atBottom).toBe(true);
  // A long journal fills the pane instead of stretching it.
  expect(
    await log.evaluate((el) => el.scrollHeight > el.clientHeight * 2),
  ).toBe(true);
  // Yesterday and today are divided.
  await expect(log.getByRole("separator")).toHaveCount(2);

  await log.evaluate((el) => (el.scrollTop = 0));
  const pill = api.getByRole("button", { name: /new lines?$/ });
  await expect(pill).toBeVisible({ timeout: 6_000 });
  await expect(api).toContainText("scrolled up; new lines wait below");
  expect(await log.evaluate((el) => el.scrollTop)).toBeLessThan(40);
  await pill.click();
  await expect.poll(atBottom).toBe(true);
  await expect(pill).toBeHidden();
  await expect(api).toContainText("following new lines");
});

test("changing the level keeps the lines on screen until the new ones arrive", async ({
  page,
}) => {
  await openServices(page);
  const api = detail(page, "atlas-api.service");
  const log = api.getByRole("log");
  await expect(log).toContainText("Slow query took 1.8 s");
  await delay(page, { service_logs: 1500 });
  await api.getByRole("radio", { name: "Errors" }).click();
  await expect(api.getByText("Filtering…")).toBeVisible();
  await expect(log).toContainText("Slow query took 1.8 s");
  await expect(log).toHaveAttribute("aria-busy", "true");
  await expect(log).not.toContainText("Slow query took 1.8 s");
  await expect(log).toHaveAttribute("aria-busy", "false");
});

test("an action in progress says what is happening", async ({ page }) => {
  await openServices(page);
  await page.getByRole("button", { name: /^nginx\.service/ }).click();
  const nginx = detail(page, "nginx.service");
  await delay(page, { service_action: 1500 });
  await nginx.getByRole("button", { name: "Restart", exact: true }).click();
  await expect(nginx.getByText("Restarting…")).toBeVisible();
  await expect(page.getByText("Restarted nginx")).toBeVisible();
  await expect(nginx.getByText("Restarting…")).toBeHidden();
  await expect(nginx).toContainText("Running");
});
