import type { Page } from "@playwright/test";
import { installAppFixture } from "./appFixture";

/**
 * Port forwarding scenes for the UI suite. Every name and path here is
 * invented: an "atlas" repository with a linked worktree, a "ledger" Compose
 * project, a "notes" project without Git, ordinary system services, and a dev
 * server whose folder was deleted. The real-server behaviour behind this data
 * is covered by `npm run test:remote` (src-tauri/src/remote_tests/ports.rs).
 */
const HOME = "/home/ci";
const ATLAS = `${HOME}/work/atlas`;
const SEARCH = `${HOME}/work/atlas-search`;

type Port = Record<string, unknown> & { port: number; address: string };
const listener = (port: number, extra: Record<string, unknown>): Port => ({
  port,
  address: "127.0.0.1",
  processName: null,
  pid: null,
  ...extra,
});
const checkout = (path: string, branch: string, main: boolean) => ({
  projectRoot: ATLAS,
  projectMarker: ".git",
  worktreePath: path,
  worktreeBranch: branch,
  worktreeMain: main,
});
const node = (pid: number, cwd: string, tool: string) => ({
  pid,
  user: "ci",
  processName: "node",
  executable: "/usr/bin/node",
  workingDirectory: cwd,
  arguments: ["node", `${cwd}/node_modules/.bin/${tool}`],
});
const uvicorn = (pid: number, cwd: string) => ({
  pid,
  user: "ci",
  processName: "python3",
  executable: "/usr/bin/python3.12",
  workingDirectory: cwd,
  arguments: ["python3", "-m", "uvicorn", "main:app"],
});
const compose = (service: string, image: string) => ({
  containerName: `ledger-${service}-1`,
  image,
  composeProject: "ledger",
  composeService: service,
  composeWorkingDir: "/srv/ledger",
  projectRoot: "/srv/ledger",
  projectMarker: "compose",
});

export const listeners: Port[] = [
  listener(4173, {
    ...node(201, `${ATLAS}/app`, "vite"),
    ...checkout(ATLAS, "main", true),
  }),
  // One service on both IPv4 and IPv6 is one row.
  listener(9000, {
    ...uvicorn(202, `${ATLAS}/api`),
    address: "0.0.0.0",
    ...checkout(ATLAS, "main", true),
  }),
  listener(9000, {
    ...uvicorn(202, `${ATLAS}/api`),
    address: "::",
    ...checkout(ATLAS, "main", true),
  }),
  listener(4174, {
    ...node(203, `${SEARCH}/app`, "vite"),
    ...checkout(SEARCH, "feat/search", false),
  }),
  listener(9001, {
    ...uvicorn(204, `${SEARCH}/api`),
    ...checkout(SEARCH, "feat/search", false),
  }),
  listener(5432, compose("db", "postgres:16")),
  listener(6379, compose("cache", "redis:7")),
  listener(3030, {
    ...node(205, `${HOME}/work/notes`, "next"),
    projectRoot: `${HOME}/work/notes`,
    projectMarker: "package.json",
  }),
  listener(22, {
    address: "0.0.0.0",
    pid: 10,
    processName: "sshd",
    executable: "/usr/sbin/sshd",
  }),
  // Without root, the server may not say who owns a socket.
  listener(8443, { address: "0.0.0.0" }),
  listener(7070, {
    pid: 206,
    user: "ci",
    processName: "node",
    applicationName: "node · preview",
    executable: "/usr/bin/node",
    workingDirectory: "/tmp/ci-build/0f3a/worktrees/preview",
    workingDirectoryDeleted: true,
    arguments: ["node", "server.js"],
  }),
];

export const savedProjects = [
  { id: "saved-atlas", name: "Atlas", path: { bytesB64: "", display: ATLAS } },
];

export type Scene = {
  /** Discovery's answer; an Error rejects the call. */
  ports?: Port[] | Error;
  /** Forwards saved before the page opens. */
  tunnels?: Record<string, unknown>[];
  /** Connection states forced onto saved forwards by name. */
  states?: Record<string, Record<string, unknown>>;
  /** Destination checks forced onto saved forwards by name. */
  health?: Record<string, Record<string, unknown>[]>;
};

/**
 * Opens the app on a scene. The scene lives on `window.__ports`, so a test
 * can change what the next discovery or snapshot returns.
 */
export async function openPorts(page: Page, scene: Scene = {}) {
  await installAppFixture(page);
  await page.addInitScript(
    ({ scene, savedProjects, listeners }) => {
      const state = {
        ports: scene.ports ?? listeners,
        tunnels: scene.tunnels ?? [],
        states: scene.states ?? {},
        health: scene.health ?? {},
        discoveries: 0,
        seeded: false,
      };
      (window as any).__ports = state;
      const patch = () => {
        const bridge = (window as any).__TAURI_INTERNALS__;
        if (!bridge?.invoke) return setTimeout(patch, 0);
        const original = bridge.invoke;
        let seeding: Promise<void> | undefined;
        bridge.invoke = async (cmd: string, args: any) => {
          if (cmd === "discover_ports") {
            state.discoveries++;
            if (state.ports instanceof Error || (state.ports as any)?.error)
              throw new Error((state.ports as any).message ?? "unavailable");
            return structuredClone(state.ports);
          }
          if (cmd === "git_projects_list")
            return savedProjects.map((p) => ({
              ...p,
              serverId: args.serverId,
            }));
          // Every early snapshot waits for the scene, or the app could
          // render the base fixture's state from a racing first read.
          if (cmd === "snapshot")
            await (seeding ??= (async () => {
              // The base fixture's own forward would only add noise.
              const base = await original("snapshot");
              for (const t of base.config.tunnels)
                await original("delete_tunnel", { id: t.id });
              const server = base.config.servers[0].id;
              for (const t of state.tunnels)
                await original("save_tunnel", {
                  tunnel: { serverId: server, ...t },
                });
              state.seeded = true;
            })());
          const result = await original(cmd, args);
          if (cmd === "snapshot") {
            for (const t of result.config.tunnels) {
              if (state.states[t.name])
                result.runtime.tunnels[t.id] = state.states[t.name];
              if (state.health[t.name])
                (result.runtime.tunnelHealth ??= {})[t.id] =
                  state.health[t.name];
            }
          }
          return result;
        };
      };
      patch();
    },
    {
      scene: {
        ...scene,
        ports:
          scene.ports instanceof Error
            ? ({ error: true, message: scene.ports.message } as never)
            : scene.ports,
      },
      savedProjects,
      listeners,
    },
  );
  await page.goto("/");
  await page.getByRole("tab", { name: "Connections", exact: true }).click();
}

/** A saved local forward, by the fields a test cares about. */
export const forward = (
  name: string,
  remotePort: number,
  extra: Record<string, unknown> = {},
) => ({
  id: `forward-${name.toLowerCase().replace(/\W+/g, "-")}`,
  name,
  localPort: remotePort,
  localPortEnd: null,
  remoteHost: "127.0.0.1",
  remotePort,
  remotePortEnd: null,
  autoConnect: false,
  autoReconnect: true,
  direction: "local",
  project: null,
  ...extra,
});
