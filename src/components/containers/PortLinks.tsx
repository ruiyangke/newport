import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ExternalLink, LoaderCircle } from "lucide-react";
import { snapshotOptions } from "../../hooks/useSnapshot";
import { useTunnelActions } from "../../hooks/useTunnelActions";
import {
  forwardingHost,
  forwardsForPort,
  localPortFor,
} from "../../forwarding";
import {
  direction,
  newTunnel,
  tunnelState,
  type DiscoveredPort,
  type Tunnel,
} from "../../types";
import {
  displayName,
  exposedPorts,
  forwardable,
  portLabel,
  publishedPorts,
  type Container,
  type ContainerPort,
} from "../../domain/containers";

const chip =
  "inline-flex h-[24px] items-center gap-[5px] rounded-[6px] px-[8px] font-mono text-[11.5px] outline-none transition-colors focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-60 motion-reduce:transition-none";

/** The first local port no saved forward claims, starting at `port`. */
function freeLocalPort(tunnels: Tunnel[], port: number) {
  const taken = new Set<number>();
  for (const t of tunnels)
    if (direction(t) === "local")
      for (let p = t.localPort; p <= (t.localPortEnd ?? t.localPort); p++)
        taken.add(p);
  let local = port;
  while (taken.has(local) && local < 65535) local++;
  return local;
}

/**
 * A container's published ports. Each one is a click away from this Mac:
 * an existing forward opens, a stopped one connects and opens, and a port
 * nothing reaches yet gets a forward saved in Connections, then opens.
 */
export function PortLinks({
  serverId,
  container,
}: {
  serverId: string;
  container: Container;
}) {
  const snapshot = useQuery({ ...snapshotOptions, enabled: false }).data;
  const tunnels = snapshot?.config.tunnels ?? [];
  const runtime = snapshot?.runtime;
  const actions = useTunnelActions(serverId);
  // A forward to open as soon as it is up.
  const [opening, setOpening] = useState<string | null>(null);
  const openingState =
    opening && runtime ? tunnelState(runtime, opening).status : null;
  useEffect(() => {
    if (!opening) return;
    if (openingState === "connected") {
      actions.open(opening);
      setOpening(null);
    } else if (openingState === "error") setOpening(null);
  }, [opening, openingState, actions]);

  const published = publishedPorts(container);
  const internal = exposedPorts(container);
  if (!published.length && !internal.length) return null;
  const running = container.state === "running";

  const link = (p: ContainerPort) => {
    const label = portLabel(p);
    const target = `container:${container.name}:${p.hostPort}`;
    const forwards = forwardsForPort(tunnels, serverId, {
      address: p.ip || "0.0.0.0",
      port: p.hostPort!,
    } as DiscoveredPort);
    const local = forwards[0];
    const state = local && runtime ? tunnelState(runtime, local.id) : null;
    const busy =
      actions.pendingTargets.has(target) ||
      (!!local && (actions.pending.has(local.id) || opening === local.id));
    if (local && state?.status === "connected") {
      const port = localPortFor(local, p.hostPort!);
      return (
        <button
          key={label}
          type="button"
          aria-label={`Open port ${label} in browser`}
          title={`Open http://localhost:${port}`}
          onClick={() => actions.open(local.id)}
          className={`${chip} bg-primary/8 text-primary hover:bg-primary/14 dark:bg-primary/15 dark:hover:bg-primary/25`}
        >
          localhost:{port}
          <span className="text-primary/60">→ {label}</span>
          <ExternalLink size={11} aria-hidden="true" />
        </button>
      );
    }
    if (!forwardable(p))
      return (
        <span
          key={label}
          title={`Bound to ${p.ip}; an SSH tunnel can't reach it`}
          className={`${chip} border border-border text-muted-foreground`}
        >
          {label}
        </span>
      );
    return (
      <button
        key={label}
        type="button"
        disabled={busy || !running}
        title={
          running
            ? local
              ? `Connect the saved forward and open it`
              : `Forward to this Mac and open it`
            : "Start the container to reach this port"
        }
        aria-label={`${local ? "Connect and open" : "Forward and open"} port ${label}`}
        onClick={() => {
          if (local) {
            actions.toggle(local.id, true);
            setOpening(local.id);
            return;
          }
          const tunnel: Tunnel = {
            ...newTunnel(serverId, p.hostPort!),
            localPort: freeLocalPort(tunnels, p.hostPort!),
            remoteHost: forwardingHost(p.ip || "0.0.0.0"),
            name: displayName(container),
          };
          actions.forward([tunnel], [], target);
          setOpening(tunnel.id);
        }}
        className={`${chip} border border-border hover:bg-accent`}
      >
        {busy ? (
          <LoaderCircle
            size={11}
            aria-hidden="true"
            className="animate-spin motion-reduce:animate-none"
          />
        ) : null}
        {label}
        <span className="font-sans text-[11px] text-muted-foreground">
          {busy ? "Forwarding…" : local ? "Connect & open" : "Forward & open"}
        </span>
      </button>
    );
  };

  return (
    <div className="flex flex-col gap-[6px]">
      <div className="flex flex-wrap items-center gap-[6px]">
        <span className="mr-[2px] text-[11px] text-muted-foreground">
          Ports
        </span>
        {published.map(link)}
        {internal.length > 0 && (
          <span
            className="text-[11px] text-muted-foreground"
            title="Exposed to other containers, not published on the server"
          >
            {published.length ? "· " : ""}internal{" "}
            <span className="font-mono">
              {internal.map(portLabel).join(", ")}
            </span>
          </span>
        )}
      </div>
      {actions.error && (
        <p role="alert" className="text-[12px] text-(--danger) wrap-anywhere">
          {actions.error}
        </p>
      )}
    </div>
  );
}
