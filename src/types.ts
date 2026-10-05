export interface Server {
  id: string;
  name: string;
  sshUser: string;
  sshHost: string;
  sshPort: number;
  identityFile: string | null;
  agentSource?: "system" | "onePassword" | null;
  agentKeyFingerprint?: string | null;
  authMethod: "publicKey" | "password";
  clipboardEnabled?: boolean;
  browserEnabled?: boolean;
}
// Match Rust Server::same_connection: presentation and clipboard preferences
// must not reset mounted SSH resources when a snapshot changes.
export function serverConnectionKey(server: Server): string {
  return JSON.stringify([
    server.id,
    server.sshHost,
    server.sshUser,
    server.sshPort,
    server.authMethod,
    server.identityFile,
    server.agentSource ?? null,
    server.agentKeyFingerprint ?? null,
  ]);
}

export interface Tunnel {
  id: string;
  name: string;
  serverId: string;
  localPort: number;
  localPortEnd: number | null;
  remoteHost: string;
  remotePort: number;
  remotePortEnd: number | null;
  autoConnect: boolean;
  autoReconnect: boolean;
  /**
   * `local` (ssh -L) serves a server port on this Mac; `remote` (ssh -R) opens
   * `remoteHost:remotePort` on the server and relays it to `127.0.0.1:localPort`
   * here. Older saved forwards have no direction and are local.
   */
  direction?: Direction;
  /** The project key a forward belongs to; see portProjects.ts. */
  project?: string | null;
}
export type Direction = "local" | "remote";
export const direction = (t: Tunnel): Direction => t.direction ?? "local";
export type Status =
  "disconnected" | "connecting" | "connected" | "reconnecting" | "error";
export interface DestinationHealth {
  localPort: number;
  remotePort: number;
  status: "checking" | "reachable" | "unavailable" | "blocked" | "unknown";
  message: string | null;
  checkedAt: number | null;
}
export interface ConnectionState {
  status: Status;
  errorMessage: string | null;
  reconnectAttempt: number;
}
export interface Snapshot {
  instanceId?: string;
  revision?: number;
  config: { servers: Server[]; tunnels: Tunnel[] };
  runtime: {
    tunnels: Record<string, ConnectionState>;
    tunnelHealth?: Record<string, DestinationHealth[]>;
    clipboard: Record<string, ConnectionState>;
    clipboardMessages?: Record<string, string>;
    clipboardPathNeeded?: Record<string, boolean>;
    health: Record<string, string>;
    connectivity?: Record<string, { status: string; error: string | null }>;
    connectionRevisions?: Record<string, number>;
  };
  loadError: string | null;
}
export type TunnelEditor = {
  kind: "tunnel";
  value: Tunnel;
  existing: boolean;
  /** Projects the forward can be filed under, from port discovery. */
  projects?: { key: string; name: string }[];
};
export type TunnelDeletion = { kind: "tunnel"; id: string; name: string };
const unknownConnection: ConnectionState = {
  status: "disconnected",
  errorMessage: null,
  reconnectAttempt: 0,
};
/** Runtime state for a tunnel that has not reported yet. */
export const tunnelState = (runtime: Snapshot["runtime"], id: string) =>
  runtime.tunnels[id] ?? unknownConnection;
export interface DiscoveredPort {
  executable?: string | null;
  workingDirectory?: string | null;
  /** The working directory was removed while the process kept running. */
  workingDirectoryDeleted?: boolean;
  /** The executable was replaced or removed since the process started. */
  executableDeleted?: boolean;
  command?: string | null;
  arguments?: string[];
  user?: string | null;
  applicationName?: string | null;
  port: number;
  address: string;
  processName: string | null;
  containerName?: string | null;
  pid: number | null;
  /** Server-side project root, found the way projectile finds one. */
  projectRoot?: string | null;
  /** The file or directory that identified projectRoot, or "compose". */
  projectMarker?: string | null;
  composeProject?: string | null;
  composeService?: string | null;
  composeWorkingDir?: string | null;
  image?: string | null;
  /** The Git checkout the process runs in; projectRoot is the repository. */
  worktreePath?: string | null;
  /** The branch checked out there, or a short commit id when detached. */
  worktreeBranch?: string | null;
  /** True for the repository's main checkout, false for a linked worktree. */
  worktreeMain?: boolean | null;
}
export const displayName = (s: Server) => s.name || `${s.sshUser}@${s.sshHost}`;
export const active = (status?: Status) =>
  status === "connected" ||
  status === "connecting" ||
  status === "reconnecting";
export const range = (start: number, end: number | null) =>
  end && end !== start ? `${start}–${end}` : String(start);
export function parseRange(value: string): [number, number | null] {
  if (!/^\d+(?:\s*-\s*\d+)?$/.test(value.trim()))
    throw new Error("Enter a port or a range, such as 8000 or 8000-8010.");
  const [start, end] = value
    .trim()
    .split(/\s*-\s*/)
    .map(Number);
  if (
    start < 1 ||
    start > 65535 ||
    (end !== undefined && (end < start || end > 65535))
  )
    throw new Error(
      "Ports must be 1–65535. The end of a range must be at least its start.",
    );
  if (end !== undefined && end - start >= 256)
    throw new Error("Use at most 256 ports per tunnel.");
  return [start, end ?? null];
}
export const newServer = (): Server => ({
  id: crypto.randomUUID(),
  name: "",
  sshUser: "",
  sshHost: "",
  sshPort: 22,
  identityFile: null,
  authMethod: "publicKey",
});
export const newTunnel = (serverId: string, port = 8000): Tunnel => ({
  id: crypto.randomUUID(),
  serverId,
  name: "",
  localPort: port,
  localPortEnd: null,
  remoteHost: "127.0.0.1",
  remotePort: port,
  remotePortEnd: null,
  autoConnect: false,
  autoReconnect: true,
  direction: "local",
  project: null,
});
