import { test, expect } from "@playwright/test";
const serverId = "a3531c9e-d53d-45ae-990c-fbe204d1a21e";
const tunnelId = "4a0429c7-7092-4c9d-ae8b-b1764985fb52";
test.beforeEach(async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.addInitScript(
    ({ serverId, tunnelId }) => {
      let metricCalls = 0;
      const terminals = new Map<string, any[]>();
      (window as any).__terminalWrites = [];
      (window as any).__terminalClosed = [];
      (window as any).__terminalSizes = [];

      let largeServiceCalls = 0;
      const fixtureAt = Date.now();
      const data = {
        config: {
          servers: [
            {
              id: serverId,
              name: "Development",
              sshUser: "developer",
              sshHost: "dev.example.com",
              sshPort: 22,
              identityFile: null,
              authMethod: "publicKey",
              browserEnabled:
                sessionStorage.getItem("fixture-browser-enabled") === "true",
              clipboardEnabled:
                sessionStorage.getItem("fixture-clipboard-enabled") === "true",
            },
          ],
          tunnels: [
            {
              id: tunnelId,
              serverId,
              name: "Web application",
              localPort: 3000,
              localPortEnd: null,
              remoteHost: "127.0.0.1",
              remotePort: 3000,
              remotePortEnd: null,
              autoConnect: false,
              autoReconnect: true,
            },
          ],
        },
        runtime: {
          tunnels: {} as Record<string, unknown>,
          clipboard: {} as Record<string, unknown>,
          clipboardMessages: {} as Record<string, string>,
          clipboardPathNeeded: {} as Record<string, boolean>,
          health: { [serverId]: "reachable" },
        },
        loadError: null,
      };
      if (location.search.includes("fixture=server-picker")) {
        const original = data.config.servers[0];
        data.config.servers.push(
          {
            ...original,
            id: "same-host-other-user",
            sshUser: "operator",
            sshPort: 2222,
          },
          ...Array.from({ length: 35 }, (_, i) => ({
            ...original,
            id: `picker-${i}`,
            name:
              i === 0
                ? "Production analytics and reporting cluster in the western region"
                : `Worker ${i}`,
            sshHost:
              i === 0
                ? "analytics-primary.production.internal.example.com"
                : `worker-${i}.example.com`,
          })),
        );
      }
      Object.defineProperty(window, "isTauri", { value: true });
      Object.defineProperty(window, "__TAURI_INTERNALS__", {
        value: {
          invoke: async (cmd: string, args: Record<string, any>) => {
            if (cmd === "update_status")
              return {
                enabled: true,
                currentVersion: "0.2.0",
                phase: "idle",
                version: null,
                downloaded: 0,
                total: null,
                error: null,
              };
            if (cmd === "get_startup_settings")
              return { enabled: false, available: true };
            if (cmd === "get_metrics_cache")
              return { bytes: 65536, samples: 0 };
            if (cmd === "clear_metrics_cache")
              return { bytes: 65536, samples: 0 };
            if (cmd === "get_sidebar_width") {
              const saved = sessionStorage.getItem("fixture-sidebar-width");
              return saved === null ? null : Number(saved);
            }
            if (cmd === "set_sidebar_width") {
              sessionStorage.setItem(
                "fixture-sidebar-width",
                String(args.width),
              );
              return;
            }
            if (
              location.search.includes("fixture=large-lists") &&
              cmd === "cockpit_collect"
            ) {
              if (args.section === "services") {
                const offset = largeServiceCalls++ ? -1 : 0;
                return Array.from({ length: 500 }, (_, i) => ({
                  name: `worker-${String(i + offset + 1).padStart(4, "0")}.service`,
                  active: "active",
                  sub: "running",
                  load: "loaded",
                  description: `Background worker ${i + offset + 1}`,
                }));
              }
              if (args.section === "containers")
                return Array.from({ length: 500 }, (_, i) => ({
                  id: `container-${i}`,
                  name: `worker-${String(i).padStart(4, "0")}`,
                  image: "worker:latest",
                  state: "running",
                  status: "Up 2 hours",
                  ports: "3000/tcp",
                }));
            }
            if (
              location.search.includes("fixture=offline") ||
              location.search.includes("fixture=waiting")
            ) {
              if (cmd === "cockpit_history")
                return [0, 1].map((i) => ({
                  at: Date.now() - 90000 + i * 60000,
                  data: {
                    cpu: 15 + i,
                    memoryUsed: 4,
                    memoryTotal: 8,
                    load: [1, 2, 3],
                    uptime: 100 + i * 10,
                    network: [
                      {
                        name: "eth0",
                        received: 1000 + i * 1000,
                        sent: 500 + i * 500,
                      },
                    ],
                  },
                }));
              if (cmd === "cockpit_collect" && args.section === "overview")
                throw location.search.includes("fixture=waiting")
                  ? "Waiting for the background sampler’s first reading. Refresh shortly."
                  : new Error("Server is offline");
            }
            if (cmd === "ssh_agent_keys")
              return {
                keys: [
                  {
                    source: "onePassword",
                    comment: "Production deploy key",
                    algorithm: "ssh-ed25519",
                    fingerprint: "SHA256:" + "A".repeat(43),
                  },
                ],
                warnings: [],
              };
            if (cmd === "cockpit_history") {
              if (!location.search.includes("fixture=history")) return [];
              return Array.from({ length: 31 }, (_, i) => ({
                at: fixtureAt - (30 - i) * 10000,
                data: {
                  cpu: 18 + Math.sin(i / 3) * 12,
                  memoryUsed: 6120328396,
                  memoryTotal: 17179869184,
                  load: [0.72, 0.94, 1.12],
                  uptime: 923450,
                  network: [
                    {
                      name: "eth0",
                      received: 8492392843 + i * 250000,
                      sent: 2048239432,
                    },
                  ],
                },
              }));
            }
            if (
              cmd === "cockpit_collect" &&
              args.section === "overview" &&
              location.search.includes("fixture=ssh-timeout")
            ) {
              const attempts = (window as any).__manualMetricAttempts ?? 0;
              if (args.refresh)
                (window as any).__manualMetricAttempts = attempts + 1;
              if (!args.refresh || attempts === 0)
                throw new Error(
                  "SSH connection or authentication timed out after 20 seconds: deadline has elapsed",
                );
              await new Promise<void>((resolve) => {
                (window as any).__completeMetricRefresh = resolve;
              });
            }
            if (cmd === "cockpit_collect") {
              if (args.section === "overview") {
                metricCalls++;
                return {
                  sampledAt: location.search.includes("fixture=history")
                    ? fixtureAt
                    : undefined,
                  hostname: "dev-linux",
                  os: "Ubuntu 24.04 LTS",
                  kernel: "6.8.0-60-generic",
                  cores: 8,
                  uptime: 923450,
                  load: [0.72, 0.94, 1.12],
                  cpu:
                    18 +
                    Math.sin(
                      (location.search.includes("fixture=history")
                        ? 30
                        : metricCalls) / 3,
                    ) *
                      12,
                  memoryTotal: 17179869184,
                  memoryUsed: 6120328396,
                  swapTotal: 2147483648,
                  swapUsed: 0,
                  disks: [
                    {
                      mount: "/",
                      device: "/dev/sda1",
                      total: 107374182400,
                      used: 45097156608,
                      available: 56908316672,
                    },
                  ],
                  network: [
                    {
                      name: "eth0",
                      received: 8492392843 + metricCalls * 250000,
                      sent: 2048239432,
                    },
                  ],
                  processCount: 182,
                  processes: location.search.includes("fixture=large-lists")
                    ? Array.from({ length: 50 }, (_, i) => ({
                        pid: 1000 + i,
                        name: `worker-${i}`,
                        user: "developer",
                        cpu: 50 - i,
                        memory: 1024 * (i + 1),
                        state: "S",
                      }))
                    : [
                        {
                          pid: 821,
                          name: "node",
                          user: "developer",
                          cpu: 12.8,
                          memory: 428000000,
                          state: "S",
                        },
                        {
                          pid: 612,
                          name: "postgres",
                          user: "postgres",
                          cpu: 3.2,
                          memory: 216000000,
                          state: "S",
                        },
                        {
                          pid: 403,
                          name: "sshd",
                          user: "root",
                          cpu: 0,
                          memory: 12000000,
                          state: "S",
                        },
                      ],
                };
              }
              if (args.section === "services")
                return [
                  {
                    name: location.search.includes("fixture=long-lists")
                      ? "worker-" + "long-name-".repeat(16) + ".service"
                      : "worker.service",
                    active: "failed",
                    sub: "failed",
                    load: "loaded",
                    description: "Background job worker",
                  },
                  {
                    name: "ssh.service",
                    active: "active",
                    sub: "running",
                    load: "loaded",
                    description: "OpenBSD Secure Shell server",
                  },
                ];
              if (args.section === "containers")
                throw new Error(
                  "Docker is unavailable or this account cannot access its socket.",
                );
            }
            if (cmd === "cockpit_logs")
              return "2026-09-20T10:21:04 worker[902]: database connection refused\n2026-09-20T10:21:05 systemd[1]: worker.service: Failed with result 'exit-code'.";
            if (cmd === "terminal_open") {
              terminals.set(args.session, [
                { type: "ready" },
                {
                  type: "data",
                  data: Array.from(
                    new TextEncoder().encode(
                      "Welcome to Development\r\n\x1b[33mANSI yellow\x1b[0m · \x1b[97mbright white\x1b[0m · \x1b[36mcyan\x1b[0m\r\n\uf179 \uf115 ~ \uf017 01:15:17\r\ndeveloper@dev-linux:~$ ",
                    ),
                  ),
                },
              ]);
              return;
            }
            if (cmd === "terminal_read") {
              const events = terminals.get(args.session);
              if (!events) return { type: "exit", data: null };
              if (events.length) return events.shift();
              await new Promise((resolve) => setTimeout(resolve, 30));
              return null;
            }
            if (cmd === "terminal_write") {
              (window as any).__terminalWrites.push(args.data);
              terminals
                .get(args.session)
                ?.push({ type: "data", data: args.data });
              return;
            }
            if (cmd === "terminal_resize") {
              (window as any).__terminalSizes.push([args.cols, args.rows]);
              return;
            }
            if (cmd === "terminal_close") {
              (window as any).__terminalClosed.push(args.session);
              terminals.delete(args.session);
              return;
            }
            if (cmd === "snapshot") return structuredClone(data);
            if (cmd === "save_server") {
              const i = data.config.servers.findIndex(
                (s) => s.id === args.server.id,
              );
              if (i < 0) data.config.servers.push(args.server);
              else data.config.servers[i] = args.server;
            }
            if (cmd === "save_tunnel") {
              const i = data.config.tunnels.findIndex(
                (t) => t.id === args.tunnel.id,
              );
              if (i < 0) data.config.tunnels.push(args.tunnel);
              else data.config.tunnels[i] = args.tunnel;
            }
            if (cmd === "set_tunnel_connected")
              data.runtime.tunnels[args.id] = {
                status: args.connected ? "connected" : "disconnected",
                errorMessage: null,
                reconnectAttempt: 0,
              };
            if (cmd === "reinstall_agent") {
              (window as any).__reinstallCount =
                ((window as any).__reinstallCount ?? 0) + 1;
              await new Promise<void>((resolve, reject) => {
                (window as any).__finishReinstall = (fail = false) =>
                  fail ? reject(new Error("Agent upload failed")) : resolve();
              });
              return;
            }
            if (cmd === "set_integration_enabled") {
              const server = data.config.servers.find(
                (server) => server.id === args.id,
              );
              if (server) {
                if (args.feature === "browser")
                  server.browserEnabled = args.enabled;
                else server.clipboardEnabled = args.enabled;
              }
              sessionStorage.setItem(
                `fixture-${args.feature}-enabled`,
                String(args.enabled),
              );
              data.runtime.clipboard[args.id] = {
                status:
                  server?.clipboardEnabled || server?.browserEnabled
                    ? "connected"
                    : "disconnected",
                errorMessage: null,
                reconnectAttempt: 0,
              };
              data.runtime.clipboardMessages[args.id] =
                "X clipboard unavailable; using the file-backed clipboard.";
              data.runtime.clipboardPathNeeded[args.id] =
                !location.search.includes("fixture=shim-ready");
            }
            if (cmd === "delete_server") {
              data.config.servers = data.config.servers.filter(
                (s) => s.id !== args.id,
              );
              data.config.tunnels = data.config.tunnels.filter(
                (t) => t.serverId !== args.id,
              );
            }
            if (cmd === "delete_tunnel")
              data.config.tunnels = data.config.tunnels.filter(
                (t) => t.id !== args.id,
              );
            if (
              cmd === "discover_ports" &&
              location.search.includes("fixture=docker-ports")
            )
              return [
                {
                  port: 8080,
                  address: "::",
                  processName: null,
                  pid: null,
                  containerName: "api",
                },
                {
                  port: 22,
                  address: "0.0.0.0",
                  processName: null,
                  pid: null,
                  containerName: null,
                },
              ];
            if (
              cmd === "discover_ports" &&
              location.search.includes("fixture=node-ports")
            )
              return [
                {
                  port: 5174,
                  address: "0.0.0.0",
                  processName: "MainThread",
                  pid: 126945,
                  user: "ruiyang",
                  applicationName: "Vite · webcontainers-demo",
                  executable: "/nix/store/node/bin/node",
                  workingDirectory: "/home/ruiyang/Projects/webcontainers-demo",
                  command:
                    "node /home/ruiyang/Projects/webcontainers-demo/node_modules/.bin/vite --host 0.0.0.0",
                },
              ];
            if (cmd === "discover_ports")
              return [
                {
                  port: 5432,
                  address: "127.0.0.1",
                  processName: "postgres",
                  pid: 812,
                  executable: "/usr/lib/postgresql/bin/postgres",
                  workingDirectory: "/var/lib/postgresql",
                  command: "postgres -D /var/lib/postgresql",
                },
              ];
          },
        },
      });
    },
    { serverId, tunnelId },
  );
});
test("integration switches are independent and setup is copyable", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("tab", { name: "Integration", exact: true }).click();
  const section = page.locator(".integration-panel");
  const clipboard = section.getByRole("switch", {
    name: "Clipboard",
    exact: true,
  });
  const browser = section.getByRole("switch", { name: "Browser", exact: true });
  await expect(clipboard).not.toBeChecked();
  await expect(browser).not.toBeChecked();
  await page.screenshot({
    path: "test-results/screenshots/integration-off.png",
  });
  await browser.click();
  await expect(section.getByText("Connected", { exact: true })).toBeVisible();
  await expect(clipboard).not.toBeChecked();
  await expect(section.locator(".setup-code-block code")).toHaveText(
    'eval "$("$HOME/.local/bin/porthop-agent" env)"',
  );
  await page.evaluate(() =>
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (text: string) => {
          (window as any).__copiedSetup = text;
        },
      },
    }),
  );
  await section.getByRole("button", { name: "Copy", exact: true }).click();
  expect(await page.evaluate(() => (window as any).__copiedSetup)).toBe(
    'eval "$("$HOME/.local/bin/porthop-agent" env)"',
  );
  await clipboard.click();
  await browser.click();
  await expect(clipboard).toBeChecked();
  await expect(browser).not.toBeChecked();
  await expect(section.getByText("Connected", { exact: true })).toBeVisible();
  await page.screenshot({
    path: "test-results/screenshots/integration-on.png",
  });
  await page.setViewportSize({ width: 640, height: 480 });
  await page.screenshot({
    path: "test-results/screenshots/integration-small.png",
  });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await clipboard.click();
  await expect(section.getByText("Off", { exact: true })).toBeVisible();
  await expect(section.locator(".setup-code-block")).toHaveCount(0);
});

