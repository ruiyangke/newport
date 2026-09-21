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
}
export type Status =
  "disconnected" | "connecting" | "connected" | "reconnecting" | "error";
export interface ConnectionState {
  status: Status;
  errorMessage: string | null;
  reconnectAttempt: number;
}
export interface Snapshot {
  config: { servers: Server[]; tunnels: Tunnel[] };
  runtime: {
    tunnels: Record<string, ConnectionState>;
    clipboard: Record<string, ConnectionState>;
    clipboardMessages?: Record<string, string>;
    clipboardPathNeeded?: Record<string, boolean>;
    health: Record<string, string>;
    connectionRevisions?: Record<string, number>;
  };
  loadError: string | null;
}
export interface DiscoveredPort {
  port: number;
  address: string;
  processName: string | null;
  containerName?: string | null;
  pid: number | null;
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
});
