import { forwardingHost, forwardsForPort } from "./forwarding";
import {
  RUNTIME_KINDS,
  portKinds,
  portName,
  processDetails,
} from "./portDetails";
import {
  direction,
  newTunnel,
  type DiscoveredPort,
  type Tunnel,
} from "./types";

/** Where a project's identity came from, strongest first. */
export type ProjectSource = "saved" | "compose" | "vcs" | "marker" | "other";

/**
 * One listening service port, with the saved local forwards that reach it. A
 * process listening on both IPv4 and IPv6 is one entry; `port` is the
 * listener a loopback client would reach first.
 */
export interface PortEntry {
  key: string;
  port: DiscoveredPort;
  addresses: string[];
  forwards: Tunnel[];
  kinds: string[];
  worktree: Worktree | null;
}
/** One Git checkout of a project: its main checkout or a linked worktree. */
export interface Worktree {
  path: string;
  branch: string | null;
  main: boolean;
}
export interface PortProject {
  /** Stable grouping key: a root path, `compose:<name>`, or "" for other. */
  key: string;
  name: string;
  /** The server path the project lives at, when one is known. */
  path: string | null;
  source: ProjectSource;
  /** The marker that identified the root, such as `.git` or `Cargo.toml`. */
  marker: string | null;
  ports: PortEntry[];
  /** Saved forwards filed here that no listening port accounts for. */
  forwards: Tunnel[];
  /** Checkouts its ports run from, the main checkout first. */
  worktrees: Worktree[];
}
/** A saved project the grouping may name, such as a Git bookmark. */
export interface KnownProject {
  name: string;
  path: string;
}

export const OTHER = "";
const VCS = new Set([
  ".git",
  ".hg",
  ".jj",
  ".sl",
  ".fslckout",
  "_FOSSIL_",
  ".bzr",
  "_darcs",
  ".svn",
  ".projectile",
]);

const trim = (path: string) => path.replace(/\/+$/, "") || "/";
const basename = (path: string) => trim(path).split("/").pop() || path;
const within = (path: string, root: string) =>
  path === root || path.startsWith(root === "/" ? "/" : `${root}/`);

/** The project key a listening port belongs to, or OTHER. */
// Every worktree of a repository reports the repository as its root, so
// one project holds them all.
export function projectKey(port: DiscoveredPort): string {
  if (port.projectRoot) return trim(port.projectRoot);
  if (port.composeWorkingDir) return trim(port.composeWorkingDir);
  if (port.composeProject) return `compose:${port.composeProject}`;
  return OTHER;
}
function worktreeOf(port: DiscoveredPort): Worktree | null {
  if (!port.worktreePath) return null;
  return {
    path: trim(port.worktreePath),
    branch: port.worktreeBranch ?? null,
    main: port.worktreeMain !== false,
  };
}

function describe(
  key: string,
  port: DiscoveredPort | undefined,
  known: KnownProject[],
): Omit<PortProject, "ports" | "forwards" | "worktrees"> {
  if (key === OTHER)
    return {
      key,
      name: "Other services",
      path: null,
      source: "other",
      marker: null,
    };
  if (key.startsWith("compose:"))
    return {
      key,
      name: key.slice("compose:".length),
      path: null,
      source: "compose",
      marker: "compose",
    };
  const saved = known.find((project) => trim(project.path) === key);
  if (saved)
    return {
      key,
      name: saved.name,
      path: key,
      source: "saved",
      marker: port?.projectMarker ?? null,
    };
  const marker = port?.projectMarker ?? null;
  return {
    key,
    // A bare repository's directory is conventionally `<name>.git`.
    name:
      (marker === "compose" && port?.composeProject) ||
      basename(key).replace(/\.git$/, "") ||
      basename(key),
    path: key,
    source:
      marker === "compose"
        ? "compose"
        : marker && VCS.has(marker)
          ? "vcs"
          : "marker",
    marker,
  };
}

/**
 * Smart aggregation: listening ports and saved forwards grouped by project.
 *
 * A port's project is its Compose working directory or project, else the root
 * the server found by walking up from the process's working directory. A
 * root inside a saved project (a Git bookmark) folds into that project, so a
 * monorepo's packages read as one project. A saved forward stays with the
 * project it was filed under, even while nothing listens; an unfiled one
 * follows the port it reaches.
 */