test("connect, disconnect, discover and add a forwarded port", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Choose server", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Disconnect", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Disconnect", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Connect", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Discover ports" }).click();
  await page.getByRole("button", { name: "Forward", exact: true }).click();
  await expect(page.getByLabel("Local port or range")).toHaveValue("5432");
  await page.getByRole("button", { name: "Save tunnel" }).click();
  await expect(
    page.getByRole("heading", { name: "postgres", exact: true }),
  ).toBeVisible();
  const remote = page.getByRole("table", { name: "Remote ports" });
  await expect(remote.getByText("Disconnected", { exact: true })).toBeVisible();
  await expect(
    remote.getByText("127.0.0.1:5432", { exact: true }),
  ).toBeVisible();
  await expect(
    remote.getByRole("button", { name: "Forward", exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("listitem")
    .filter({
      has: page.getByRole("heading", { name: "postgres", exact: true }),
    })
    .getByRole("button", { name: "Connect", exact: true })
    .click();
  await expect(remote.getByText("Connected", { exact: true })).toBeVisible();
  await remote
    .getByRole("button", { name: "Edit forward for port 5432" })
    .click();
  await expect(page.getByLabel("Local port or range")).toHaveValue("5432");
});
test("invalid ranges stay editable and server deletion cascades", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  await page
    .getByRole("button", { name: "Actions for tunnel Web application" })
    .click();
  await page
    .getByRole("menuitem", { name: "Edit Web application", exact: true })
    .click();
  await page.getByLabel("Local port or range").fill("9000-8999");
  await page.getByRole("button", { name: "Save tunnel" }).click();
  await expect(page.getByRole("alert")).toContainText("end of a range");
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await page
    .getByRole("button", { name: "Server actions", exact: true })
    .click();
  await page
    .getByRole("menuitem", { name: "Delete server", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Delete server", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Your servers, one hop away." }),
  ).toBeVisible();
});
test("desktop, minimum size and dark appearance", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Discover ports" }).click();
  await expect(
    page.getByRole("button", { name: "Forward", exact: true }),
  ).toBeVisible();
  await page.screenshot({ path: "test-results/screenshots/desktop.png" });
  await page.setViewportSize({ width: 640, height: 480 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({ path: "test-results/screenshots/compact.png" });
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Disconnect", exact: true }),
  ).toBeVisible();
  await page.emulateMedia({ colorScheme: "dark" });
  await page.setViewportSize({ width: 960, height: 680 });
  await page.screenshot({ path: "test-results/screenshots/dark.png" });
  await page.getByRole("button", { name: "Edit server", exact: true }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.screenshot({ path: "test-results/screenshots/server-form.png" });
});

test("cockpit health, service logs, capability errors and explicit commands", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto("/?fixture=history");
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await expect(
    page.getByRole("meter", { name: "CPU usage", exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("img", { name: /^CPU history/ })).toBeVisible();
  await expect(page.getByRole("img", { name: /^Network/ })).toHaveAttribute(
    "aria-label",
    /24.4 KiB\/s/,
  );
  await expect(
    page.getByRole("meter", { name: "Memory usage", exact: true }),
  ).toHaveAttribute("aria-valuenow", "36");
  await page.screenshot({
    path: "test-results/screenshots/gauges-desktop.png",
  });
  await page.setViewportSize({ width: 640, height: 480 });
  await expect(
    page.getByRole("heading", { name: "Storage · /", exact: true }),
  ).toBeInViewport();
  await expect(page.getByText("Reachable", { exact: true })).toBeInViewport();
  await page.screenshot({
    path: "test-results/screenshots/gauges-compact.png",
  });
  await page.setViewportSize({ width: 960, height: 680 });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.screenshot({ path: "test-results/screenshots/gauges-dark.png" });
  await page.emulateMedia({ colorScheme: "light" });
  await page.getByLabel("History time window").click();
  await page
    .getByRole("option", { name: "Last 15 minutes", exact: true })
    .click();
  await page.getByLabel("History time window").click();
  await page
    .getByRole("option", { name: "Last 5 minutes", exact: true })
    .click();
  await page
    .locator(".recharts-surface")
    .first()
    .hover({ position: { x: 180, y: 70 } });
  await page.clock.runFor(50);
  await expect(page.locator(".metric-tooltip").first()).toBeVisible();
  await page.screenshot({
    path: "test-results/screenshots/recharts-tooltip.png",
  });
  await page.mouse.move(10, 10);
  await page.screenshot({
    path: "test-results/screenshots/cockpit-desktop.png",
  });
  await page.getByText("View readings", { exact: true }).focus();
  await page.keyboard.press("Enter");
  await expect(
    page
      .getByRole("region", { name: "Recorded metric samples" })
      .getByRole("row"),
  ).toHaveCount(32);
  await page.keyboard.press("Enter");
  await page.getByText("System details", { exact: true }).click();
  await expect(
    page.getByText("Ubuntu 24.04 LTS", { exact: true }),
  ).toBeVisible();
  await page.getByText("System details", { exact: true }).click();
  await page.getByLabel("Filter processes").fill("postgres");
  await expect(
    page.getByRole("cell", { name: "node", exact: true }),
  ).toHaveCount(0);
  await expect(
    page
      .getByRole("table", { name: "Processes", exact: true })
      .locator(".row-name")
      .filter({ hasText: /^postgres$/ }),
  ).toBeVisible();
  await page.getByRole("tab", { name: "Services", exact: true }).click();
  await expect(page.getByText("worker.service", { exact: true })).toBeVisible();
  await page.getByLabel("Filter services").fill("worker");
  await page.screenshot({
    path: "test-results/screenshots/cockpit-services.png",
  });
  await page
    .getByRole("button", { name: "View logs for worker.service" })
    .click();
  await expect(page.getByLabel("Log output")).toContainText(
    "database connection refused",
  );
  await page.getByRole("button", { name: "Close logs" }).click();
  await expect(page.getByLabel("Filter services")).toHaveValue("worker");
  await expect(
    page.getByRole("button", { name: "View logs for worker.service" }),
  ).toBeFocused();
  await page.getByRole("tab", { name: "Containers", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Docker is unavailable");
  await page.getByRole("tab", { name: "Commands", exact: true }).click();
  await page
    .getByRole("button", { name: "Connect terminal", exact: true })
    .click();
  await expect(page.locator(".terminal-status")).toHaveText("Connected");
  await page.screenshot({
    path: "test-results/screenshots/cockpit-commands.png",
  });
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await expect(
    page.getByRole("meter", { name: "CPU usage", exact: true }),
  ).toBeVisible();
  await page.setViewportSize({ width: 640, height: 480 });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: "test-results/screenshots/cockpit-compact.png",
  });
  await page
    .getByRole("img", { name: /^CPU history/ })
    .scrollIntoViewIfNeeded();
  await page.screenshot({
    path: "test-results/screenshots/graphs-compact.png",
  });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.setViewportSize({ width: 960, height: 680 });
  await page.screenshot({ path: "test-results/screenshots/cockpit-dark.png" });
  await page.getByLabel("Graph network interface").scrollIntoViewIfNeeded();
  await page.getByRole("img", { name: /^Network/ }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: "test-results/screenshots/graphs-dark.png" });
  await page.emulateMedia({ colorScheme: "light" });
  await page.screenshot({
    path: "test-results/screenshots/graphs-desktop.png",
  });
});

test("overview waiting state keeps a compact header and saved history", async ({
  page,
}) => {
  await page.goto("/?fixture=waiting");
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await expect(
    page.getByText("Waiting for first reading…", { exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(
    page.getByRole("img", { name: /^CPU history, 2 samples/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("checkbox", { name: "Auto-refresh", exact: true }),
  ).toBeChecked();
  for (const width of [960, 640]) {
    await page.setViewportSize({ width, height: 680 });
    const header = await page.locator(".overview-masthead").boundingBox();
    expect(header!.height).toBeLessThan(110);
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    await page.screenshot({
      path: `test-results/screenshots/overview-waiting-${width}.png`,
    });
  }
});

test("saved metric history remains visible when the server is offline", async ({
  page,
}) => {
  await page.goto("/?fixture=offline");
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Server is offline");
  await expect(
    page.getByRole("img", { name: /^CPU history, 2 samples/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("img", { name: /^Memory history/ }),
  ).toHaveAttribute("aria-label", /50.0%/);
  await expect(
    page.locator(".metric-chart").first().locator(".isolated-metric-point"),
  ).toHaveCount(2);
  await page.locator(".metric-chart").first().scrollIntoViewIfNeeded();
  await page.screenshot({ path: "test-results/screenshots/recharts-gaps.png" });
});

test("Compose projects, logs and confirmed lifecycle controls", async ({
  page,
}) => {
  await page.goto("/");
  await expect(
    page.getByRole("button", { name: "Choose server", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    const containers = [
      {
        id: "a".repeat(64),
        name: "shop-api-1",
        image: "shop-api:latest",
        state: "running",
        status: "Up 2 hours",
        ports: "127.0.0.1:3000->3000/tcp",
        composeProject: "shop",
        composeService: "api",
        composeOneoff: "False",
      },
      {
        id: "b".repeat(64),
        name: "shop-db-1",
        image: "postgres:16",
        state: "running",
        status: "Up 2 hours (healthy)",
        ports: "5432/tcp",
        composeProject: "shop",
        composeService: "db",
        composeOneoff: "False",
      },
      {
        id: "c".repeat(64),
        name: "shop-migrate-run-1",
        image: "shop-api:latest",
        state: "exited",
        status: "Exited (0)",
        ports: "",
        composeProject: "shop",
        composeService: "migrate",
        composeOneoff: "True",
      },
      {
        id: "d".repeat(64),
        name: "standalone-redis",
        image: "redis:7",
        state: "running",
        status: "Up 1 day",
        ports: "6379/tcp",
        composeProject: "",
        composeService: "",
        composeOneoff: "",
      },
    ];
    (window as any).projectActions = [];
    bridge.invoke = async (cmd: string, args: any) => {
      if (cmd === "cockpit_collect" && args.section === "containers")
        return structuredClone(containers);
      if (cmd === "cockpit_project_action") {
        (window as any).projectActions.push(args);
        containers
          .filter((c) => args.expectedIds.includes(c.id))
          .forEach((c) => {
            c.state = args.action === "stop" ? "exited" : "running";
            c.status = args.action === "stop" ? "Exited (0)" : "Up 1 second";
          });
        return;
      }
      if (cmd === "cockpit_logs" && args.source === "compose")
        return "--- shop-api-1 ---\nApplication ready\n--- shop-db-1 ---\nDatabase ready";
      return original(cmd, args);
    };
  });
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await page.getByRole("tab", { name: "Containers", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Docker & Compose" }),
  ).toBeVisible();
  await expect(
    page.getByText(
      "2 services · 2/2 service containers running · 1 one-off job",
    ),
  ).toBeVisible();
  await expect(page.locator(".compose-project summary .status")).toHaveText(
    "Running",
  );
  await page.getByRole("button", { name: "Actions for project shop" }).click();
  await expect(
    page.getByRole("menuitem", { name: "Start project shop", exact: true }),
  ).toBeDisabled();
  await page.keyboard.press("Escape");
  await page.getByLabel("Filter Docker projects and containers").fill("shop");
  await page.screenshot({
    path: "test-results/screenshots/compose-desktop.png",
  });
  await page
    .getByRole("button", { name: "View project logs for shop" })
    .click();
  await expect(page.getByLabel("Log output")).toContainText("Database ready");
  await page.getByRole("button", { name: "Close logs" }).click();
  await expect(
    page.getByLabel("Filter Docker projects and containers"),
  ).toHaveValue("shop");
  await expect(
    page.getByRole("button", { name: "View project logs for shop" }),
  ).toBeFocused();
  await page.getByRole("button", { name: "Clear filter" }).click();
  await page.getByRole("button", { name: "Collapse all" }).click();
  await expect(page.locator(".compose-project")).not.toHaveAttribute(
    "open",
    "",
  );
  await page.getByRole("button", { name: "Expand all" }).click();
  await page.getByRole("button", { name: "Actions for project shop" }).click();
  await page.getByRole("menuitem", { name: "Stop project shop" }).click();
  await expect(page.getByRole("dialog")).toContainText(
    "2 existing service containers",
  );
  await page.screenshot({
    path: "test-results/screenshots/compose-confirmation.png",
  });
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  expect(await page.evaluate(() => (window as any).projectActions.length)).toBe(
    0,
  );
  await page.getByRole("button", { name: "Actions for project shop" }).click();
  await page.getByRole("menuitem", { name: "Stop project shop" }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Stop project", exact: true })
    .click();
  await expect(page.getByText("shop: stop completed.")).toBeVisible();
  expect(
    await page.evaluate(() => (window as any).projectActions[0].expectedIds),
  ).toEqual(["a".repeat(64), "b".repeat(64)]);
  await page.getByRole("button", { name: "Actions for project shop" }).click();
  await page
    .getByRole("menuitem", { name: "Start project shop", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Start project", exact: true })
    .click();
  await expect(page.getByText("shop: start completed.")).toBeVisible();
  await page.getByRole("button", { name: "Actions for project shop" }).click();
  await page.getByRole("menuitem", { name: "Restart project shop" }).click();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Restart project", exact: true })
    .click();
  await expect(page.getByText("shop: restart completed.")).toBeVisible();
  await page.setViewportSize({ width: 640, height: 480 });
  await page.locator(".compose-project").scrollIntoViewIfNeeded();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: "test-results/screenshots/compose-compact.png",
  });
  await page.setViewportSize({ width: 960, height: 680 });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.screenshot({ path: "test-results/screenshots/compose-dark.png" });
});

test("agent key selection persists and can switch to a file", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Edit server", exact: true }).click();
  const fingerprint = "SHA256:" + "A".repeat(43);
  await page.getByRole("combobox", { name: "SSH key", exact: true }).click();
  await expect(
    page.getByRole("option", { name: /1Password.*Production deploy key/ }),
  ).toHaveCount(1);
  await page
    .getByRole("option", { name: /1Password.*Production deploy key/ })
    .click();
  await expect(
    page.getByText(`Selected key: ${fingerprint}`, { exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: "test-results/screenshots/agent-key-desktop.png",
  });
  await page.getByRole("button", { name: "Save server", exact: true }).click();
  await page.getByRole("button", { name: "Edit server", exact: true }).click();
  await expect(
    page.getByRole("combobox", { name: "SSH key", exact: true }),
  ).toContainText("Production deploy key");
  await page.setViewportSize({ width: 640, height: 480 });
  await page.getByRole("combobox", { name: "SSH key", exact: true }).click();
  await page
    .getByRole("option", { name: "Enter a key-file path…", exact: true })
    .click();
  await page.getByLabel(/^Identity file/).fill("~/.ssh/deploy_key");
  await page.screenshot({
    path: "test-results/screenshots/agent-key-compact.png",
  });
  await page.getByRole("button", { name: "Save server", exact: true }).click();
  await page.getByRole("button", { name: "Edit server", exact: true }).click();
  await expect(
    page.getByRole("combobox", { name: "SSH key", exact: true }),
  ).toContainText("Enter a key-file path");
  await expect(page.getByLabel(/^Identity file/)).toHaveValue(
    "~/.ssh/deploy_key",
  );
  await page.getByRole("combobox", { name: "SSH key", exact: true }).click();
  await page.getByRole("option", { name: /Automatic/ }).click();
  await expect(page.getByLabel(/^Identity file/)).toHaveCount(0);
});

test("workspace command palette supports keyboard navigation", async ({
  page,
}) => {
  await page.goto("/");
  await expect(
    page.getByRole("button", { name: "Choose server", exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Meta+k");
  await expect(page.getByRole("dialog")).toBeVisible();
  await page
    .getByPlaceholder("Search servers and actions…")
    .fill("Open containers");
  await page.keyboard.press("Enter");
  await expect(
    page.getByRole("tab", { name: "Containers", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Meta+k");
  await page.screenshot({
    path: "test-results/screenshots/command-palette.png",
  });
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("native sidebar stays usable at minimum size and server picker searches", async ({
  page,
}) => {
  await page.setViewportSize({ width: 640, height: 480 });
  await page.goto("/");
  const overview = page.getByRole("tab", { name: "Overview", exact: true });
  await expect(overview).toBeVisible();
  for (const name of [
    "Overview",
    "Connections",
    "Services",
    "Containers",
    "Commands",
  ]) {
    const tab = page.getByRole("tab", { name, exact: true });
    await expect(tab).toBeInViewport();
    const box = await tab.boundingBox();
    expect(box!.height).toBeLessThanOrEqual(40);
  }
  await overview.focus();
  await page.keyboard.press("ArrowDown");
  await expect(
    page.getByRole("tab", { name: "Connections", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await page
    .getByRole("button", { name: "Choose server", exact: true })
    .click();
  await page.getByPlaceholder("Find a server…").fill("no-such-server");
  await expect(
    page.getByText("No matching servers.", { exact: true }),
  ).toBeVisible();
  await page.getByPlaceholder("Find a server…").fill("Development");
  await page.keyboard.press("Enter");
  await expect(page.getByPlaceholder("Find a server…")).toHaveCount(0);
});

test("sidebar resizes, remembers width, and can be restored with the toolbar", async ({
  page,
}) => {
  await page.goto("/");
  const divider = page.getByRole("separator", { name: "Sidebar width" });
  const handle = await divider.boundingBox();
  if (!handle) throw new Error("Sidebar resize handle is missing");
  await page.mouse.move(handle.x + handle.width / 2, handle.y + 120);
  await page.mouse.down();
  await page.mouse.move(260, handle.y + 180, { steps: 8 });
  await page.mouse.up();
  await expect(divider).toHaveAttribute("aria-valuenow", "260");
  expect(await page.evaluate(() => window.getSelection()?.toString())).toBe("");
  await divider.dblclick();
  await expect(divider).toHaveAttribute("aria-valuenow", "204");
  await divider.focus();
  await page.keyboard.press("ArrowRight");
  await expect(divider).toHaveAttribute("aria-valuenow", "214");
  await expect
    .poll(() =>
      page.evaluate(() => sessionStorage.getItem("fixture-sidebar-width")),
    )
    .toBe("214");
  await page.reload();
  await expect(divider).toHaveAttribute("aria-valuenow", "214");
  await page.getByRole("button", { name: "Hide sidebar", exact: true }).click();
  await expect(
    page.getByRole("tab", { name: "Overview", exact: true }),
  ).toBeHidden();
  await page.getByRole("button", { name: "Show sidebar", exact: true }).click();
  await expect(
    page.getByRole("tab", { name: "Overview", exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Meta+2");
  await expect(
    page.getByRole("tab", { name: "Connections", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
});

test("shadcn dropdowns support keyboard selection and compact long labels", async ({
  page,
}) => {
  await page.goto("/?fixture=history");
  const history = page.getByLabel("History time window");
  await history.focus();
  await page.keyboard.press("Space");
  await expect(
    page.getByRole("option", { name: "Last 5 minutes", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("End");
  await expect(
    page.getByRole("option", { name: "Last 15 minutes", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(history).toContainText("Last 15 minutes");
  await expect(history).toBeFocused();
  await history.click();
  await page.screenshot({
    path: "test-results/screenshots/dropdown-history.png",
  });
  await page.keyboard.press("Escape");
  await page.getByLabel("Sort processes").click();
  await page
    .getByRole("option", { name: "Highest memory", exact: true })
    .click();
  await expect(page.getByLabel("Sort processes")).toContainText(
    "Highest memory",
  );
  await page.getByLabel("Graph network interface").click();
  await expect(page.getByRole("listbox")).toBeVisible();
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "Server actions", exact: true })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Edit server…", exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: "test-results/screenshots/dropdown-actions.png",
  });
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Edit server", exact: true }).click();
  await page.getByLabel("Authentication", { exact: true }).click();
  await page.getByRole("option", { name: "Password", exact: true }).click();
  await expect(
    page.getByLabel("Authentication", { exact: true }),
  ).toContainText("Password");
  await page.getByLabel("Authentication", { exact: true }).click();
  await page
    .getByRole("option", { name: "SSH key / agent", exact: true })
    .click();
  await page.setViewportSize({ width: 640, height: 480 });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.getByRole("combobox", { name: "SSH key", exact: true }).click();
  const list = page.getByRole("listbox");
  await expect(list).toBeVisible();
  const bounds = await list.boundingBox();
  expect(bounds!.x).toBeGreaterThanOrEqual(0);
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(640);
  await page.screenshot({
    path: "test-results/screenshots/dropdown-key-compact-dark.png",
  });
  await page.keyboard.press("Escape");
  await expect(
    page.getByRole("combobox", { name: "SSH key", exact: true }),
  ).toBeFocused();
});

test("lists expose context, sort state and filtered results", async ({
  page,
}) => {
  await page.goto("/");
  const processes = page.getByRole("table", { name: "Processes", exact: true });
  await expect(
    processes.getByRole("columnheader", { name: "CPU", exact: true }),
  ).toHaveAttribute("aria-sort", "descending");
  await page.getByLabel("Filter processes").fill("  postgres  ");
  await expect(processes.getByRole("row")).toHaveCount(2);
  await expect(page.getByText(/1 matching · 3 collected/)).toBeVisible();
  await page.getByLabel("Filter processes").fill("not-a-process");
  await expect(
    page.getByText("No matches in the collected processes."),
  ).toBeVisible();
  await page.getByRole("tab", { name: "Services", exact: true }).click();
  const services = page.getByRole("list", {
    name: "System services",
    exact: true,
  });
  await expect(services.getByRole("listitem")).toHaveCount(2);
  await expect(services.getByRole("listitem").first()).toContainText(
    "worker.service",
  );
  await page.getByLabel("Filter services").fill("  ssh  ");
  await expect(services.getByRole("listitem")).toHaveCount(1);
  await page.getByLabel("Filter services").fill("no-such-service");
  await expect(page.getByText(/No matching services/)).toBeVisible();
  await page.getByRole("tab", { name: /Connections/ }).click();
  const tunnels = page.getByRole("list", { name: "Tunnels", exact: true });
  await expect(tunnels.getByRole("listitem")).toHaveCount(1);
  await expect(
    tunnels.getByRole("button", { name: "Connect", exact: true }),
  ).toHaveAccessibleDescription(/.+/);
  await page
    .getByRole("button", { name: "Discover ports", exact: true })
    .click();
  const ports = page.getByRole("table", { name: "Remote ports", exact: true });
  await expect(ports.getByRole("columnheader")).toHaveText([
    "Port",
    "Application / address",
    "PID",
    "User",
    "Forwarding",
    "Actions",
  ]);
  await expect(
    ports.getByRole("button", { name: "Forward", exact: true }),
  ).toHaveAccessibleDescription(/\d+/);
  await page
    .getByRole("region", { name: "Remote ports scroll area", exact: true })
    .focus();
  await expect(
    page.getByRole("region", { name: "Remote ports scroll area", exact: true }),
  ).toBeFocused();
});

test("long list identities wrap without hiding row actions", async ({
  page,
}) => {
  await page.setViewportSize({ width: 640, height: 480 });
  await page.goto("/?fixture=long-lists");
  await page.getByRole("tab", { name: "Services", exact: true }).click();
  const first = page
    .getByRole("list", { name: "System services", exact: true })
    .getByRole("listitem")
    .first();
  await expect(first).toContainText("long-name-");
  const action = first.getByRole("button");
  await action.focus();
  await expect(action).toBeFocused();
  const box = await action.boundingBox();
  expect(box!.x + box!.width).toBeLessThanOrEqual(640);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: "test-results/screenshots/lists-long-compact.png",
  });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.screenshot({
    path: "test-results/screenshots/lists-long-dark.png",
  });
});

test("500-row collections render, scroll and filter responsively", async ({
  page,
  browserName,
}) => {
  await page.goto("/?fixture=large-lists");
  const started = Date.now();
  await page.getByRole("tab", { name: "Services", exact: true }).click();
  const services = page.getByRole("list", {
    name: "System services",
    exact: true,
  });
  await expect(services.getByRole("listitem")).toHaveCount(500);
  const readyMs = Date.now() - started;
  const frames = await page.evaluate(async () => {
    const pane = document.querySelector(".workspace-scroll")!;
    const intervals: number[] = [];
    let previous = performance.now();
    for (let i = 0; i < 60; i++) {
      await new Promise<void>((resolve) =>
        requestAnimationFrame((now) => {
          intervals.push(now - previous);
          previous = now;
          pane.scrollTop += 80;
          resolve();
        }),
      );
    }
    return intervals.sort((a, b) => a - b);
  });
  const filterStart = Date.now();
  await page.getByLabel("Filter services").fill("worker-0499");
  await expect(services.getByRole("listitem")).toHaveCount(1);
  const filterMs = Date.now() - filterStart;
  await page.getByRole("tab", { name: "Containers", exact: true }).click();
  await expect(
    page
      .getByRole("list", { name: "Standalone containers", exact: true })
      .getByRole("listitem"),
  ).toHaveCount(500);
  await page
    .getByLabel("Filter Docker projects and containers")
    .fill("worker-0499");
  await expect(
    page
      .getByRole("list", { name: "Standalone containers", exact: true })
      .getByRole("listitem"),
  ).toHaveCount(1);
  console.log(
    JSON.stringify({
      browserName,
      rows: 500,
      readyMs,
      filterMs,
      scrollFrameP95Ms: frames[Math.floor(frames.length * 0.95)],
      scrollFrameMaxMs: frames.at(-1),
    }),
  );
});

test("interactive terminal streams input, resizes and disconnects on navigation", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("tab", { name: "Commands", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Open a remote shell" }),
  ).toBeVisible();
  await page.screenshot({
    path: "test-results/screenshots/terminal-initial.png",
  });
  await page
    .getByRole("button", { name: "Connect terminal", exact: true })
    .click();
  await expect(page.locator(".terminal-status")).toHaveText("Connected");
  await expect(page.locator(".xterm-accessibility-tree")).toContainText(
    "Welcome to Development",
  );
  const expectTerminalFits = async () => {
    await expect
      .poll(() =>
        page.evaluate(() => {
          const workspace = document.querySelector(".workspace-scroll")!;
          const surface = document.querySelector(".terminal-surface")!;
          const screen = document.querySelector(".xterm-screen")!;
          return (
            workspace.scrollHeight <= workspace.clientHeight + 1 &&
            screen.getBoundingClientRect().bottom <=
              surface.getBoundingClientRect().bottom + 1
          );
        }),
      )
      .toBe(true);
  };
  await expectTerminalFits();
  await expect(page.locator(".xterm-viewport")).toHaveCSS(
    "background-color",
    "rgb(255, 255, 255)",
  );
  await page
    .getByLabel("Remote terminal input")
    .pressSequentially("echo hello");
  await page.getByLabel("Remote terminal input").press("Enter");
  await page.getByLabel("Remote terminal input").press("Control+c");
  await expect
    .poll(() =>
      page.evaluate(() => (window as any).__terminalWrites.flat().includes(3)),
    )
    .toBe(true);
  await page.screenshot({
    path: "test-results/screenshots/terminal-desktop.png",
  });
  await page.setViewportSize({ width: 640, height: 480 });
  await expect
    .poll(() => page.evaluate(() => (window as any).__terminalSizes.length))
    .toBeGreaterThan(1);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: "test-results/screenshots/terminal-compact.png",
  });
  await expectTerminalFits();
  await page.emulateMedia({ colorScheme: "dark" });
  await page.screenshot({ path: "test-results/screenshots/terminal-dark.png" });
  await page.getByRole("button", { name: "Disconnect terminal" }).click();
  await expect(page.locator(".terminal-status")).toHaveText("Disconnected");
  await page
    .getByRole("button", { name: "Connect terminal", exact: true })
    .click();
  await expect(page.locator(".terminal-status")).toHaveText("Connected");
  const before = await page.evaluate(
    () => (window as any).__terminalClosed.length,
  );
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => (window as any).__terminalClosed.length))
    .toBeGreaterThan(before);
});

test("terminal setup errors allow a fresh connection", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    bridge.invoke = (cmd: string, args: any) =>
      cmd === "terminal_open"
        ? Promise.reject("Server refused the terminal PTY.")
        : original(cmd, args);
  });
  await page.getByRole("tab", { name: "Commands", exact: true }).click();
  await page.getByRole("button", { name: "Connect terminal" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "refused the terminal PTY",
  );
  await expect(
    page.getByRole("button", { name: "Connect terminal" }),
  ).toBeEnabled();
});

test("table headers stick and collection refresh keeps the visible row and focus", async ({
  page,
  browserName,
}) => {
  await page.goto("/?fixture=large-lists");
  const region = page.getByRole("region", {
    name: "Processes scroll area",
    exact: true,
  });
  await expect(region.getByRole("row")).toHaveCount(51);
  await region.scrollIntoViewIfNeeded();
  await region.evaluate((el) => {
    el.scrollTop = 700;
  });
  const regionBox = await region.boundingBox();
  const headerBox = await region
    .getByRole("columnheader", { name: "Process", exact: true })
    .boundingBox();
  expect(Math.abs(headerBox!.y - regionBox!.y)).toBeLessThan(2);
  await page.screenshot({
    path: `test-results/screenshots/sticky-processes-${browserName}.png`,
  });
  await page.getByRole("tab", { name: "Services", exact: true }).click();
  const anchor = page
    .getByRole("listitem")
    .filter({ has: page.getByText("worker-0250.service", { exact: true }) });
  await anchor.evaluate((el) => el.scrollIntoView({ block: "start" }));
  await anchor
    .getByRole("button")
    .evaluate((el) => el.focus({ preventScroll: true }));
  const before = await anchor.boundingBox();
  await page
    .getByRole("button", { name: "Refresh", exact: true })
    .evaluate((el) => el.click());
  await expect(
    page.getByText("worker-0000.service", { exact: true }),
  ).toBeAttached();
  const after = await anchor.boundingBox();
  expect(Math.abs(before!.y - after!.y)).toBeLessThan(2);
  await expect(anchor.getByRole("button")).toBeFocused();
});

test("leaving during terminal setup closes a late session", async ({
  page,
}) => {
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    bridge.invoke = (cmd: string, args: any) => {
      if (cmd !== "terminal_open") return original(cmd, args);
      (window as any).__pendingTerminal = args.session;
      return new Promise((resolve) => {
        (window as any).__finishTerminalOpen = async () => {
          await original(cmd, args);
          resolve(undefined);
        };
      });
    };
  });
  await page.getByRole("tab", { name: "Commands", exact: true }).click();
  await page.getByRole("button", { name: "Connect terminal" }).click();
  await expect(page.locator(".terminal-status")).toHaveText("Connecting…");
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await page.evaluate(() => (window as any).__finishTerminalOpen());
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__terminalClosed.filter(
            (id: string) => id === (window as any).__pendingTerminal,
          ).length,
      ),
    )
    .toBe(2);
});

test("port discovery identifies containers and explains unavailable ownership", async ({
  page,
}) => {
  await page.goto("/?fixture=docker-ports");
  await page.getByRole("tab", { name: /Connections/ }).click();
  await page
    .getByRole("button", { name: "Discover ports", exact: true })
    .click();
  const row = page.getByRole("row").filter({ hasText: "api · Docker" });
  await expect(row).toBeVisible();
  await expect(
    page.getByRole("cell", { name: /Owner not reported/ }),
  ).toBeVisible();
  await row.getByRole("button", { name: "Forward", exact: true }).click();
  await expect(page.getByLabel(/^Remote host/)).toHaveValue("::1");
});

test("container inventory distinguishes loading, empty, and filtered states", async ({
  page,
}) => {
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    bridge.invoke = (cmd: string, args: any) =>
      cmd === "cockpit_collect" && args.section === "containers"
        ? new Promise((resolve) => {
            (window as any).__resolveContainers = resolve;
          })
        : original(cmd, args);
  });
  await page.getByRole("tab", { name: "Containers", exact: true }).click();
  await expect(
    page.getByText("Reading containers…", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("0 projects · 0 containers", { exact: true }),
  ).toHaveCount(0);
  await page.evaluate(() => (window as any).__resolveContainers([]));
  await expect(
    page.getByText("No Docker containers on this server."),
  ).toBeVisible();
  await page
    .getByLabel("Filter Docker projects and containers")
    .fill("missing");
  await expect(
    page.getByText("No matches. Try another name or clear the filter."),
  ).toBeVisible();
  await page.getByRole("button", { name: "Clear filter" }).click();
  await expect(
    page.getByLabel("Filter Docker projects and containers"),
  ).toHaveValue("");
});

test("service state filters and log return keep the working context", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("tab", { name: "Services", exact: true }).click();
  await page.getByRole("combobox", { name: "Filter by service state" }).click();
  await page.getByRole("option", { name: "Failed", exact: true }).click();
  const list = page.getByRole("list", { name: "System services", exact: true });
  await expect(list.getByRole("listitem")).toHaveCount(1);
  await page
    .getByRole("button", { name: "View logs for worker.service" })
    .click();
  await expect(page.getByRole("button", { name: "Close logs" })).toBeFocused();
  await page.getByRole("button", { name: "Close logs" }).click();
  await expect(
    page.getByRole("combobox", { name: "Filter by service state" }),
  ).toContainText("Failed");
  await expect(
    page.getByRole("button", { name: "View logs for worker.service" }),
  ).toBeFocused();
  await page.getByRole("button", { name: "Clear filters" }).click();
  await expect(list.getByRole("listitem")).toHaveCount(2);
  await page.setViewportSize({ width: 640, height: 480 });
  await page.screenshot({
    path: "test-results/screenshots/services-compact.png",
  });
});

test("server picker distinguishes accounts, restores focus and offers add after no results", async ({
  page,
}, testInfo) => {
  await page.goto("/?fixture=server-picker");
  const trigger = page.getByRole("button", {
    name: "Choose server",
    exact: true,
  });
  await trigger.click();
  const picker = page.getByRole("dialog", { name: "Server picker" });
  await expect(
    page.getByRole("combobox", { name: "Search servers" }),
  ).toBeFocused();
  await expect(picker.locator('[data-checked="true"]')).toHaveCount(1);
  await page.screenshot({
    path: testInfo.outputPath("server-picker-desktop.png"),
  });
  await page.getByPlaceholder("Find a server…").fill("operator");
  await expect(picker.getByRole("option")).toHaveCount(1);
  await expect(picker.getByRole("option")).toContainText(
    "operator@dev.example.com:2222",
  );
  await page.keyboard.press("Enter");
  await expect(picker).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await expect(trigger).toContainText("operator@dev.example.com:2222");
  await trigger.click();
  await expect(picker.locator('[data-checked="true"]')).toContainText(
    "operator@",
  );
  await page.keyboard.press("Escape");
  await expect(trigger).toBeFocused();
  await page.setViewportSize({ width: 640, height: 480 });
  await page.emulateMedia({ colorScheme: "dark" });
  await trigger.click();
  await page.getByPlaceholder("Find a server…").fill("analytics");
  const row = picker.getByRole("option");
  await expect(row).toContainText(
    "Production analytics and reporting cluster in the western region",
  );
  const box = await picker.boundingBox();
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(640);
  expect(box!.y + box!.height).toBeLessThanOrEqual(480);
  expect(
    await row.evaluate((element) => element.scrollWidth <= element.clientWidth),
  ).toBe(true);
  await page.screenshot({
    path: testInfo.outputPath("server-picker-compact-dark.png"),
  });
  await page.getByPlaceholder("Find a server…").fill("no-such-server");
  await expect(picker.getByText("No matching servers.")).toBeVisible();
  await picker
    .getByRole("button", { name: "Add server…", exact: true })
    .click();
  await expect(
    page.getByRole("dialog", { name: "Add server", exact: true }),
  ).toBeVisible();
});

test("remembered integrations can retry or be disabled while disconnected", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("tab", { name: "Integration", exact: true }).click();
  const section = page.locator(".integration-panel");
  const browser = section.getByRole("switch", { name: "Browser", exact: true });
  await browser.click();
  await page.reload();
  await page.getByRole("tab", { name: "Integration", exact: true }).click();
  await expect(browser).toBeChecked();
  await expect(
    section.getByText("Disconnected", { exact: true }),
  ).toBeVisible();
  await section.getByRole("button", { name: "Retry", exact: true }).click();
  await expect(section.getByText("Connected", { exact: true })).toBeVisible();
  await page.reload();
  await page.getByRole("tab", { name: "Integration", exact: true }).click();
  await browser.click();
  await expect(section.getByText("Off", { exact: true })).toBeVisible();
  await page.reload();
  await page.getByRole("tab", { name: "Integration", exact: true }).click();
  await expect(browser).not.toBeChecked();
  await expect(
    section.getByRole("button", { name: "Retry", exact: true }),
  ).toHaveCount(0);
});

test("buttons explain icon actions and prevent repeat submission while busy", async ({
  page,
}) => {
  await page.goto("/");
  const menu = page.getByRole("button", {
    name: "Server actions",
    exact: true,
  });
  await menu.hover();
  await expect(page.getByRole("tooltip")).toHaveText("Server actions");
  await page.getByRole("button", { name: "Edit server", exact: true }).click();
  await page
    .getByRole("button", { name: "Refresh agent keys", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    (window as any).__saveCount = 0;
    bridge.invoke = (cmd: string, args: any) => {
      if (cmd !== "save_server") return original(cmd, args);
      (window as any).__saveCount++;
      return new Promise((resolve) => {
        (window as any).__finishSave = () => resolve(original(cmd, args));
      });
    };
  });
  await page.getByRole("button", { name: "Save server", exact: true }).click();
  const saving = page.getByRole("button", { name: "Saving…", exact: true });
  await expect(saving).toBeDisabled();
  await expect(saving).toHaveAttribute("aria-busy", "true");
  await expect(saving.locator(".button-spinner")).toBeVisible();
  await page.keyboard.press("Enter");
  expect(await page.evaluate(() => (window as any).__saveCount)).toBe(1);
  await page.evaluate(() => (window as any).__finishSave());
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("Settings is separate from server tools and saves appearance", async ({
  page,
}) => {
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    bridge.invoke = (cmd: string, args: any) => {
      if (cmd === "get_startup_settings")
        return Promise.resolve({ enabled: false, available: true });
      if (cmd === "set_launch_at_login")
        return Promise.resolve({ enabled: args.enabled, available: true });
      return original(cmd, args);
    };
  });
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Settings", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Edit server", exact: true }),
  ).toHaveCount(0);
  const theme = page.getByRole("combobox", { name: "Theme", exact: true });
  await theme.click();
  await page.getByRole("option", { name: "Dark", exact: true }).click();
  await expect(page.locator("html")).toHaveClass(/dark/);
  const startup = page.getByRole("checkbox", { name: /Launch at login/ });
  await startup.click();
  await expect(startup).toBeChecked();
  await page.screenshot({ path: "test-results/screenshots/settings-dark.png" });
  await theme.click();
  await page.getByRole("option", { name: "Light", exact: true }).click();
  await page.setViewportSize({ width: 640, height: 480 });
  await page.screenshot({
    path: "test-results/screenshots/settings-compact.png",
  });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await page.emulateMedia({ colorScheme: "dark" });
  await page
    .getByRole("button", { name: "Test connection", exact: true })
    .click();
  await expect(page.locator("[data-sonner-toaster]")).toHaveAttribute(
    "data-sonner-theme",
    "light",
  );

  await expect(
    page.getByRole("heading", { name: "System health", exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Meta+,");
  await expect(
    page.getByRole("heading", { name: "Settings", exact: true }),
  ).toBeVisible();
  await page.reload();
  expect(
    await page.evaluate(() => localStorage.getItem("porthop-appearance")),
  ).toBe("light");
  await expect(page.locator("html")).not.toHaveClass(/dark/);
});

test("Settings displays and clears metrics cache with retry", async ({
  page,
}) => {
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    let info = { bytes: 12 * 1024 * 1024, samples: 1000 };
    let attempts = 0;
    bridge.invoke = (cmd: string, args: any) => {
      if (cmd === "get_metrics_cache") return Promise.resolve(info);
      if (cmd === "clear_metrics_cache") {
        if (++attempts === 1)
          return Promise.reject("Could not clear metric history: storage busy");
        return new Promise((resolve) =>
          setTimeout(() => {
            info = { bytes: 65536, samples: 0 };
            resolve(info);
          }, 300),
        );
      }
      return original(cmd, args);
    };
  });
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const size = page.getByLabel("Metrics cache size");
  const clear = page.getByRole("button", { name: "Clear cache", exact: true });
  await expect(size).toHaveText("12.0 MiB");
  await expect(clear).toBeEnabled();
  await page.screenshot({
    path: "test-results/screenshots/settings-cache.png",
  });
  await page.setViewportSize({ width: 640, height: 480 });
  await clear.scrollIntoViewIfNeeded();
  await page.screenshot({
    path: "test-results/screenshots/settings-cache-compact.png",
  });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await clear.click();
  await expect(page.getByRole("alert")).toContainText("storage busy");
  await expect(size).toHaveText("12.0 MiB");
  await clear.click();
  await expect(clear).toBeDisabled();
  await expect(page.getByText("Cache cleared.", { exact: true })).toBeVisible();
  await expect(size).toHaveText("64.0 KiB");
  await expect(clear).toBeDisabled();
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("Settings recovers a failed cache size request", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    let failed = true;
    (window as any).allowCacheSize = () => {
      failed = false;
    };
    bridge.invoke = (cmd: string, args: any) =>
      cmd === "get_metrics_cache"
        ? failed
          ? Promise.reject("Cache is unavailable")
          : Promise.resolve({ bytes: 32768, samples: 0 })
        : original(cmd, args);
  });
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Clear cache", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("region", { name: "Cache", exact: true }).getByRole("alert"),
  ).toContainText("Cache is unavailable");
  await page.evaluate(() => (window as any).allowCacheSize());
  await page.getByRole("button", { name: "Retry cache size" }).click();
  await expect(page.getByLabel("Metrics cache size")).toHaveText("32.0 KiB");
  await expect(page.getByText("No recorded history.")).toBeVisible();
});

test("Settings explains unavailable startup controls and recovers from save errors", async ({
  page,
}) => {
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    let attempts = 0;
    bridge.invoke = (cmd: string, args: any) => {
      if (cmd === "get_startup_settings")
        return Promise.resolve({ enabled: false, available: true });
      if (cmd === "set_launch_at_login") {
        if (++attempts === 1)
          return Promise.reject("Could not update login item");
        return Promise.resolve({ enabled: args.enabled, available: true });
      }
      return original(cmd, args);
    };
  });
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const startup = page.getByRole("checkbox", { name: /Launch at login/ });
  await startup.click();
  await expect(page.getByRole("alert")).toContainText(
    "Could not update login item",
  );
  await expect(startup).not.toBeChecked();
  await startup.click();
  await expect(startup).toBeChecked();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    bridge.invoke = (cmd: string, args: any) =>
      cmd === "get_startup_settings"
        ? Promise.resolve({ enabled: false, available: false })
        : original(cmd, args);
  });
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(startup).toBeDisabled();
  await expect(
    page.getByText("Available in the installed app.", { exact: true }),
  ).toBeVisible();
});

test("metadata updates preserve the workspace and connected terminal", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("tab", { name: "Commands", exact: true }).click();
  await page
    .getByRole("button", { name: "Connect terminal", exact: true })
    .click();
  await expect(page.locator(".terminal-status")).toHaveText("Connected");
  await page.evaluate(() => {
    (window as any).__workspaceBefore = document.querySelector(".cockpit");
    (window as any).__closedBefore = (window as any).__terminalClosed.length;
  });
  await page.getByRole("button", { name: "Edit server", exact: true }).click();
  await page.getByLabel("Name (optional)", { exact: true }).fill("Renamed");
  await page.getByRole("button", { name: "Save server", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        document.querySelector(".cockpit") ===
        (window as any).__workspaceBefore,
    ),
  ).toBe(true);
  await expect(page.locator(".terminal-status")).toHaveText("Connected");
  expect(
    await page.evaluate(
      () =>
        (window as any).__terminalClosed.length -
        (window as any).__closedBefore,
    ),
  ).toBe(0);
  // A real connection change must still discard the old workspace/session.
  await page.getByRole("button", { name: "Edit server", exact: true }).click();
  await page.getByLabel("SSH host", { exact: true }).fill("new.example.com");
  await page.getByRole("button", { name: "Save server", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator(".terminal-status")).toHaveText("Disconnected");
  expect(
    await page.evaluate(
      () =>
        document.querySelector(".cockpit") ===
        (window as any).__workspaceBefore,
    ),
  ).toBe(false);
});

test("changing a host clears its old metrics and discovered ports", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page.locator(".metric-history")).toBeVisible();
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  await page
    .getByRole("button", { name: "Discover ports", exact: true })
    .click();
  await expect(
    page.getByRole("table", { name: "Remote ports", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const invoke = bridge.invoke;
    let changed = false;
    bridge.invoke = async (command: string, args: any) => {
      if (command === "save_server") {
        const result = await invoke(command, args);
        changed = true;
        return result;
      }
      if (changed && command === "cockpit_history") return [];
      if (changed && command === "discover_ports")
        throw new Error("New endpoint is offline");
      if (
        changed &&
        command === "cockpit_collect" &&
        args.section === "overview"
      )
        throw new Error("New endpoint is offline");
      return invoke(command, args);
    };
  });
  await page.getByRole("button", { name: "Edit server", exact: true }).click();
  await page
    .getByLabel("SSH host", { exact: true })
    .fill("new-endpoint.example.com");
  await page.getByRole("button", { name: "Save server", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(
    page.getByRole("table", { name: "Remote ports", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "New endpoint is offline",
  );
  await expect(page.locator(".metric-history")).toHaveCount(0);
  await expect(
    page.getByRole("meter", { name: "CPU usage", exact: true }),
  ).toHaveCount(0);
});

test("command palette distinguishes accounts sharing the same display name and host", async ({
  page,
}) => {
  await page.goto("/?fixture=server-picker");
  await page.keyboard.press("Meta+k");
  await page.getByPlaceholder("Search servers and actions…").fill("operator");
  const item = page
    .getByRole("option")
    .filter({ hasText: "operator@dev.example.com:2222" });
  await expect(item).toHaveCount(1);
  await expect(item).toHaveAttribute(
    "data-value",
    "server:same-host-other-user",
  );
  await page.keyboard.press("Enter");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Choose server" }),
  ).toContainText("operator");
});

test("confirmation dialogs block workspace shortcuts and command search", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    const callbacks = new Map<number, (event: any) => void>();
    bridge.transformCallback = (callback: (event: any) => void) => {
      const id = callbacks.size + 1;
      callbacks.set(id, callback);
      return id;
    };
    bridge.invoke = (cmd: string, args: any) => {
      if (cmd === "plugin:event|listen") {
        if (args.event === "workspace-action")
          (window as any).__workspaceAction = callbacks.get(args.handler);
        return Promise.resolve(args.handler);
      }
      if (cmd === "plugin:event|unlisten") return Promise.resolve();
      return original(cmd, args);
    };
  });
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    bridge.invoke = (cmd: string, args: any) =>
      cmd === "cockpit_collect" && args.section === "containers"
        ? Promise.resolve([
            {
              id: "a".repeat(64),
              name: "shop-api-1",
              image: "shop:latest",
              state: "running",
              status: "Up",
              ports: "",
              composeProject: "shop",
              composeService: "api",
              composeOneoff: "False",
            },
          ])
        : original(cmd, args);
  });
  await page.getByRole("tab", { name: "Containers", exact: true }).click();
  await page.getByRole("button", { name: "Actions for project shop" }).click();
  await page.getByRole("menuitem", { name: "Stop project shop" }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.evaluate(() =>
    (window as any).__workspaceAction({ payload: "view-overview" }),
  );
  await page.keyboard.press("Meta+1");
  await page.keyboard.press("Meta+k");
  await expect(page.locator('[role="tab"][data-state="active"]')).toContainText(
    "Containers",
  );
  await expect(page.getByRole("dialog")).toHaveCount(1);
  await expect(page.getByRole("dialog")).toContainText("Stop project");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.keyboard.press("Meta+1");
  await expect(
    page.getByRole("tab", { name: "Overview", exact: true }),
  ).toHaveAttribute("data-state", "active");
});

test("editors restore keyboard focus after cancel and save", async ({
  page,
}) => {
  await page.goto("/");
  const edit = page.getByRole("button", { name: "Edit server", exact: true });
  await edit.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(edit).toBeFocused();
  await page.keyboard.press("Enter");
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Save server", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(edit).toBeFocused();
});

test("failed lazy workspace leaves navigation and recovery available", async ({
  page,
}) => {
  await page.route(
    /(?:TerminalPanel-.*\.js|\/src\/components\/TerminalPanel.tsx)/,
    (route) =>
      route.fulfill({
        contentType: "application/javascript",
        body: 'throw new Error("Fixture lazy import failure");',
      }),
  );
  await page.goto("/");
  await page.getByRole("tab", { name: "Commands", exact: true }).click();
  await expect(
    page.getByRole("alert", { name: "View unavailable" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Reload window" }),
  ).toBeVisible();
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  await expect(
    page.getByRole("alert", { name: "View unavailable" }),
  ).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Add tunnel" })).toBeVisible();
});

test("Files and its icon catalog load only when the workspace is opened", async ({
  page,
}) => {
  const requests: string[] = [];
  page.on("request", (request) => requests.push(request.url()));
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "System health" }),
  ).toBeVisible();
  expect(
    requests.some((url) =>
      /(FilesPanel-.*\.js|\/src\/components\/FilesPanel.tsx)/.test(url),
    ),
  ).toBe(false);
  await page.getByRole("tab", { name: "Files", exact: true }).click();
  await expect
    .poll(() =>
      requests.some((url) =>
        /(FilesPanel-.*\.js|\/src\/components\/FilesPanel.tsx)/.test(url),
      ),
    )
    .toBe(true);
});

test("modal forms keep actions visible at minimum window size", async ({
  page,
}) => {
  await page.goto("/");
  await page.setViewportSize({ width: 640, height: 480 });
  await page.getByRole("button", { name: "Edit server", exact: true }).click();
  await expect(page.getByRole("dialog")).toBeInViewport();
  await expect(
    page.getByRole("button", { name: "Save server", exact: true }),
  ).toBeInViewport();
  await page.screenshot({
    path: "test-results/screenshots/modal-server-compact.png",
  });
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  await page.getByRole("button", { name: "Add tunnel", exact: true }).click();
  await expect(
    page
      .getByRole("dialog")
      .getByRole("button", { name: "Save tunnel", exact: true }),
  ).toBeInViewport();
  await page.screenshot({
    path: "test-results/screenshots/modal-tunnel-compact.png",
  });
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
});

test("Updates download without installing and require an explicit restart", async ({
  page,
}) => {
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    let phase = "idle";
    (window as any).__updateInstalls = 0;
    bridge.invoke = async (cmd: string, args: any) => {
      if (cmd === "update_status")
        return {
          enabled: true,
          currentVersion: "0.2.0",
          phase,
          version: "0.3.0",
          downloaded: 50,
          total: 100,
          error: null,
        };
      if (cmd === "check_for_updates") {
        phase = "downloading";
        await new Promise((resolve) => setTimeout(resolve, 400));
        phase = "ready";
        return;
      }
      if (cmd === "install_update") {
        (window as any).__updateInstalls++;
        return;
      }
      return original(cmd, args);
    };
  });
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page
    .getByRole("button", { name: "Check for updates", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Check for updates", exact: true }),
  ).toBeDisabled();
  const restart = page.getByRole("button", {
    name: "Restart to update",
    exact: true,
  });
  await expect(restart).toBeEnabled();
  expect(await page.evaluate(() => (window as any).__updateInstalls)).toBe(0);
  await restart.click();
  await expect(
    page.getByText(
      "Restarting disconnects active sessions and cancels transfers.",
    ),
  ).toBeVisible();
  await page.getByRole("button", { name: "Later", exact: true }).click();
  expect(await page.evaluate(() => (window as any).__updateInstalls)).toBe(0);
  await page.screenshot({ path: "test-results/screenshots/updates-ready.png" });
  await restart.click();
  await page.getByRole("button", { name: "Restart now", exact: true }).click();
  await expect
    .poll(() => page.evaluate(() => (window as any).__updateInstalls))
    .toBe(1);
});

test("Updates recover from a failed check", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    let phase = "idle";
    let attempts = 0;
    bridge.invoke = async (cmd: string, args: any) => {
      if (cmd === "update_status")
        return {
          enabled: true,
          currentVersion: "0.2.0",
          phase,
          version: null,
          downloaded: 0,
          total: null,
          error: null,
        };
      if (cmd === "check_for_updates") {
        if (++attempts === 1) throw new Error("Update server unavailable");
        phase = "current";
        return;
      }
      return original(cmd, args);
    };
  });
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const check = page.getByRole("button", {
    name: "Check for updates",
    exact: true,
  });
  await check.click();
  await expect(page.getByRole("alert")).toContainText(
    "Update server unavailable",
  );
  await check.click();
  await expect(page.getByText("You’re up to date.")).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("discovered Node applications show useful project context without system details", async ({
  page,
}) => {
  await page.goto("/?fixture=node-ports");
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  await page
    .getByRole("button", { name: "Discover ports", exact: true })
    .click();
  const table = page.getByRole("table", { name: "Remote ports" });
  await expect(
    table.getByText("Vite · webcontainers-demo", { exact: true }),
  ).toBeVisible();
  await expect(
    table.getByRole("cell", { name: "126945", exact: true }),
  ).toBeVisible();
  await expect(
    table.getByRole("cell", { name: "ruiyang", exact: true }),
  ).toBeVisible();
  await expect(
    table.getByText("Project directory", { exact: true }),
  ).toBeVisible();
  await expect(
    table.getByText("/home/ruiyang/Projects/webcontainers-demo", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(
    table.getByText("/nix/store/node/bin/node", { exact: true }),
  ).toHaveCount(0);
  await expect(table.locator("summary")).toHaveCount(0);
  for (const width of [960, 640]) {
    await page.setViewportSize({ width, height: 680 });
    await table.scrollIntoViewIfNeeded();
    await page.screenshot({ path: `/tmp/porthop-ports-${width}.png` });
  }
  await table.getByRole("button", { name: "Forward", exact: true }).click();
  await expect(
    page
      .getByRole("dialog")
      .locator('input[value="Vite · webcontainers-demo"]'),
  ).toBeVisible();
});

test("ports discover automatically and refresh while Connections is open", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto("/?fixture=node-ports");
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("table", { name: "Remote ports" })).toContainText(
    "Vite · webcontainers-demo",
  );
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    (window as any).__portPolls = 0;
    bridge.invoke = (cmd: string, args: any) => {
      if (cmd === "discover_ports") (window as any).__portPolls++;
      return original(cmd, args);
    };
  });
  await page.clock.fastForward(31_000);
  await expect
    .poll(() => page.evaluate(() => (window as any).__portPolls))
    .toBe(1);
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await page.clock.fastForward(31_000);
  expect(await page.evaluate(() => (window as any).__portPolls)).toBe(1);
});
test("automatic discovery failure keeps a retry available", async ({
  page,
}) => {
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    (window as any).__portDiscoveryFail = true;
    bridge.invoke = (cmd: string, args: any) => {
      if (cmd === "discover_ports" && (window as any).__portDiscoveryFail) {
        return Promise.reject(new Error("SSH unavailable"));
      }
      return original(cmd, args);
    };
  });
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Could not refresh ports",
  );
  await page.evaluate(() => {
    (window as any).__portDiscoveryFail = false;
  });
  await page
    .getByRole("button", { name: "Discover ports", exact: true })
    .click();
  await expect(page.getByRole("table", { name: "Remote ports" })).toContainText(
    "postgres",
  );
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("system listeners remain visible without verbose process details", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  const table = page.getByRole("table", { name: "Remote ports" });
  await expect(table).toContainText("postgres");
  await expect(
    table.getByText("Project directory", { exact: true }),
  ).toHaveCount(0);
  await expect(table).not.toContainText("/var/lib/postgresql");
  await expect(table).not.toContainText("/usr/lib/postgresql/bin/postgres");
  await expect(
    table.getByRole("button", { name: "Forward", exact: true }),
  ).toBeVisible();
});

