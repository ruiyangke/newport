import { test, expect, type Page } from "@playwright/test";
import { installAppFixture } from "./fixtures/appFixture";

const GiB = 1024 ** 3;
const MiB = 1024 ** 2;

/**
 * Opens Overview on the history fixture, with `patch` applied to every live
 * reading. Values are invented: a strained 8-core host with a nearly full
 * data disk, or a container host with many virtual interfaces.
 */
async function openOverview(page: Page, patch: Record<string, unknown> = {}) {
  await installAppFixture(page);
  await page.addInitScript((patch) => {
    const install = () => {
      const bridge = (window as any).__TAURI_INTERNALS__;
      if (!bridge?.invoke) return setTimeout(install, 0);
      const original = bridge.invoke;
      bridge.invoke = async (cmd: string, args: any) => {
        const result = await original(cmd, args);
        return cmd === "cockpit_collect" &&
          args.section === "overview" &&
          result
          ? { ...result, ...patch }
          : result;
      };
    };
    install();
  }, patch);
  await page.goto("/?fixture=history");
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "CPU", exact: true }),
  ).toBeVisible();
}

const strained = {
  cores: 8,
  load: [11.6, 9.8, 7.2],
  memoryTotal: 16 * GiB,
  memoryUsed: 15.4 * GiB,
  swapTotal: 2 * GiB,
  swapUsed: 0.2 * GiB,
  disks: [
    {
      mount: "/",
      device: "/dev/sda1",
      total: 100 * GiB,
      used: 42 * GiB,
      available: 58 * GiB,
    },
    {
      mount: "/srv",
      device: "/dev/sdb1",
      total: 1000 * GiB,
      used: 910 * GiB,
      available: 90 * GiB,
    },
    {
      mount: "/boot",
      device: "/dev/sda2",
      total: 1 * GiB,
      used: 0.2 * GiB,
      available: 0.8 * GiB,
    },
  ],
};

test("a comfortable server flags nothing", async ({ page }) => {
  await openOverview(page);
  for (const name of ["CPU", "Memory", "Load", "Network"])
    await expect(
      page.getByRole("region", { name, exact: true }),
    ).not.toContainText(/High|Critical/);
  await expect(
    page.getByRole("region", { name: "Storage", exact: true }).locator("svg"),
  ).toHaveCount(0);
});

test("strained resources flag themselves in words, not color alone", async ({
  page,
}) => {
  await openOverview(page, strained);
  const memory = page.getByRole("region", { name: "Memory", exact: true });
  await expect(memory).toContainText("Critical");
  await expect(memory).toContainText("96%");
  await expect(memory).toContainText("15.4 GiB of 16.0 GiB");
  await expect(
    page.getByRole("region", { name: "Load", exact: true }),
  ).toContainText("High");
  await expect(
    page.getByRole("region", { name: "CPU", exact: true }),
  ).not.toContainText(/High|Critical/);
  // Storage reads fullest first; the full disk carries a warning mark.
  const storage = page.getByRole("region", { name: "Storage", exact: true });
  await expect(storage.getByRole("meter")).toHaveCount(3);
  await expect(storage.locator("li code")).toHaveText(["/srv", "/", "/boot"]);
  await expect(storage.locator("li").first().locator("svg")).toHaveCount(1);
});

test("every core gets its own graph of the last minute, like Task Manager", async ({
  page,
}) => {
  await openOverview(page);
  const cores = page.getByRole("region", { name: "CPU cores" });
  const cells = cores.getByRole("img", { name: /^Core \d+:/ });
  await expect(cells).toHaveCount(8);
  await expect(cores).toContainText("8 online · busiest 61% · last minute");
  await expect(cells.nth(4)).toHaveAccessibleName(
    /^Core 4: 61% now, peak \d+% in the last minute$/,
  );
  await expect(cells.nth(4)).toContainText("61%");
  // Each cell draws its minute as one filled line, not a progress bar.
  await expect(cells.nth(4).locator("svg path")).toHaveCount(2);
  await expect(cores.getByRole("meter")).toHaveCount(0);
});

test("cores are labelled by kernel id, so an offline core leaves a gap", async ({
  page,
}) => {
  await openOverview(page, {
    cpus: [
      { id: 0, usage: 10 },
      { id: 2, usage: 90 },
    ],
  });
  await expect(
    page
      .getByRole("region", { name: "CPU cores" })
      .locator("li > span:first-of-type"),
  ).toHaveText(["0", "2"]);
  // Without history yet, each core still reads as a filled area, not a line.
  await expect(
    page.getByRole("region", { name: "CPU cores" }).locator("svg path"),
  ).toHaveCount(4);
});