export function groupPorts(
  ports: DiscoveredPort[],
  tunnels: Tunnel[],
  serverId: string,
  known: KnownProject[] = [],
): PortProject[] {
  const savedRoots = known
    .map((project) => ({ ...project, path: trim(project.path) }))
    .sort((a, b) => b.path.length - a.path.length);
  const fold = (key: string) =>
    key.startsWith("/")
      ? (savedRoots.find((project) => within(key, project.path))?.path ?? key)
      : key;
  const groups = new Map<string, PortProject>();
  const group = (key: string, port?: DiscoveredPort) => {
    let found = groups.get(key);
    if (!found) {
      found = {
        ...describe(key, port, known),
        ports: [],
        forwards: [],
        worktrees: [],
      };
      groups.set(key, found);
    }
    return found;
  };
  const mine = tunnels.filter((t) => t.serverId === serverId);
  const placed = new Set<string>();
  // Dual-stack listeners of one owner merge into one entry.
  const merged = new Map<string, DiscoveredPort[]>();
  for (const port of ports) {
    const owner = port.pid ?? port.containerName ?? port.processName ?? "";
    const id = `${port.port}|${owner}`;
    merged.set(id, [...(merged.get(id) ?? []), port]);
  }
  for (const listeners of merged.values()) {
    listeners.sort((a, b) => rank(a.address) - rank(b.address));
    const port = listeners[0];
    const forwards = [
      ...new Map(
        listeners
          .flatMap((listener) => forwardsForPort(mine, serverId, listener))
          .map((t) => [t.id, t]),
      ).values(),
    ];
    // A forward filed under a project pulls an unattributed port in with it.
    const filed = forwards.find((t) => t.project)?.project;
    const own = fold(projectKey(port));
    const key = own === OTHER && filed ? fold(filed) : own;
    group(key, port).ports.push({
      key: `${port.address}:${port.port}`,
      port,
      addresses: listeners.map((listener) => listener.address),
      forwards,
      kinds: portKinds(port),
      worktree: worktreeOf(port),
    });
    for (const t of forwards) placed.add(t.id);
  }
  for (const t of mine) {
    if (placed.has(t.id)) continue;
    group(t.project ? fold(t.project) : OTHER).forwards.push(t);
  }
  for (const project of groups.values()) {
    const checkouts = new Map<string, Worktree>();
    for (const { worktree } of project.ports)
      if (worktree) checkouts.set(worktree.path, worktree);
    project.worktrees = [...checkouts.values()].sort(
      (a, b) =>
        Number(b.main) - Number(a.main) ||
        (a.branch ?? "").localeCompare(b.branch ?? "") ||
        a.path.localeCompare(b.path),
    );
    project.ports.sort(
      (a, b) =>
        a.port.port - b.port.port ||
        a.port.address.localeCompare(b.port.address),
    );
  }
  // Projects first, by name; the catch-all last.
  return [...groups.values()].sort(
    (a, b) =>
      Number(a.key === OTHER) - Number(b.key === OTHER) ||
      a.name.localeCompare(b.name) ||
      a.key.localeCompare(b.key),
  );
}

/** Every saved forward that belongs to these rows of a project. */
export function projectTunnels(
  project: PortProject,
  entries: PortEntry[] = project.ports,
  saved: Tunnel[] = project.forwards,
): Tunnel[] {
  const seen = new Map<string, Tunnel>();
  for (const entry of entries)
    for (const t of entry.forwards) seen.set(t.id, t);
  for (const t of saved) seen.set(t.id, t);
  return [...seen.values()];
}

/**
 * A project's rows by checkout, the main checkout first. Rows without a Git
 * checkout lead, under no heading. A project with only its main checkout
 * gets one section and needs no heading at all.
 */
export function worktreeSections(entries: PortEntry[]) {
  const sections = new Map<
    string,
    { worktree: Worktree | null; entries: PortEntry[] }
  >();
  for (const entry of entries) {
    const key = entry.worktree?.path ?? "";
    const section = sections.get(key) ?? {
      worktree: entry.worktree,
      entries: [],
    };
    section.entries.push(entry);
    sections.set(key, section);
  }
  return [...sections.values()].sort(
    (a, b) =>
      Number(!!a.worktree) - Number(!!b.worktree) ||
      Number(!!b.worktree?.main) - Number(!!a.worktree?.main) ||
      (a.worktree?.branch ?? "").localeCompare(b.worktree?.branch ?? ""),
  );
}

/**
 * What a person calls a listening service: its Compose service, else what it
 * is (a framework or known service, never a bare runtime like "node"), else
 * the name discovery gave it, else its process.
 */
export function serviceName(entry: PortEntry): string | null {
  const p = entry.port;
  return (
    p.composeService ||
    p.containerName ||
    entry.kinds.find((kind) => !RUNTIME_KINDS.has(kind)) ||
    // Discovery's own name (such as "Vite · app") beats a thread name.
    p.applicationName ||
    p.processName ||
    portName(p)
  );
}

const WILDCARDS = new Set(["0.0.0.0", "::", "*", "[::]"]);
const DIRECTORY = new Set(["Project directory", "Working directory"]);

