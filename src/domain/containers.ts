import type { Tone } from "../components/master/MasterList";

/** One published or exposed port, as the agent reports it. */
export interface ContainerPort {
  ip: string | null;
  hostPort: number | null;
  containerPort: number;
  protocol: "tcp" | "udp" | "sctp";
}
export interface ComposeLabels {
  project: string;
  service: string;
  oneoff: boolean;
  number: number | null;
  workingDir: string | null;
  configFiles: string | null;
}
export type ContainerState =
  | "created"
  | "running"
  | "paused"
  | "restarting"
  | "removing"
  | "exited"
  | "dead";
export interface Container {
  id: string;
  name: string;
  image: string;
  imageId: string;
  command: string;
  state: ContainerState;
  status: string;
  health: "healthy" | "unhealthy" | "starting" | null;
  exitCode: number | null;
  oomKilled: boolean;
  error: string | null;
  restartCount: number;
  restartPolicy: string | null;
  created: number;
  startedAt: number | null;
  finishedAt: number | null;
  ports: ContainerPort[];
  compose: ComposeLabels | null;
  /** Percent of one core since the agent's previous reading. */
  cpu: number | null;
  memory: number | null;
  memoryLimit: number | null;
}
export interface ContainersSnapshot {
  engine: {
    version: string;
    apiVersion: string;
    os: string;
    rootless: boolean;
    podman: boolean;
  };
  containers: Container[];
  truncated: boolean;
}
export interface ContainerInspect {
  id: string;
  name: string;
  image: string;
  created: number;
  entrypoint: string[] | null;
  cmd: string[] | null;
  workingDir: string | null;
  user: string | null;
  hostname: string | null;
  env: { name: string; value: string }[];
  mounts: {
    type: string;
    source: string | null;
    destination: string;
    mode: string | null;
    rw: boolean;
    name: string | null;
  }[];
  networks: { name: string; ipAddress: string | null; aliases: string[] }[];
  restartPolicy: string | null;
  labels: Record<string, string>;
  healthcheck: { test: string[]; interval: number | null } | null;
  lastHealth: { at: number; exitCode: number; output: string }[];
}
export interface ContainerLogEntry {
  at: number | null;
  stream: "stdout" | "stderr";
  message: string;
  container: string | null;
}
export interface ContainerLogPage {
  entries: ContainerLogEntry[];
  cursor: string | null;
}
export type ContainerVerb = "start" | "stop" | "restart" | "pause" | "unpause";
export type ProjectVerb = "start" | "stop" | "restart";
export interface DiskTotals {
  count: number;
  size: number;
  reclaimable: number;
}
export interface DockerDisk {
  images: {
    id: string;
    tags: string[];
    size: number;
    sharedSize: number | null;
    created: number;
    containers: number;
    dangling: boolean;
  }[];
  volumes: {
    name: string;
    driver: string;
    size: number | null;
    refCount: number | null;
    anonymous: boolean;
    compose: string | null;
  }[];
  totals: {
    images: DiskTotals;
    containers: DiskTotals;
    volumes: DiskTotals;
    buildCache: DiskTotals;
  };
}
export type PruneTarget =
  | "containers"
  | "dangling-images"
  | "unused-images"
  | "build-cache"
  | "volumes";

/**
 * Selection keys. Containers are chosen by name, which survives Compose
 * recreating them under a new id; projects by their Compose name.
 */
export const containerKey = (c: { name: string }) => `c:${c.name}`;
export const projectKey = (project: string) => `p:${project}`;
export const DISK = "disk";

/** What a person calls it: the Compose service, else the container name. */
export const displayName = (c: Container) =>
  c.compose && !c.compose.oneoff ? c.compose.service : c.name;

export const isRunning = (c: Container) =>
  c.state === "running" || c.state === "restarting" || c.state === "paused";
export const isJob = (c: Container) => !!c.compose?.oneoff;

// Exit codes that mean someone stopped it on purpose: Ctrl-C, SIGTERM, and
// SIGKILL (docker stop after its timeout) unless the kernel ran out of memory.
const STOPPED = new Set([0, 130, 137, 143]);