test("process context highlights application and service details across runtimes", async ({
  page,
}) => {
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    bridge.invoke = (cmd: string, args: any) =>
      cmd === "discover_ports"
        ? Promise.resolve([
            {
              port: 8000,
              address: "0.0.0.0",
              pid: 101,
              user: "deploy",
              processName: "python3",
              executable: "/usr/bin/python3",
              workingDirectory: "/srv/orders",
              arguments: ["python3", "-m", "uvicorn", "orders:app"],
            },
            {
              port: 8080,
              address: "0.0.0.0",
              pid: 102,
              user: "deploy",
              processName: "java",
              executable: "/usr/bin/java",
              workingDirectory: "/",
              arguments: ["java", "-jar", "/opt/orders app.jar"],
            },
            {
              port: 5432,
              address: "127.0.0.1",
              pid: 103,
              user: "postgres",
              processName: "postgres",
              executable: "/usr/bin/postgres",
              workingDirectory: "/",
              arguments: ["postgres", "-D", "/var/lib/postgresql/orders"],
            },
            {
              port: 22,
              address: "0.0.0.0",
              pid: 104,
              user: "root",
              processName: "sshd",
              executable: "/usr/sbin/sshd",
              workingDirectory: "/",
              arguments: ["sshd", "-D"],
            },
          ])
        : original(cmd, args);
  });
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  const table = page.getByRole("table", { name: "Remote ports" });
  await expect(table).toContainText("Module");
  await expect(table).toContainText("uvicorn");
  await expect(table).toContainText("/srv/orders");
  await expect(table).toContainText("/opt/orders app.jar");
  await expect(table).toContainText("/var/lib/postgresql/orders");
  const ssh = table.getByRole("row").filter({ hasText: "sshd" });
  await expect(ssh.locator(".port-project-directory")).toHaveCount(0);
  for (const width of [960, 640]) {
    await page.setViewportSize({ width, height: 680 });
    await table.scrollIntoViewIfNeeded();
    await page.screenshot({ path: `/tmp/porthop-context-${width}.png` });
  }
});

