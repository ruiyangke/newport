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
                  // The agent's last minute per core, one value per second.
                  cpus: [42, 8, 16, 3, 61, 5, 12, 0].map((usage, id) => ({
                    id,
                    usage,
                    history: Array.from({ length: 60 }, (_, t) =>
                      Math.max(
                        0,
                        Math.round(
                          usage +
                            Math.sin((t + id * 7) / 6) * Math.min(usage, 20),
                        ),
                      ),
                    ),
                  })),
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
                        command: `/opt/jobs/worker --shard ${i}`,
                        user: "developer",
                        cpu: 50 - i,
                        memory: 1024 * (i + 1),
                        state: "S",
                      }))
                    : [
                        {
                          pid: 821,
                          name: "node",
                          command: "node /srv/app/server.js --port 3000",
                          user: "developer",
                          cpu: 12.8,
                          memory: 428000000,
                          state: "S",
                        },
                        {
                          pid: 612,
                          name: "postgres",
                          command: "postgres: checkpointer",
                          user: "postgres",
                          cpu: 3.2,
                          memory: 216000000,
                          state: "S",
                        },
                        {
                          pid: 403,
                          name: "sshd",
                          command: "sshd: /usr/sbin/sshd -D [listener]",
                          user: "root",
                          cpu: 0,
                          memory: 12000000,
                          state: "S",
                        },
                      ],
                };
              }
            }
            // Services, as the agent reports them. Invented units; actions
            // change state, and ?fixture=sudo-password makes system actions
            // need a password so the Terminal hand-off can be exercised.
            const svc = ((window as any).__services ??= (() => {
              const now = Date.now();
              const MiB = 1024 ** 2;
              const unit = (name: string, extra: Record<string, unknown>) => ({
                name,
                scope: "system",
                description: "",
                load: "loaded",
                active: "active",
                sub: "running",
                enabled: "enabled",
                mainPid: null,
                since: now - 2 * 86400000,
                memory: null,
                cpu: null,
                restarts: 0,
                result: "success",
                exitCode: null,
                exitKind: null,
                fragment: `/lib/systemd/system/${name}`,
                dropIns: [],
                triggeredBy: [],
                user: null,
                ...extra,
              });
              return {
                privilege: location.search.includes("fixture=sudo-password")
                  ? "password"
                  : "sudo",
                services: [
                  unit("nginx.service", {
                    description: "A high performance web server",
                    mainPid: 1201,
                    memory: 42 * MiB,
                    cpu: 0.4,
                    user: "www-data",
                  }),
                  unit("postgresql@16-main.service", {
                    description: "PostgreSQL Cluster 16-main",
                    mainPid: 980,
                    memory: 1200 * MiB,
                    cpu: 1.8,
                    user: "postgres",
                    since: now - 9 * 86400000,
                  }),
                  unit("atlas-api.service", {
                    description: "Atlas API",
                    active: "failed",
                    sub: "failed",
                    result: "exit-code",
                    exitCode: 1,
                    exitKind: "exited",
                    restarts: 3,
                    since: now - 180000,
                    fragment: "/etc/systemd/system/atlas-api.service",
                    dropIns: [
                      "/etc/systemd/system/atlas-api.service.d/env.conf",
                    ],
                    user: "atlas",
                  }),
                  unit("atlas-worker.service", {
                    description: "Atlas background worker",
                    active: "activating",
                    sub: "auto-restart",
                    result: "exit-code",
                    exitCode: 2,
                    exitKind: "exited",
                    restarts: 14,
                    since: now - 4000,
                    fragment: "/etc/systemd/system/atlas-worker.service",
                    user: "atlas",
                  }),
                  unit("redis-server.service", {
                    description: "Advanced key-value store",
                    mainPid: 777,
                    memory: 18 * MiB,
                    cpu: 0.2,
                    user: "redis",
                  }),
                  unit("certbot.service", {
                    description: "Certbot",
                    active: "inactive",
                    sub: "dead",
                    enabled: "static",
                    since: now - 9 * 3600000,
                    triggeredBy: ["certbot.timer"],
                  }),
                  unit("ssh.service", {
                    description: "OpenBSD Secure Shell server",
                    mainPid: 700,
                    memory: 6 * MiB,
                  }),
                  unit("cron.service", {
                    description: "Regular background program processing daemon",
                    mainPid: 650,
                  }),
                  unit("systemd-journald.service", {
                    description: "Journal Service",
                    enabled: "static",
                    mainPid: 300,
                  }),
                  unit("getty@tty1.service", {
                    description: "Getty on tty1",
                    mainPid: 820,
                  }),
                  unit("apt-daily.service", {
                    description: "Daily apt download activities",
                    active: "inactive",
                    sub: "dead",
                    enabled: "static",
                    triggeredBy: ["apt-daily.timer"],
                  }),
                  unit("dev-tunnel.service", {
                    scope: "user",
                    description: "Personal dev tunnel",
                    mainPid: 5100,
                    memory: 9 * MiB,
                    fragment:
                      "/home/ci/.config/systemd/user/dev-tunnel.service",
                    enabled: "enabled",
                  }),
                ],
                timers: [
                  {
                    name: "certbot.timer",
                    scope: "system",
                    description: "Run certbot twice daily",
                    active: "active",
                    next: now + 3 * 3600000,
                    last: now - 9 * 3600000,
                    unit: "certbot.service",
                  },
                  {
                    name: "apt-daily.timer",
                    scope: "system",
                    description: "Daily apt download activities",
                    active: "active",
                    next: now + 14 * 3600000,
                    last: now - 10 * 3600000,
                    unit: "apt-daily.service",
                  },
                ],
                tail: 0,
                actions: [] as unknown[],
              };
            })());
            if (
              cmd === "services_snapshot" &&
              location.search.includes("fixture=long-lists")
            )
              svc.services[2].name =
                "atlas-" + "long-name-".repeat(16) + ".service";
            if (
              cmd === "services_snapshot" &&
              location.search.includes("fixture=large-lists")
            ) {
              // 500 of the administrator's own units; after the first reading
              // one more appears at the top, as when a unit is installed.
              const offset = largeServiceCalls++ ? -1 : 0;
              svc.services = svc.services
                .filter((u: any) => !u.name.startsWith("worker-"))
                .concat(
                  Array.from({ length: 500 }, (_, i) => ({
                    ...svc.services[0],
                    name: `worker-${String(i + offset + 1).padStart(4, "0")}.service`,
                    description: `Background worker ${i + offset + 1}`,
                    fragment: "/etc/systemd/system/worker@.service",
                  })),
                );
            }
            if (cmd === "services_snapshot")
              return structuredClone({
                services: svc.services,
                timers: svc.timers,
                user: { available: true, reason: null },
                privilege: svc.privilege,
              });
            if (cmd === "service_logs") {
              const base = Date.now() - 60000;
              const entry = (i: number, priority: number, message: string) => ({
                at: base + i * 1000,
                priority,
                pid: 4242,
                identifier: (args.unit ?? "systemd").replace(/\.service$/, ""),
                message,
              });
              if (args.cursor) {
                svc.tail++;
                return {
                  entries: [entry(60 + svc.tail, 6, `live line ${svc.tail}`)],
                  cursor: `c${svc.tail}`,
                };
              }
              // ?fixture=long-logs adds yesterday's traffic: enough lines to
              // scroll, and a day boundary.
              const history = location.search.includes("fixture=long-logs")
                ? Array.from({ length: 120 }, (_, k) =>
                    entry(k - 86_400, 6, `GET /health 200 (${k})`),
                  )
                : [];
              const lines = [
                ...history,
                entry(0, 6, "Starting service…"),
                entry(1, 6, "Listening on 127.0.0.1:9000"),
                entry(2, 4, "Slow query took 1.8 s"),
                entry(3, 3, "database connection refused"),
                entry(
                  4,
                  3,
                  `${args.unit ?? "unit"}: Failed with result 'exit-code'.`,
                ),
              ];
              const max =
                args.priority === "err"
                  ? 3
                  : args.priority === "warning"
                    ? 4
                    : 7;
              return {
                entries: lines.filter((l) => l.priority <= max),
                cursor: "c0",
              };
            }
            if (cmd === "service_unit_file")
              return {
                text: `# /etc/systemd/system/${args.unit}\n[Unit]\nDescription=${args.unit}\nAfter=network.target\n\n[Service]\nExecStart=/usr/local/bin/${args.unit.replace(/\.(service|timer)$/, "")}\nRestart=on-failure\n\n[Install]\nWantedBy=multi-user.target\n`,
              };
            if (cmd === "service_action") {
              svc.actions.push(args);
              if (args.scope === "system" && svc.privilege === "password")
                return {
                  ok: false,
                  needsPassword: true,
                  command: `sudo systemctl ${args.verb} ${args.unit}`,
                };
              const target = svc.services.find(
                (u: any) => u.name === args.unit && u.scope === args.scope,
              );
              if (!target)
                throw `Failed to ${args.verb} ${args.unit}: Unit not found.`;
              const running = {
                active: "active",
                sub: "running",
                result: "success",
                exitCode: null,
                exitKind: null,
                since: Date.now(),
              };
              if (args.verb === "start" || args.verb === "restart")
                Object.assign(target, running, {
                  mainPid: 9000 + svc.actions.length,
                });
              if (args.verb === "stop")
                Object.assign(target, {
                  active: "inactive",
                  sub: "dead",
                  mainPid: null,
                  since: Date.now(),
                });
              if (args.verb === "enable") target.enabled = "enabled";
              if (args.verb === "disable") target.enabled = "disabled";
              return { ok: true };
            }
            // Docker, as the agent reports it: an invented host with two
            // Compose projects in trouble and a few standalone containers.
            // Actions change state. ?fixture=no-docker denies the socket;
            // ?fixture=large-lists has 500 containers.
            const docker = ((window as any).__docker ??= (() => {
              const now = Date.now();
              const MiB = 1024 ** 2;
              const hex = (seed: string) =>
                Array.from({ length: 64 }, (_, i) =>
                  ((seed.charCodeAt(i % seed.length) + i) % 16).toString(16),
                ).join("");
              const box = (
                name: string,
                extra: Record<string, any> = {},
              ): Record<string, any> => ({
                id: hex(name),
                name,
                image: "example/app:1",
                imageId: `sha256:${hex(`img${name}`)}`,
                command: "app",
                state: "running",
                status: "Up 2 hours",
                health: null,
                exitCode: null,
                oomKilled: false,
                error: null,
                restartCount: 0,
                restartPolicy: "unless-stopped",
                created: now - 3 * 86400000,
                startedAt: now - 2 * 3600000,
                finishedAt: null,
                ports: [],
                compose: null,
                cpu: 1.2,
                memory: 64 * MiB,
                memoryLimit: null,
                ...extra,
              });
              const compose = (
                project: string,
                service: string,
                workingDir: string,
                oneoff = false,
              ) => ({
                project,
                service,
                oneoff,
                number: oneoff ? null : 1,
                workingDir,
                configFiles: `${workingDir}/compose.yaml`,
              });
              const down = (code: number, ago: number) => ({
                state: "exited",
                status: `Exited (${code}) ${Math.round(ago / 60000)} minutes ago`,
                exitCode: code,
                startedAt: null,
                finishedAt: now - ago,
                cpu: null,
                memory: null,
              });
              const large = location.search.includes("fixture=large-lists");
              const long = location.search.includes("fixture=long-lists");
              const containers = large
                ? Array.from({ length: 500 }, (_, i) =>
                    box(`worker-${String(i).padStart(4, "0")}`, {
                      image: "worker:latest",
                    }),
                  )
                : [
                    box("atlas-api-1", {
                      image: "ghcr.io/example/atlas-api:2.4.1",
                      health: "healthy",
                      status: "Up 2 hours (healthy)",
                      compose: compose("atlas", "api", "/srv/atlas"),
                      ports: [
                        {
                          ip: "0.0.0.0",
                          hostPort: 8080,
                          containerPort: 8080,
                          protocol: "tcp",
                        },
                        {
                          ip: "::",
                          hostPort: 8080,
                          containerPort: 8080,
                          protocol: "tcp",
                        },
                      ],
                      cpu: 3.2,
                      memory: 180 * MiB,
                      memoryLimit: 512 * MiB,
                    }),
                    box(
                      long
                        ? `atlas-${"long-name-".repeat(12)}worker-1`
                        : "atlas-worker-1",
                      {
                        image: "ghcr.io/example/atlas-api:2.4.1",
                        health: "unhealthy",
                        status: "Up 2 hours (unhealthy)",
                        restartCount: 2,
                        compose: compose(
                          "atlas",
                          long ? `${"long-name-".repeat(12)}worker` : "worker",
                          "/srv/atlas",
                        ),
                        cpu: 41.3,
                        memory: 212 * MiB,
                      },
                    ),
                    box("atlas-db-1", {
                      image: "postgres:16",
                      health: "healthy",
                      status: "Up 3 days (healthy)",
                      startedAt: now - 3 * 86400000,
                      compose: compose("atlas", "db", "/srv/atlas"),
                      ports: [
                        {
                          ip: null,
                          hostPort: null,
                          containerPort: 5432,
                          protocol: "tcp",
                        },
                      ],
                      cpu: 0.8,
                      memory: 96 * MiB,
                    }),
                    box("atlas-migrate-run-8f2c", {
                      image: "ghcr.io/example/atlas-api:2.4.1",
                      created: now - 2 * 3600000,
                      compose: compose("atlas", "migrate", "/srv/atlas", true),
                      ...down(0, 2 * 3600000),
                    }),
                    box("ledger-web-1", {
                      image: "ledger-web:dev",
                      oomKilled: true,
                      restartPolicy: "no",
                      compose: compose("ledger", "web", "/home/ci/ledger"),
                      ports: [
                        {
                          ip: "127.0.0.1",
                          hostPort: 3000,
                          containerPort: 3000,
                          protocol: "tcp",
                        },
                      ],
                      ...down(137, 5 * 60000),
                    }),
                    box("ledger-redis-1", {
                      image: "redis:7-alpine",
                      compose: compose("ledger", "redis", "/home/ci/ledger"),
                      ports: [
                        {
                          ip: null,
                          hostPort: null,
                          containerPort: 6379,
                          protocol: "tcp",
                        },
                      ],
                      cpu: 0.3,
                      memory: 12 * MiB,
                    }),
                    box("ledger-queue-1", {
                      image: "ledger-web:dev",
                      state: "restarting",
                      status: "Restarting (1) 4 seconds ago",
                      exitCode: 1,
                      restartCount: 6,
                      restartPolicy: "always",
                      compose: compose("ledger", "queue", "/home/ci/ledger"),
                      cpu: null,
                      memory: null,
                    }),
                    box("portainer", {
                      image: "portainer/portainer-ce:2.21",
                      startedAt: now - 12 * 86400000,
                      ports: [
                        {
                          ip: "0.0.0.0",
                          hostPort: 9443,
                          containerPort: 9443,
                          protocol: "tcp",
                        },
                      ],
                      cpu: 0.1,
                      memory: 40 * MiB,
                    }),
                    box("buildkit", {
                      image: "moby/buildkit:buildx-stable-1",
                      created: now - 21 * 86400000,
                      ...down(0, 21 * 86400000),
                    }),
                  ];
              return {
                containers,
                actions: [] as unknown[],
                tail: {} as Record<string, number>,
                images: [
                  {
                    id: "sha256:a1",
                    tags: ["ghcr.io/example/atlas-api:2.4.1"],
                    size: 412 * MiB,
                    sharedSize: null,
                    created: now - 86400000,
                    containers: 3,
                    dangling: false,
                  },
                  {
                    id: "sha256:b2",
                    tags: ["postgres:16"],
                    size: 438 * MiB,
                    sharedSize: null,
                    created: now - 20 * 86400000,
                    containers: 1,
                    dangling: false,
                  },
                  {
                    id: "sha256:c3",
                    tags: ["ghcr.io/example/atlas-api:2.3.0"],
                    size: 405 * MiB,
                    sharedSize: null,
                    created: now - 30 * 86400000,
                    containers: 0,
                    dangling: false,
                  },
                  {
                    id: "sha256:d4e5f6a7b8c9d0e1",
                    tags: [],
                    size: 220 * MiB,
                    sharedSize: null,
                    created: now - 9 * 86400000,
                    containers: 0,
                    dangling: true,
                  },
                ],
                volumes: [
                  {
                    name: "atlas_pgdata",
                    driver: "local",
                    size: 1.6 * 1024 * MiB,
                    refCount: 1,
                    anonymous: false,
                    compose: "atlas",
                  },
                  {
                    name: hex("anon"),
                    driver: "local",
                    size: 88 * MiB,
                    refCount: 0,
                    anonymous: true,
                    compose: null,
                  },
                ],
                buildCache: {
                  count: 34,
                  size: 1.1 * 1024 * MiB,
                  reclaimable: 900 * MiB,
                },
              };
            })());
            const byRef = (ref: string) =>
              docker.containers.find(
                (x: any) => x.id === ref || x.name === ref,
              );
            if (
              cmd.startsWith("container") ||
              cmd === "project_action" ||
              cmd.startsWith("docker_")
            ) {
              if (location.search.includes("fixture=no-docker"))
                throw "This account can't use Docker: permission denied on /var/run/docker.sock. Add it to the docker group, or use rootless Docker.";
            }
            if (cmd === "containers_snapshot") {
              for (const x of docker.containers)
                if (x.state === "running" && x.cpu !== null)
                  x.cpu = Math.max(0, x.cpu * (0.85 + Math.random() * 0.3));
              return structuredClone({
                engine: {
                  version: "27.3.1",
                  apiVersion: "1.47",
                  os: "linux",
                  rootless: false,
                  podman: false,
                },
                containers: docker.containers,
                truncated: false,
              });
            }
            if (cmd === "container_inspect") {
              const x = byRef(args.container);
              if (!x) throw `No such container: ${args.container}`;
              return {
                id: x.id,
                name: x.name,
                image: x.image,
                created: x.created,
                entrypoint: ["/usr/local/bin/entrypoint"],
                cmd: ["serve", "--port", "8080"],
                workingDir: "/app",
                user: "app",
                hostname: x.id.slice(0, 12),
                env: [
                  { name: "PATH", value: "/usr/local/bin:/usr/bin:/bin" },
                  { name: "APP_ENV", value: "production" },
                  { name: "DATABASE_HOST", value: "db" },
                  { name: "POSTGRES_PASSWORD", value: "example-only" },
                ],
                mounts: [
                  {
                    type: "volume",
                    source: "/var/lib/docker/volumes/atlas_pgdata/_data",
                    destination: "/data",
                    mode: "z",
                    rw: true,
                    name: "atlas_pgdata",
                  },
                  {
                    type: "bind",
                    source: "/srv/atlas/config",
                    destination: "/app/config",
                    mode: "",
                    rw: false,
                    name: null,
                  },
                ],
                networks: [
                  {
                    name: "atlas_default",
                    ipAddress: "172.18.0.3",
                    aliases: [x.compose?.service ?? x.name],
                  },
                ],
                restartPolicy: x.restartPolicy,
                labels: x.compose
                  ? {
                      "com.docker.compose.project": x.compose.project,
                      "com.docker.compose.service": x.compose.service,
                    }
                  : {},
                healthcheck: x.health
                  ? {
                      test: ["CMD-SHELL", "curl -fsS localhost:8081/health"],
                      interval: 30000,
                    }
                  : null,
                lastHealth:
                  x.health === "unhealthy"
                    ? [1, 2, 3].map((i) => ({
                        at: Date.now() - (4 - i) * 30000,
                        exitCode: 7,
                        output:
                          "curl: (7) Failed to connect to localhost port 8081: Connection refused",
                      }))
                    : x.health
                      ? [{ at: Date.now() - 30000, exitCode: 0, output: "ok" }]
                      : [],
              };
            }
            if (cmd === "container_logs") {
              const members = args.project
                ? docker.containers.filter(
                    (x: any) => x.compose?.project === args.project,
                  )
                : [byRef(args.container)].filter(Boolean);
              if (!members.length) throw "No such container";
              const key = args.project
                ? `p:${args.project}`
                : `c:${args.container}`;
              const base = Date.now() - 120000;
              const line = (
                x: any,
                i: number,
                message: string,
                stream = "stdout",
              ) => ({
                at: base + i * 1000,
                stream,
                message,
                container: args.project ? (x.compose?.service ?? x.name) : null,
              });
              if (args.cursor) {
                const n = (docker.tail[key] = (docker.tail[key] ?? 0) + 1);
                const x = members[n % members.length];
                return {
                  entries: [line(x, 100 + n, `GET /health 200 (${n})`)],
                  cursor: `${key}:${n}`,
                };
              }
              const long = location.search.includes("fixture=long-logs");
              const entries = members.flatMap((x: any, k: number) => [
                ...(long
                  ? Array.from({ length: 120 }, (_, j) =>
                      line(x, j - 86_400 + k, `GET /api/items 200 ${j}ms`),
                    )
                  : []),
                line(
                  x,
                  k * 4,
                  `${x.compose?.service ?? x.name} listening on :8080`,
                ),
                line(x, k * 4 + 1, "WARN slow query took 1.8s"),
                line(
                  x,
                  k * 4 + 2,
                  "ERROR connection refused to db:5432",
                  "stderr",
                ),
                line(x, k * 4 + 3, "GET /health 200"),
              ]);
              entries.sort((a: any, b: any) => a.at - b.at);
              return { entries, cursor: `${key}:0` };
            }
            if (cmd === "container_action") {
              docker.actions.push(args);
              const x = byRef(args.container);
              if (!x) throw `No such container: ${args.container}`;
              const verb = args.verb;
              if (verb === "start" && x.state === "running")
                throw `Container ${x.name} is already running`;
              const up = {
                state: "running",
                status: "Up 1 second",
                health: x.health ? "starting" : null,
                exitCode: null,
                oomKilled: false,
                startedAt: Date.now(),
                finishedAt: null,
                cpu: 0.5,
                memory: 30 * 1024 ** 2,
              };
              if (verb === "start" || verb === "restart" || verb === "unpause")
                Object.assign(x, up);
              if (verb === "stop")
                Object.assign(x, {
                  state: "exited",
                  status: "Exited (0) 1 second ago",
                  exitCode: 0,
                  health: x.health ? "unhealthy" : null,
                  startedAt: null,
                  finishedAt: Date.now(),
                  cpu: null,
                  memory: null,
                });
              if (verb === "pause")
                Object.assign(x, {
                  state: "paused",
                  status: "Up 2 hours (Paused)",
                });
              return { ok: true };
            }
            if (cmd === "project_action") {
              docker.actions.push(args);
              const ids = docker.containers
                .filter(
                  (x: any) =>
                    x.compose?.project === args.project && !x.compose.oneoff,
                )
                .map((x: any) => x.id)
                .sort();
              if (
                JSON.stringify(ids) !==
                JSON.stringify([...args.expectedIds].sort())
              )
                throw "Project containers changed since you opened it. Refresh and review the project again.";
              for (const x of docker.containers)
                if (ids.includes(x.id))
                  Object.assign(
                    x,
                    args.verb === "stop"
                      ? {
                          state: "exited",
                          status: "Exited (0) 1 second ago",
                          exitCode: 0,
                          oomKilled: false,
                          startedAt: null,
                          finishedAt: Date.now(),
                          cpu: null,
                          memory: null,
                        }
                      : {
                          state: "running",
                          status: "Up 1 second",
                          health: x.health ? "healthy" : null,
                          exitCode: null,
                          oomKilled: false,
                          startedAt: Date.now(),
                          finishedAt: null,
                          cpu: 0.5,
                          memory: 30 * 1024 ** 2,
                        },
                  );
              return { ok: true };
            }
            if (cmd === "containers_remove") {
              docker.actions.push(args);
              const targets = args.containers.map(byRef);
              const running = targets.find(
                (x: any) =>
                  x && ["running", "restarting", "paused"].includes(x.state),
              );
              if (running) throw `${running.name} is running. Stop it first.`;
              docker.containers = docker.containers.filter(
                (x: any) => !targets.includes(x),
              );
              return { removed: targets.filter(Boolean).length };
            }
            if (cmd === "docker_disk") {
              const stopped = docker.containers.filter(
                (x: any) => x.state === "exited",
              );
              const sum = (xs: any[], f: (x: any) => number) =>
                xs.reduce((n, x) => n + f(x), 0);
              return structuredClone({
                images: docker.images,
                volumes: docker.volumes,
                totals: {
                  images: {
                    count: docker.images.length,
                    size: sum(docker.images, (i) => i.size),
                    reclaimable: sum(
                      docker.images.filter((i: any) => !i.containers),
                      (i) => i.size,
                    ),
                  },
                  containers: {
                    count: docker.containers.length,
                    size: docker.containers.length * 8 * 1024 ** 2,
                    reclaimable: stopped.length * 8 * 1024 ** 2,
                  },
                  volumes: {
                    count: docker.volumes.length,
                    size: sum(docker.volumes, (v) => v.size ?? 0),
                    reclaimable: sum(
                      docker.volumes.filter((v: any) => !v.refCount),
                      (v) => v.size ?? 0,
                    ),
                  },
                  buildCache: docker.buildCache,
                },
              });
            }
            // Compose, as the agent runs it: jobs whose output arrives a
            // line per poll and that change the containers when they finish.
            // ?fixture=no-compose has no CLI; ?fixture=compose-fail fails jobs.
            const compose = ((docker as any).compose ??= {
              jobs: [] as any[],
              services: {
                atlas: [
                  { name: "api", image: null, build: true, profiles: [] },
                  {
                    name: "db",
                    image: "postgres:16",
                    build: false,
                    profiles: [],
                  },
                  {
                    name: "docs",
                    image: "nginx:1.27",
                    build: false,
                    profiles: ["docs"],
                  },
                  { name: "worker", image: null, build: true, profiles: [] },
                ],
                ledger: [
                  { name: "queue", image: null, build: true, profiles: [] },
                  {
                    name: "redis",
                    image: "redis:7-alpine",
                    build: false,
                    profiles: [],
                  },
                  { name: "web", image: null, build: true, profiles: [] },
                ],
              } as Record<string, any[]>,
            });
            const members = (project: string) =>
              docker.containers.filter(
                (x: any) => x.compose?.project === project,
              );
            const runningUp = (x: any) =>
              Object.assign(x, {
                state: "running",
                status: "Up 1 second",
                health: x.health ? "healthy" : null,
                exitCode: null,
                oomKilled: false,
                startedAt: Date.now(),
                finishedAt: null,
                cpu: 0.4,
                memory: 24 * 1024 ** 2,
                restartCount: 0,
              });
            const finish = (job: any) => {
              const a = job.action;
              const project = job.project;
              const template = members(project)[0];
              const create = (service: string, n = 1) => {
                const name = `${project}-${service}-${n}`;
                docker.containers.push(
                  runningUp({
                    ...structuredClone(template),
                    id: `${name}`
                      .padEnd(64, "f")
                      .replace(/[^0-9a-f]/g, "e")
                      .slice(0, 64),
                    name,
                    image:
                      compose.services[project]?.find(
                        (s: any) => s.name === service,
                      )?.image ?? `${project}-${service}`,
                    health: null,
                    ports: [],
                    compose: {
                      ...template.compose,
                      service,
                      oneoff: false,
                      number: n,
                    },
                  }),
                );
              };
              if (a.kind === "up") {
                const wanted = a.services.length
                  ? a.services
                  : compose.services[project]
                      .filter((s: any) => !s.profiles.length)
                      .map((s: any) => s.name);
                for (const service of wanted) {
                  const existing = members(project).filter(
                    (x: any) =>
                      x.compose.service === service && !x.compose.oneoff,
                  );
                  if (!existing.length) create(service);
                  for (const x of existing) {
                    if (a.forceRecreate) x.id = x.id.slice(1) + x.id[0];
                    if (a.forceRecreate || x.state !== "running") runningUp(x);
                  }
                }
              }
              if (a.kind === "down") {
                docker.containers = docker.containers.filter(
                  (x: any) => x.compose?.project !== project,
                );
                if (a.volumes)
                  docker.volumes = docker.volumes.filter(
                    (v: any) => v.compose !== project,
                  );
              }
              if (a.kind === "scale") {
                const existing = members(project).filter(
                  (x: any) =>
                    x.compose.service === a.service && !x.compose.oneoff,
                );
                for (let n = existing.length + 1; n <= a.replicas; n++)
                  create(a.service, n);
                const extra = existing.filter(
                  (x: any) => (x.compose.number ?? 1) > a.replicas,
                );
                docker.containers = docker.containers.filter(
                  (x: any) => !extra.includes(x),
                );
              }
            };
            if (
              cmd.startsWith("compose_") &&
              location.search.includes("fixture=no-docker")
            )
              throw "This account can't use Docker: permission denied on /var/run/docker.sock. Add it to the docker group, or use rootless Docker.";
            if (cmd === "compose_project") {
              const list = members(args.project);
              if (!list.length)
                throw `No containers remain for ${args.project}, so Newport can't find its compose files.`;
              const wd = list[0].compose.workingDir;
              const services = compose.services[args.project] ?? [];
              const noCli = location.search.includes("fixture=no-compose");
              return {
                cli: noCli ? null : { kind: "plugin", version: "2.29.7" },
                cliError: noCli
                  ? "The Compose CLI isn't installed on this server."
                  : null,
                workingDir: wd,
                configFiles: [`${wd}/compose.yaml`],
                files: [
                  {
                    path: `${wd}/compose.yaml`,
                    text:
                      "services:\n" +
                      services
                        .map(
                          (svc: any) =>
                            `  ${svc.name}:\n` +
                            (svc.build
                              ? "    build: .\n"
                              : `    image: ${svc.image}\n`) +
                            (svc.profiles.length
                              ? `    profiles: [${svc.profiles.join(", ")}]\n`
                              : ""),
                        )
                        .join(""),
                    error: null,
                  },
                ],
                services,
                configError: location.search.includes("fixture=compose-error")
                  ? 'service "api" refers to undefined volume cache: invalid compose project'
                  : null,
              };
            }
            if (cmd === "compose_run") {
              compose.calls = [...(compose.calls ?? []), args];
              if (
                compose.jobs.some(
                  (j: any) =>
                    j.project === args.project && j.state === "running",
                )
              )
                throw `Another Compose command is running for ${args.project}.`;
              const a = args.action;
              const flags =
                a.kind === "up"
                  ? [
                      "up",
                      "-d",
                      ...(a.build ? ["--build"] : []),
                      ...(a.pull ? ["--pull", "always"] : []),
                      ...(a.forceRecreate ? ["--force-recreate"] : []),
                      ...(a.services.length
                        ? ["--no-deps", ...a.services]
                        : []),
                    ]
                  : a.kind === "down"
                    ? [
                        "down",
                        ...(a.volumes ? ["--volumes"] : []),
                        ...(a.removeOrphans ? ["--remove-orphans"] : []),
                      ]
                    : a.kind === "scale"
                      ? [
                          "up",
                          "-d",
                          "--no-deps",
                          "--scale",
                          `${a.service}=${a.replicas}`,
                          a.service,
                        ]
                      : a.kind === "build"
                        ? [
                            "build",
                            ...(a.noCache ? ["--no-cache"] : []),
                            ...a.services,
                          ]
                        : ["pull", ...a.services];
              const job = {
                job: `job-${compose.jobs.length + 1}`,
                project: args.project,
                command: `docker compose -p ${args.project} ${flags.join(" ")}`,
                state: "running",
                exitCode: null,
                startedAt: Date.now(),
                finishedAt: null,
                action: a,
                planned: [
                  ` Network ${args.project}_default  Created`,
                  a.kind === "pull" || (a.kind === "up" && a.pull)
                    ? " api Pulling"
                    : a.kind === "build" || (a.kind === "up" && a.build)
                      ? "#5 [api 2/4] RUN npm ci"
                      : " Container started",
                  "WARN[0000] the attribute `version` is obsolete",
                  location.search.includes("fixture=compose-fail")
                    ? "Error response from daemon: driver failed programming external connectivity"
                    : " Done",
                ],
                text: "",
                shown: 0,
              };
              compose.jobs.unshift(job);
              return { job: job.job };
            }
            // Jobs run on their own, as on the server: a line every 0.6 s.
            const advance = (j: any) => {
              if (j.state !== "running") return;
              const due = Math.floor((Date.now() - j.startedAt) / 600);
              while (j.shown < due && j.planned.length) {
                j.text += `${j.planned.shift()}\n`;
                j.shown++;
              }
              if (!j.planned.length) {
                const failed = location.search.includes("fixture=compose-fail");
                j.state = failed ? "failed" : "succeeded";
                j.exitCode = failed ? 1 : 0;
                j.finishedAt = Date.now();
                if (!failed) finish(j);
              }
            };
            const publicJob = (j: any, offset: number) => ({
              job: j.job,
              project: j.project,
              command: j.command,
              state: j.state,
              exitCode: j.exitCode,
              startedAt: j.startedAt,
              finishedAt: j.finishedAt,
              output: j.text.slice(offset),
              offset: j.text.length,
            });
            if (cmd === "compose_job") {
              const j = compose.jobs.find((x: any) => x.job === args.job);
              if (!j)
                throw "That Compose command is no longer known; the agent may have restarted.";
              advance(j);
              return publicJob(j, args.offset);
            }
            if (cmd === "compose_cancel") {
              const j = compose.jobs.find((x: any) => x.job === args.job);
              if (j && j.state === "running")
                Object.assign(j, {
                  state: "cancelled",
                  exitCode: 130,
                  finishedAt: Date.now(),
                  planned: [],
                });
              return { ok: true };
            }
            if (cmd === "compose_jobs") {
              compose.jobs.forEach(advance);
              return {
                jobs: compose.jobs.map((j: any) => ({
                  ...publicJob(j, 0),
                  output: "",
                  offset: 0,
                })),
              };
            }
            if (cmd === "docker_prune") {
              docker.actions.push(args);
              const take = (
                keep: (x: any) => boolean,
                list: any[],
                size: (x: any) => number,
              ) => {
                const gone = list.filter((x) => !keep(x));
                return {
                  kept: list.filter(keep),
                  removed: gone.length,
                  reclaimed: gone.reduce((n, x) => n + size(x), 0),
                };
              };
              let result = { removed: 0, reclaimed: 0 };
              if (args.what === "containers") {
                const r = take(
                  (x) => x.state !== "exited",
                  docker.containers,
                  () => 8 * 1024 ** 2,
                );
                docker.containers = r.kept;
                result = r;
              } else if (
                args.what === "dangling-images" ||
                args.what === "unused-images"
              ) {
                const r = take(
                  (i) =>
                    args.what === "dangling-images"
                      ? !i.dangling
                      : i.containers > 0,
                  docker.images,
                  (i) => i.size,
                );
                docker.images = r.kept;
                result = r;
              } else if (args.what === "volumes") {
                const r = take(
                  (v) => !(v.anonymous && !v.refCount),
                  docker.volumes,
                  (v) => v.size ?? 0,
                );
                docker.volumes = r.kept;
                result = r;
              } else if (args.what === "build-cache") {
                result = {
                  removed: docker.buildCache.count,
                  reclaimed: docker.buildCache.reclaimable,
                };
                docker.buildCache = {
                  count: 0,
                  size: docker.buildCache.size - docker.buildCache.reclaimable,
                  reclaimable: 0,
                };
              }
              return { removed: result.removed, reclaimed: result.reclaimed };
            }
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
            if (cmd === "open_tunnel") {
              ((window as any).__openedTunnels ??= []).push(args.id);
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
            // Port forwarding scenes live in tests/fixtures/ports.ts.
            if (cmd === "git_projects_list") return [];
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