/** Something is wrong now, or it went down on its own. */
export function isProblem(c: Container) {
  if (isJob(c)) return c.state === "exited" && !STOPPED.has(c.exitCode ?? 0);
  // Docker keeps the last health status after a container stops; it only
  // means something while it runs.
  return (
    (c.state === "running" && c.health === "unhealthy") ||
    c.state === "restarting" ||
    c.state === "dead" ||
    c.oomKilled ||
    (c.state === "exited" && !STOPPED.has(c.exitCode ?? 0))
  );
}

/** The one-word state a person reads, and how loud it is. */
export function stateOf(c: Container): { label: string; tone: Tone } {
  if (c.state === "restarting") return { label: "Restarting", tone: "warning" };
  if (c.state === "dead") return { label: "Dead", tone: "danger" };
  if (c.state === "removing") return { label: "Removing", tone: "warning" };
  if (c.state === "paused") return { label: "Paused", tone: "muted" };
  if (c.state === "created") return { label: "Created", tone: "muted" };
  if (c.state === "running") {
    if (c.health === "unhealthy") return { label: "Unhealthy", tone: "danger" };
    if (c.health === "starting") return { label: "Starting", tone: "warning" };
    return { label: "Running", tone: "success" };
  }
  if (c.oomKilled) return { label: "Out of memory", tone: "danger" };
  if (isProblem(c)) return { label: "Crashed", tone: "danger" };
  if (isJob(c) && (c.exitCode ?? 0) === 0)
    return { label: "Completed", tone: "muted" };
  return { label: "Stopped", tone: "muted" };
}

/** Why it is down or struggling, in words. */
export function problemReason(c: Container): string | null {
  if (c.state === "running" && c.health === "unhealthy")
    return "Failing its health check";
  if (c.state === "restarting")
    return c.restartCount
      ? `Keeps exiting; restarted ${c.restartCount}×`
      : "Keeps exiting and restarting";
  if (c.state === "dead")
    return c.error ? `Docker couldn't stop or remove it: ${c.error}` : "Dead";
  if (c.oomKilled) return "Killed for running out of memory";
  if (c.state !== "exited") return null;
  if (c.error) return c.error;
  const code = c.exitCode ?? 0;
  if (STOPPED.has(code)) return null;
  return exitMeaning(code);
}

/** What an exit code usually means for a container's main process. */
export function exitMeaning(code: number): string {
  switch (code) {
    case 0:
      return "Exited normally";
    case 1:
      return "Exited with an error (code 1)";
    case 125:
      return "Docker couldn't run it (code 125)";
    case 126:
      return "Its command couldn't be executed (code 126)";
    case 127:
      return "Its command wasn't found (code 127)";
    case 134:
      return "Aborted (code 134, SIGABRT)";
    case 137:
      return "Killed (code 137, SIGKILL)";
    case 139:
      return "Crashed with a segmentation fault (code 139)";
    case 143:
      return "Stopped (code 143, SIGTERM)";
    default:
      return code > 128 && code < 160
        ? `Killed by signal ${code - 128} (code ${code})`
        : `Exited with code ${code}`;
  }
}

/** The short right-hand note in the list. */
export function listNote(c: Container, now: number): string {
  if (c.state === "running") {
    if (c.health === "unhealthy") return "unhealthy";
    return c.cpu !== null ? `${formatCpu(c.cpu)}` : since(c.startedAt, now);
  }
  if (c.state === "restarting") return `restart ×${c.restartCount}`;
  if (c.state === "exited") {
    if (c.oomKilled) return "OOM";
    const code = c.exitCode ?? 0;
    return isProblem(c) ? `exit ${code}` : since(c.finishedAt, now, "ago");
  }
  return stateOf(c).label.toLowerCase();
}

export function formatCpu(cpu: number) {
  return cpu >= 10 ? `${Math.round(cpu)}%` : `${cpu.toFixed(1)}%`;
}

