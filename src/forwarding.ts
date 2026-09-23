import type { DiscoveredPort, Tunnel } from "./types";

export function forwardingHost(address: string): string {
  const host = address.toLowerCase().replace(/\.$/, "");
  if (["0.0.0.0", "*", "localhost"].includes(host)) return "127.0.0.1";
  if (host.includes(":")) {
    try {
      const normalized = new URL(`http://[${host}]/`).hostname.slice(1, -1);
      return normalized === "::" ? "::1" : normalized;
    } catch {
      return host;
    }
  }
  return host;
}

export function forwardsForPort(
  tunnels: Tunnel[],
  serverId: string,
  port: DiscoveredPort,
) {
  return tunnels.filter(
    (t) =>
      t.serverId === serverId &&
      forwardingHost(t.remoteHost) === forwardingHost(port.address) &&
      port.port >= t.remotePort &&
      port.port <= (t.remotePortEnd ?? t.remotePort),
  );
}
