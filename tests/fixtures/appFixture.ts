import type { Page } from "@playwright/test";

const serverId = "a3531c9e-d53d-45ae-990c-fbe204d1a21e";
const tunnelId = "4a0429c7-7092-4c9d-ae8b-b1764985fb52";

/**
 * Installs the mock Tauri backend the UI talks to, with a fixed set of
 * servers, tunnels, terminals, services and metrics. Shared so that both the
 * behavioural suite and the style-regression suite drive the same app state.
 */
export async function installAppFixture(page: Page) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.addInitScript(
    ({ serverId, tunnelId }) => {
      let metricCalls = 0;
      const terminals = new Map<string, any[]>();
      (window as any).__terminalWrites = [];
      (window as any).__terminalClosed = [];
      (window as any).__terminalSizes = [];
      (window as any).__openedUrls = [];

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
          metadata: { currentWindow: { label: "main" } },
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
              (window as any).__activeTerminal = args.session;
              terminals.set(args.session, [
                { type: "ready" },
                {
                  type: "data",
                  data: Array.from(
                    new TextEncoder().encode(
                      "\x1b]0;developer@dev-linux: ~\x07Welcome to Development\r\n\x1b[33mANSI yellow\x1b[0m · \x1b[97mbright white\x1b[0m · \x1b[36mcyan\x1b[0m\r\n\uf179 \uf115 ~ \uf017 01:15:17\r\ndeveloper@dev-linux:~$ ",
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
            if (cmd === "open_url") {
              (window as any).__openedUrls.push(args.url);
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
}