test("connected tunnels show destination failure and recovery independently", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    const callbacks = new Map<number, (event: any) => void>();
    bridge.transformCallback = (callback: (event: any) => void) => {
      const id = callbacks.size + 1;
      callbacks.set(id, callback);
      return id;
    };
    bridge.invoke = (cmd: string, args: any) => {
      if (cmd === "plugin:event|listen") {
        if (args.event === "state-changed")
          (window as any).__stateChanged = callbacks.get(args.handler);
        return Promise.resolve(args.handler);
      }
      if (cmd === "plugin:event|unlisten") return Promise.resolve();
      return original(cmd, args);
    };
  });
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    (window as any).__destination = "unavailable";
    bridge.invoke = async (cmd: string, args: any) => {
      const result = await original(cmd, args);
      if (cmd === "snapshot") {
        const tunnel = result.config.tunnels[0];
        result.runtime.tunnels[tunnel.id] = {
          status: "connected",
          errorMessage: null,
          reconnectAttempt: 0,
        };
        result.runtime.tunnelHealth = {
          [tunnel.id]: [
            {
              localPort: 3000,
              remotePort: 3000,
              status: (window as any).__destination,
              message:
                (window as any).__destination === "unavailable"
                  ? "The server could not connect to this destination."
                  : null,
              checkedAt: 1700000000,
            },
          ],
        };
      }
      if (cmd === "discover_ports")
        return [
          { port: 3000, address: "0.0.0.0", pid: null, processName: null },
        ];
      return result;
    };
  });
  await page.waitForFunction(() => !!(window as any).__stateChanged);
  await page.evaluate(() =>
    (window as any).__stateChanged({
      payload: { instanceId: "fixture", revision: 1 },
    }),
  );
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
  const tunnels = page.getByRole("list", { name: "Tunnels" });
  const table = page.getByRole("table", { name: "Remote ports" });
  await expect(tunnels.getByText("Connected", { exact: true })).toBeVisible();
  await expect(
    tunnels.getByText("Destination unavailable", { exact: true }),
  ).toBeVisible();
  await expect(
    table.getByText("Destination unavailable", { exact: true }),
  ).toBeVisible();
  await tunnels.locator("summary").click();
  await expect(tunnels).toContainText("The server could not connect");
  for (const width of [960, 640]) {
    await page.setViewportSize({ width, height: 680 });
    await page.screenshot({ path: `/tmp/porthop-health-${width}.png` });
  }
  await page.evaluate(() => {
    (window as any).__destination = "reachable";
    (window as any).__stateChanged({
      payload: { instanceId: "fixture", revision: 2 },
    });
  });
  await expect(
    tunnels.getByText("Destination reachable", { exact: true }),
  ).toBeVisible();
  await expect(
    table.getByText("Destination reachable", { exact: true }),
  ).toBeVisible();
  await expect(
    tunnels.getByText("Destination unavailable", { exact: true }),
  ).toHaveCount(0);
});