test("an older agent without per-core data shows no cores panel", async ({
  page,
}) => {
  await openOverview(page, { cpus: null });
  await expect(page.getByRole("region", { name: "CPU cores" })).toHaveCount(0);
  await expect(
    page.getByRole("region", { name: "CPU", exact: true }),
  ).toBeVisible();
});

test("load reads against the core count, in the tile and the chart", async ({
  page,
}) => {
  await openOverview(page, strained);
  const load = page.getByRole("region", { name: "Load", exact: true });
  await expect(load).toContainText("145%");
  await expect(load).toContainText("of 8 cores · load 11.60");
  await load.getByRole("button", { name: "Show load history" }).click();
  await expect(
    load.getByRole("button", { name: "Show load history" }),
  ).toHaveAttribute("aria-pressed", "true");
  const chart = page.getByRole("region", { name: "Load history" });
  await expect(
    chart.getByRole("img", { name: /^Load, 1 minute history/ }),
  ).toBeVisible();
  // The core count is drawn as a labelled line, so a light load looks light.
  await expect(chart.getByText("8 cores", { exact: true })).toBeVisible();
});

test("tiles choose the large chart, and the window applies to all of them", async ({
  page,
}) => {
  await openOverview(page);
  await expect(page.getByRole("region", { name: "CPU history" })).toBeVisible();
  await page.getByRole("button", { name: "Show memory history" }).click();
  await expect(
    page.getByRole("img", { name: /^Memory history/ }),
  ).toBeVisible();
  await page.getByRole("radio", { name: "Last hour" }).click();
  await expect(
    page.getByRole("region", { name: "Memory history" }),
  ).toContainText("last hour");
  // The choice survives leaving the page.
  await page.getByRole("tab", { name: "Services", exact: true }).click();
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Show memory history" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("radio", { name: "Last hour" })).toHaveAttribute(
    "aria-checked",
    "true",
  );
});

test("container hosts fold their virtual interfaces away", async ({ page }) => {
  await openOverview(page, {
    network: [
      { name: "lo", received: 900 * GiB, sent: 900 * GiB },
      { name: "docker0", received: 400 * GiB, sent: 120 * GiB },
      { name: "ens3", received: 250 * GiB, sent: 140 * GiB },
      { name: "wg0", received: 2 * GiB, sent: 1 * GiB },
      ...Array.from({ length: 12 }, (_, i) => ({
        name: i % 4 ? `veth${1000 + i}` : `br-${(0xabc000 + i).toString(16)}`,
        received: (i + 1) * 10 * MiB,
        sent: (i + 1) * 5 * MiB,
      })),
    ],
  });
  const totals = page.getByRole("table", { name: "Network totals" });
  const names = () =>
    totals.locator("tbody tr td:first-child").allTextContents();
  expect(await names()).toEqual(["ens3", "wg0", "14 virtual and loopback"]);
  await totals.getByRole("button", { name: "14 virtual and loopback" }).click();
  expect(await names()).toEqual(["ens3", "wg0", "14 virtual and loopback"]);
  // Opened, the links flow into columns across the whole row.
  const links = page.getByRole("list", {
    name: "Virtual and loopback interfaces",
  });
  await expect(links.getByRole("listitem")).toHaveCount(14);
  // Busiest first: loopback and the Docker bridge lead the folded links.
  await expect(links.getByRole("listitem").nth(0)).toContainText("lo");
  await expect(links.getByRole("listitem").nth(1)).toContainText("docker0");
  const section = await page
    .getByRole("region", { name: "Network totals", exact: true })
    .boundingBox();
  const storage = await page
    .getByRole("region", { name: "Storage", exact: true })
    .boundingBox();
  expect(section!.width).toBeCloseTo(storage!.width, 0);
  expect(section!.y).toBeGreaterThan(storage!.y + storage!.height);
});