export function duration(ms: number) {
  const s = Math.max(0, Math.round(ms / 1000));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d) return h ? `${d}d ${h}h` : `${d}d`;
  if (h) return m ? `${h}h ${m}m` : `${h}h`;
  if (m) return `${m}m`;
  return `${s}s`;
}
const since = (at: number | null, now: number, suffix = "") =>
  at !== null ? `${duration(now - at)}${suffix ? ` ${suffix}` : ""}` : "";

/**
 * Published ports a person can reach, one per host port: Docker lists the
 * IPv4 and IPv6 bindings of the same port separately.
 */
export function publishedPorts(c: Container): ContainerPort[] {
  const seen = new Map<string, ContainerPort>();
  for (const p of c.ports) {
    if (p.hostPort === null) continue;
    const key = `${p.hostPort}/${p.protocol}`;
    const existing = seen.get(key);
    // Prefer the IPv4 / wildcard binding a loopback forward reaches.
    if (!existing || (existing.ip?.includes(":") && !p.ip?.includes(":")))
      seen.set(key, p);
  }
  return [...seen.values()].sort((a, b) => a.hostPort! - b.hostPort!);
}
/** Exposed but not published: reachable only from other containers. */
export const exposedPorts = (c: Container) => {
  const published = new Set(publishedPorts(c).map((p) => p.containerPort));
  return [
    ...new Map(
      c.ports
        .filter((p) => p.hostPort === null && !published.has(p.containerPort))
        .map((p) => [`${p.containerPort}/${p.protocol}`, p]),
    ).values(),
  ];
};
/** An SSH tunnel can carry TCP to a port bound on loopback or every address. */
export const forwardable = (p: ContainerPort) =>
  p.protocol === "tcp" &&
  p.hostPort !== null &&
  (p.ip === null || ["0.0.0.0", "::", "127.0.0.1", "::1", ""].includes(p.ip));
export const portLabel = (p: ContainerPort) =>
  p.hostPort === null
    ? `${p.containerPort}${p.protocol === "tcp" ? "" : `/${p.protocol}`}`
    : `${p.hostPort}${p.hostPort !== p.containerPort ? `→${p.containerPort}` : ""}${p.protocol === "tcp" ? "" : `/${p.protocol}`}`;

export interface ProjectGroup {
  name: string;
  /** Long-running service containers, by service then replica. */
  services: Container[];
  /** One-off `compose run` containers, newest first. */
  jobs: Container[];
  running: number;
  workingDir: string | null;
  problems: number;
}
export interface ContainerGroups {
  problems: Container[];
  projects: ProjectGroup[];
  standalone: Container[];
}

/** Problems first, then Compose projects, then everything else. */
export function groupContainers(containers: Container[]): ContainerGroups {
  const byName = (a: Container, b: Container) =>
    displayName(a).localeCompare(displayName(b)) ||
    (a.compose?.number ?? 0) - (b.compose?.number ?? 0) ||
    a.name.localeCompare(b.name);
  const projects = new Map<string, Container[]>();
  const standalone: Container[] = [];
  for (const c of containers) {
    if (!c.compose) standalone.push(c);
    else
      projects.set(c.compose.project, [
        ...(projects.get(c.compose.project) ?? []),
        c,
      ]);
  }
  return {
    problems: containers.filter(isProblem).sort(byName),
    projects: [...projects]
      .map(([name, members]) => {
        const services = members.filter((c) => !isJob(c)).sort(byName);
        return {
          name,
          services,
          jobs: members.filter(isJob).sort((a, b) => b.created - a.created),
          running: services.filter(isRunning).length,
          workingDir:
            members.find((c) => c.compose?.workingDir)?.compose?.workingDir ??
            null,
          problems: members.filter(isProblem).length,
        };
      })
      // Projects needing attention first, then by name.
      .sort(
        (a, b) =>
          Number(b.problems > 0) - Number(a.problems > 0) ||
          a.name.localeCompare(b.name),
      ),
    standalone: standalone.sort(byName),
  };
}

/** A project's overall state, from its long-running services. */
export function projectState(p: ProjectGroup): { label: string; tone: Tone } {
  if (!p.services.length) return { label: "Jobs only", tone: "muted" };
  if (p.services.some((c) => isProblem(c)))
    return { label: "Needs attention", tone: "danger" };
  if (p.running === p.services.length)
    return { label: "Running", tone: "success" };
  if (p.running === 0) return { label: "Stopped", tone: "muted" };
  return { label: "Partly running", tone: "warning" };
}