test("Overview Refresh retries SSH after a cached timeout and remains retryable", async ({
  page,
}) => {
  await page.goto("/?fixture=ssh-timeout");
  const timeout = page.getByText(
    /SSH connection or authentication timed out after 20 seconds/,
  );
  await expect(timeout).toBeVisible();
  await page
    .getByRole("checkbox", { name: "Auto-refresh", exact: true })
    .uncheck();
  const refresh = page.getByRole("button", { name: "Refresh", exact: true });
  await refresh.click();
  await expect
    .poll(() => page.evaluate(() => (window as any).__manualMetricAttempts))
    .toBe(1);
  await expect(timeout).toBeVisible();
  await expect(refresh).toBeEnabled();
  await refresh.click();
  await expect(
    page.getByRole("button", { name: "Refreshing…", exact: true }),
  ).toBeDisabled();
  await expect
    .poll(() => page.evaluate(() => (window as any).__manualMetricAttempts))
    .toBe(2);
  await page.evaluate(() => (window as any).__completeMetricRefresh());
  await expect(timeout).toHaveCount(0);
  await expect(
    page.getByRole("table", { name: "Processes", exact: true }),
  ).toBeVisible();
  await expect(refresh).toBeEnabled();
});

test("agent reinstall preserves switches, prevents repeat clicks and allows retry", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("tab", { name: "Integration", exact: true }).click();
  const clipboard = page.getByRole("switch", {
    name: "Clipboard",
    exact: true,
  });
  const browser = page.getByRole("switch", { name: "Browser", exact: true });
  await browser.click();
  const reinstall = page.getByRole("button", {
    name: "Reinstall agent",
    exact: true,
  });
  await reinstall.click();
  await expect(reinstall).toBeDisabled();
  await expect(reinstall).toHaveAttribute("aria-busy", "true");
  await expect(browser).toBeDisabled();
  await expect(clipboard).toBeDisabled();
  await page.evaluate(() => (window as any).__finishReinstall(true));
  await expect(
    page.getByText("Agent upload failed", { exact: false }),
  ).toBeVisible();
  await expect(reinstall).toBeEnabled();
  await expect(browser).toBeChecked();
  await expect(clipboard).not.toBeChecked();
  await reinstall.click();
  await page.evaluate(() => (window as any).__finishReinstall());
  await expect(
    page.getByText("Agent reinstalled", { exact: true }),
  ).toBeVisible();
  await expect(browser).toBeChecked();
  await expect(clipboard).not.toBeChecked();
  await browser.click();
  await reinstall.click();
  await page.evaluate(() => (window as any).__finishReinstall());
  await expect(reinstall).toBeEnabled();
  await expect(browser).not.toBeChecked();
  await expect(clipboard).not.toBeChecked();
  expect(await page.evaluate(() => (window as any).__reinstallCount)).toBe(3);
});