test("the page fits the minimum window", async ({ page }) => {
  await openOverview(page, strained);
  await page.setViewportSize({ width: 640, height: 480 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await expect(
    page.getByRole("region", { name: "Memory", exact: true }),
  ).toBeInViewport();
});

test("the footnote says who collected the values", async ({ page }) => {
  await openOverview(page);
  await expect(page.locator("[data-slot=cockpit-footnote]")).toHaveText(
    "Collected by the Newport agent on the server. Monitoring is read-only.",
  );
  // Recent per-process CPU is described as such, not as a lifetime average.
  await expect(
    page.getByRole("region", { name: "Processes", exact: true }),
  ).toContainText("CPU over the last second, as a share of one core");
});

test("a server the agent cannot run on says why, instead of showing nothing", async ({
  page,
}) => {
  await installAppFixture(page);
  await page.addInitScript(() => {
    const install = () => {
      const bridge = (window as any).__TAURI_INTERNALS__;
      if (!bridge?.invoke) return setTimeout(install, 0);
      const original = bridge.invoke;
      bridge.invoke = (cmd: string, args: any) =>
        cmd === "cockpit_collect" && args.section === "overview"
          ? Promise.reject(
              "Monitoring needs the Newport agent, which can’t run on this server: The Newport agent requires Linux.",
            )
          : original(cmd, args);
    };
    install();
  });
  await page.goto("/");
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Monitoring needs the Newport agent, which can’t run on this server: The Newport agent requires Linux.",
  );
});

test("storage and network totals each take a full row", async ({ page }) => {
  await openOverview(page);
  const storage = await page
    .getByRole("region", { name: "Storage", exact: true })
    .boundingBox();
  const network = await page
    .getByRole("region", { name: "Network totals", exact: true })
    .boundingBox();
  expect(network!.width).toBeCloseTo(storage!.width, 0);
  expect(network!.y).toBeGreaterThan(storage!.y + storage!.height);
});

// Invented processes: a busy build, an idle but large database, a stuck
// writer, a zombie, a kernel thread, and enough workers to need "Show all".
const processes = [
  {
    pid: 4100,
    name: "cc1plus",
    user: "ci",
    cpu: 186.4,
    memory: 1.2 * GiB,
    state: "R",
    command: "/usr/lib/gcc/x86_64-linux-gnu/14/cc1plus -O2 engine.cpp",
  },
  {
    pid: 900,
    name: "postgres",
    user: "postgres",
    cpu: 0.4,
    memory: 6.4 * GiB,
    state: "S",
    command: "postgres: main: checkpointer",
  },
  {
    pid: 4200,
    name: "rsync",
    user: "ci",
    cpu: 2.1,
    memory: 40 * MiB,
    state: "D",
    command: "rsync -a /srv/cache/ /mnt/backup/",
  },
  { pid: 4300, name: "defunct", user: "ci", cpu: 0, memory: 0, state: "Z" },
  {
    pid: 12,
    name: "kworker/3:1",
    user: "root",
    cpu: 1.5,
    memory: 0,
    state: "I",
  },
  ...Array.from({ length: 24 }, (_, i) => ({
    pid: 5000 + i,
    name: "worker",
    user: "ci",
    cpu: 1 - i / 100,
    memory: (100 - i) * MiB,
    state: "S",
    command: `/opt/jobs/worker --shard ${i}`,
  })),
];

test("processes show what runs, sort by any column, and fold the long tail", async ({
  page,
}) => {
  await openOverview(page, { processes, processCount: 412 });
  const section = page.getByRole("region", { name: "Processes", exact: true });
  const table = section.getByRole("table", { name: "Processes" });
  const first = () => table.locator("tbody tr").first().locator("td").first();
  await expect(section).toContainText(
    "The busiest and largest 29 of 412 · CPU over the last second, as a share of one core",
  );
  // Busiest first by default, with its command line under the name.
  await expect(first()).toContainText("cc1plus");
  await expect(first()).toContainText("cc1plus -O2 engine.cpp");
  await expect(table.getByRole("row")).toHaveCount(21);
  // Memory puts the idle but large database on top.
  await table.getByRole("button", { name: "Memory", exact: true }).click();
  await expect(first()).toContainText("postgres");
  await expect(
    table.getByRole("columnheader", { name: "Memory" }),
  ).toHaveAttribute("aria-sort", "descending");
  await table.getByRole("button", { name: "Process", exact: true }).click();
  await expect(first()).toContainText("cc1plus");
  // States that need attention say so; sleeping ones say nothing.
  await table.getByRole("button", { name: "CPU", exact: true }).click();
  await section.getByRole("button", { name: "Show all 29" }).click();
  await expect(table.getByRole("row")).toHaveCount(30);
  await expect(
    table.getByRole("row").filter({ hasText: "rsync" }),
  ).toContainText("Waiting on I/O");
  await expect(
    table.getByRole("row").filter({ hasText: "defunct" }),
  ).toContainText("Zombie");
  await expect(
    table.getByRole("row").filter({ hasText: "kworker/3:1" }),
  ).toContainText("Kernel thread");
  await expect(
    table.getByRole("row").filter({ hasText: "postgres" }),
  ).not.toContainText(/Waiting|Zombie|Stopped/);
  // Search reaches command lines.
  await section.getByLabel("Filter processes").fill("/mnt/backup");
  await expect(table.locator("tbody tr")).toHaveCount(1);
  await expect(section).toContainText("Showing 1 of 29 collected processes");
});