/**
 * The supporting line under a service's name: where in the checkout it runs,
 * what else it is, and whether it listens beyond the server itself. PIDs,
 * users and loopback addresses are for the tooltip, not the scan.
 */
export function serviceDetail(entry: PortEntry, base: string | null): string[] {
  const p = entry.port;
  const name = serviceName(entry);
  const short = (path: string) => shortPath(path, [p.user]);
  const parts: string[] = [];
  for (const detail of processDetails(p)) {
    if (!DIRECTORY.has(detail.label)) parts.push(short(detail.value));
    else if (base && detail.value.startsWith(`${base}/`))
      parts.unshift(`${detail.value.slice(base.length + 1)}/`);
    else if (detail.value !== base) parts.unshift(short(detail.value));
  }
  for (const kind of entry.kinds) if (kind !== name) parts.push(kind);
  // A Compose container's own name is `<project>-<service>-<n>`: it repeats
  // the row and project, and lives in the tooltip instead.
  if (!p.composeService && p.containerName && p.containerName !== name)
    parts.push(p.containerName);
  // A service named by what it is still says which process that is, unless a
  // runtime tag already does or the names only differ in case ("Postgres").
  if (
    !p.containerName &&
    p.processName &&
    p.processName.toLowerCase() !== name?.toLowerCase() &&
    !entry.kinds.some((kind) => RUNTIME_KINDS.has(kind))
  )
    parts.push(p.processName);
  if (entry.addresses.some((address) => WILDCARDS.has(address)))
    parts.push("all interfaces");
  // Usually a dev server left behind by a removed checkout; lead with it so
  // a long path cannot truncate it away.
  if (p.workingDirectoryDeleted) parts.unshift("folder deleted");
  const seen = new Set([name?.toLowerCase()]);
  return parts.filter((part) => {
    const key = part.toLowerCase();
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/**
 * A path short enough to scan: `~` for the owner's home, then only as many
 * trailing folders as fit (always at least two). The full path is in the
 * row's tooltip.
 */
export function shortPath(
  path: string,
  users: (string | null | undefined)[],
  budget = 40,
) {
  if (!path.startsWith("/")) return path;
  const tilded = tildify(path, users);
  if (tilded.length <= budget) return tilded;
  const segments = tilded.split("/").filter(Boolean);
  let kept = segments.slice(-2).join("/");
  for (let i = segments.length - 3; i >= 0; i--) {
    const next = `${segments[i]}/${kept}`;
    if (next.length + 2 > budget) break;
    kept = next;
  }
  return `…/${kept}`;
}

/** What to call a forward: its service, and the branch for a linked worktree. */
export function forwardName(entry: PortEntry): string {
  const name = serviceName(entry) ?? "";
  const branch = entry.worktree?.main === false ? entry.worktree.branch : null;
  return branch ? (name ? `${name} · ${branch}` : branch) : name;
}

/** `/home/<user>/x` as `~/x` when every process there runs as that user. */
export function tildify(path: string, users: (string | null | undefined)[]) {
  const user = users.find(Boolean);
  if (!user || users.some((u) => u && u !== user)) return path;
  const home = user === "root" ? "/root" : `/home/${user}`;
  return path === home
    ? "~"
    : path.startsWith(`${home}/`)
      ? `~${path.slice(home.length)}`
      : path;
}

// Of a dual-stack pair, forward to the address a loopback client reaches.
function rank(address: string) {
  const host = forwardingHost(address);
  return host === "127.0.0.1" ? 0 : host === "::1" ? 1 : 2;
}

/**
 * New local forwards for every listening port in a project (or some of its
 * rows, such as one worktree's) that none reaches yet. Each uses the same port on this Mac, or the next one no saved local
 * forward claims.
 */
export function planForwards(
  project: PortProject,
  tunnels: Tunnel[],
  serverId: string,
  entries: PortEntry[] = project.ports,
): Tunnel[] {
  const taken = new Set<number>();
  for (const t of tunnels)
    if (direction(t) === "local")
      for (let p = t.localPort; p <= (t.localPortEnd ?? t.localPort); p++)
        taken.add(p);
  const planned: Tunnel[] = [];
  for (const entry of entries) {
    const { port, forwards } = entry;
    if (forwards.length) continue;
    let local = port.port;
    while (taken.has(local) && local < 65535) local++;
    if (taken.has(local)) continue;
    taken.add(local);
    planned.push({
      ...newTunnel(serverId, port.port),
      localPort: local,
      remoteHost: forwardingHost(port.address),
      name: forwardName(entry),
      project: project.key === OTHER ? null : project.key,
    });
  }
  return planned;
}