test("runtime notifications update health and focus reconciles a missed event", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    const callbacks = new Map<number, (event: any) => void>();
    bridge.transformCallback = (callback: (event: any) => void) => {
      const id = callbacks.size + 1;
      callbacks.set(id, callback);
      return id;
    };
    (window as any).__health = "reachable";
    (window as any).__revision = 1;
    bridge.invoke = async (cmd: string, args: any) => {
      if (cmd === "plugin:event|listen") {
        if (args.event === "state-changed")
          (window as any).__stateChanged = callbacks.get(args.handler);
        return args.handler;
      }
      if (cmd === "plugin:event|unlisten") return;
      const result = await original(cmd, args);
      if (cmd === "snapshot") {
        const value = structuredClone(result);
        value.instanceId = "test-instance";
        value.revision = (window as any).__revision;
        for (const id of Object.keys(value.runtime.health))
          value.runtime.health[id] = (window as any).__health;
        return value;
      }
      return result;
    };
  });
  await page.goto("/");
  await expect(page.locator(".toolbar-health")).toHaveText("Reachable");
  await page.waitForFunction(() => !!(window as any).__stateChanged);
  await page.evaluate(() => {
    (window as any).__health = "error";
    (window as any).__revision = 2;
    (window as any).__stateChanged({
      payload: { instanceId: "test-instance", revision: 2 },
    });
  });
  await expect(page.locator(".toolbar-health")).toHaveText("Connection failed");
  await page.evaluate(() => {
    (window as any).__health = "reachable";
    (window as any).__revision = 3;
    window.dispatchEvent(new Event("focus"));
  });
  await expect(page.locator(".toolbar-health")).toHaveText("Reachable");
});

test("Overview retains an old reading with its refresh error and clears it on recovery", async ({
  page,
}) => {
  await page.addInitScript(() => {
    const bridge = (window as any).__TAURI_INTERNALS__;
    const original = bridge.invoke;
    bridge.invoke = async (cmd: string, args: any) => {
      const result = await original(cmd, args);
      if (
        cmd === "cockpit_collect" &&
        args.section === "overview" &&
        !args.refresh
      )
        return {
          ...result,
          sampledAt: Date.now() - 60_000,
          collectionError: "Connection interrupted",
        };
      return result;
    };
  });
  await page.goto("/");
  await expect(
    page.getByText("Connection interrupted", { exact: false }),
  ).toBeVisible();
  await expect(page.locator(".overview-refresh")).toContainText("Last reading");
  await expect(
    page.getByRole("heading", { name: "System health" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await expect(
    page.getByText("Connection interrupted", { exact: false }),
  ).toHaveCount(0);
  await expect(page.locator(".overview-refresh")).toContainText("Updated");
});