/** Words that mark a line as an error or a warning, for logs with no levels. */
const ERROR =
  /\b(error|err|fatal|panic|exception|traceback|critical|crit|failed|failure)\b|level=(error|fatal)|"level":"(error|fatal)"/i;
const WARNING = /\b(warn|warning)\b|level=warn|"level":"warn/i;
export function logTone(message: string): "danger" | "warning" | "muted" {
  if (ERROR.test(message)) return "danger";
  if (WARNING.test(message)) return "warning";
  return "muted";
}

/** A shell inside the container: bash when it has one, else sh. */
export function shellCommand(c: Container, podman = false) {
  return `${podman ? "podman" : "docker"} exec -it ${c.name} sh -c 'command -v bash >/dev/null && exec bash || exec sh'`;
}

/** What the Compose CLI knows about a project: its files and services. */
export interface ComposeProject {
  cli: { kind: "plugin" | "standalone" | "podman"; version: string } | null;
  cliError: string | null;
  workingDir: string | null;
  configFiles: string[];
  files: { path: string; text: string | null; error: string | null }[];
  services:
    | {
        name: string;
        image: string | null;
        build: boolean;
        profiles: string[];
      }[]
    | null;
  configError: string | null;
}
export type ComposeAction =
  | {
      kind: "up";
      services: string[];
      build: boolean;
      pull: boolean;
      forceRecreate: boolean;
    }
  | { kind: "down"; volumes: boolean; removeOrphans: boolean }
  | { kind: "pull"; services: string[] }
  | { kind: "build"; services: string[]; noCache: boolean }
  | { kind: "scale"; service: string; replicas: number };
export interface ComposeJob {
  job: string;
  project: string;
  command: string;
  state: "running" | "succeeded" | "failed" | "cancelled";
  exitCode: number | null;
  startedAt: number;
  finishedAt: number | null;
  output: string;
  offset: number;
}

/** A Compose action in words: the button, the progress line, the toast. */
export function composeWords(action: ComposeAction): {
  label: string;
  progress: string;
  done: string;
} {
  const who = (services: string[]) =>
    services.length === 1
      ? services[0]
      : services.length
        ? `${services.length} services`
        : "the project";
  switch (action.kind) {
    case "up":
      if (action.services.length && action.forceRecreate)
        return {
          label: action.pull
            ? "Pull & recreate"
            : action.build
              ? "Rebuild"
              : "Recreate",
          progress: `${action.pull ? "Pulling and recreating" : action.build ? "Rebuilding" : "Recreating"} ${who(action.services)}…`,
          done: `${action.build ? "Rebuilt" : "Recreated"} ${who(action.services)}`,
        };
      return {
        label: action.pull ? "Pull & up" : action.build ? "Rebuild & up" : "Up",
        progress: action.pull
          ? `Pulling images and bringing ${who(action.services)} up…`
          : action.build
            ? `Building and bringing ${who(action.services)} up…`
            : `Bringing ${who(action.services)} up…`,
        done: `${who(action.services) === "the project" ? "Project" : who(action.services)} is up`,
      };
    case "down":
      return {
        label: action.volumes ? "Down & delete volumes" : "Down",
        progress: action.volumes
          ? "Taking the project down and deleting its volumes…"
          : "Taking the project down…",
        done: "Project is down",
      };
    case "pull":
      return {
        label: "Pull images",
        progress: `Pulling images for ${who(action.services)}…`,
        done: "Images pulled",
      };
    case "build":
      return {
        label: "Build images",
        progress: `Building ${who(action.services)}…`,
        done: "Images built",
      };
    case "scale":
      return {
        label: "Scale",
        progress: `Scaling ${action.service} to ${action.replicas}…`,
        done: `${action.service} scaled to ${action.replicas}`,
      };
  }
}

/** How long a job ran, or has been running. */
export const jobDuration = (job: ComposeJob, now: number) =>
  duration((job.finishedAt ?? now) - job.startedAt);
