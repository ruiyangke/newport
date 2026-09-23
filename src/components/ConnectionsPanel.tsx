import { DestinationStatus } from "./DestinationStatus";
import { processDetails } from "../portDetails";
import { forwardingHost, forwardsForPort } from "../forwarding";
import { usePorts } from "../hooks/usePorts";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "./ui/table";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "./ui/dropdown-menu";
import { Button } from "./controls";
import { StatusLabel } from "./StatusLabel";
import {
  ArrowRight,
  ExternalLink,
  MoreHorizontal,
  Pencil,
  Play,
  Plus,
  Search,
  Square,
  Trash2,
} from "lucide-react";
import { desktop } from "../api/desktop";
import {
  active,
  newTunnel,
  range,
  type Server,
  type Tunnel,
  type Snapshot,
  type ConnectionState,
} from "../types";
const disconnected: ConnectionState = {
  status: "disconnected",
  errorMessage: null,
  reconnectAttempt: 0,
};
type Props = {
  server: Server;
  tunnels: Tunnel[];
  runtime: Snapshot["runtime"];
  pending: Set<string>;
  act: (
    key: string,
    task: () => Promise<unknown>,
    message?: string,
  ) => Promise<void>;
  setEditor: (editor: {
    kind: "tunnel";
    value: Tunnel;
    existing: boolean;
  }) => void;
  setDeleting: (target: { kind: "tunnel"; id: string; name: string }) => void;
};
export function ConnectionsPanel({
  server,
  tunnels,
  runtime,
  pending,
  act,
  setEditor,
  setDeleting,
}: Props) {
  const selected = server.id;
  const ports = usePorts(selected);
  return (
    <div className="connections-panel">
      <div className="workspace-masthead">
        <div>
          <h2>Port forwarding</h2>
        </div>
      </div>
      <section className="tunnels-section">
        <div className="section-heading">
          <h2>
            Tunnels <span>{tunnels.length}</span>
          </h2>
          <div className="section-actions">
            {tunnels.length > 1 && (
              <Button
                loading={pending.has("all")}
                onClick={() =>
                  void act("all", async () => {
                    for (const t of tunnels)
                      await desktop("set_tunnel_connected", {
                        id: t.id,
                        connected: !tunnels.every((t) =>
                          active(runtime.tunnels[t.id]?.status),
                        ),
                      });
                  })
                }
              >
                {tunnels.every((t) => active(runtime.tunnels[t.id]?.status))
                  ? "Disconnect all"
                  : "Connect all"}
              </Button>
            )}
            <Button
              variant={tunnels.length === 0 ? "default" : "outline"}
              onClick={() =>
                setEditor({
                  kind: "tunnel",
                  value: newTunnel(selected),
                  existing: false,
                })
              }
            >
              <Plus size={15} />
              Add tunnel
            </Button>
          </div>
        </div>
        {tunnels.length === 0 ? (
          <div className="section-empty">
            <p>No tunnels yet</p>
            <small>Add a tunnel or discover ports below.</small>
          </div>
        ) : (
          <ul className="tunnel-list" role="list" aria-label="Tunnels">
            {tunnels.map((t) => {
              const state = runtime.tunnels[t.id] ?? disconnected;
              return (
                <li key={t.id} className="tunnel-row">
                  <div className="tunnel-info">
                    <div className="tunnel-title">
                      <h3 id={`tunnel-${t.id}`}>
                        {t.name || `Port ${t.localPort}`}
                      </h3>
                      <StatusLabel state={state} />
                    </div>
                    <div className="port-route">
                      <div className="route-endpoint">
                        <small>On this Mac</small>
                        <code>
                          127.0.0.1:
                          <b>{range(t.localPort, t.localPortEnd)}</b>
                        </code>
                      </div>
                      <ArrowRight size={14} />
                      <div className="route-endpoint">
                        <small>Remote destination</small>
                        <code>
                          {t.remoteHost}:
                          <b>{range(t.remotePort, t.remotePortEnd)}</b>
                        </code>
                      </div>
                    </div>
                    {(t.autoConnect || t.autoReconnect) && (
                      <small className="tunnel-options">
                        {[
                          t.autoConnect && "Connect on launch",
                          t.autoReconnect && "Auto reconnect",
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                      </small>
                    )}
                    {state.status === "connected" && (
                      <DestinationStatus
                        health={runtime.tunnelHealth?.[t.id]}
                      />
                    )}
                    {state.errorMessage && (
                      <p className="connection-error">{state.errorMessage}</p>
                    )}
                  </div>
                  <div className="tunnel-actions">
                    <Button
                      variant="ghost"
                      size="icon"
                      className="icon"
                      aria-label={`Open ${t.name || t.localPort} in browser`}
                      title="Open in browser (HTTP)"
                      disabled={state.status !== "connected"}
                      onClick={() =>
                        void act(`open-${t.id}`, () =>
                          desktop("open_tunnel", { id: t.id }),
                        )
                      }
                    >
                      <ExternalLink size={15} />
                    </Button>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="icon"
                          aria-label={`Actions for tunnel ${t.name || t.localPort}`}
                        >
                          <MoreHorizontal size={16} />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        {" "}
                        <DropdownMenuItem
                          aria-label={`Edit ${t.name || t.localPort}`}
                          onSelect={() =>
                            setEditor({
                              kind: "tunnel",
                              value: t,
                              existing: true,
                            })
                          }
                        >
                          <Pencil size={15} /> Edit tunnel
                        </DropdownMenuItem>
                        <DropdownMenuItem
                          variant="destructive"
                          aria-label={`Delete ${t.name || t.localPort}`}
                          onSelect={() =>
                            setDeleting({
                              kind: "tunnel",
                              id: t.id,
                              name: t.name || `Port ${t.localPort}`,
                            })
                          }
                        >
                          <Trash2 size={15} /> Delete tunnel
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                    <Button
                      className="connect-button"
                      aria-describedby={`tunnel-${t.id}`}
                      loading={pending.has(t.id)}
                      disabled={pending.has("all")}
                      onClick={() =>
                        void act(t.id, () =>
                          desktop("set_tunnel_connected", {
                            id: t.id,
                            connected: !active(state.status),
                          }),
                        )
                      }
                    >
                      {active(state.status) ? (
                        <Square size={13} />
                      ) : (
                        <Play size={13} />
                      )}
                      {active(state.status) ? "Disconnect" : "Connect"}
                    </Button>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </section>
      <section className="remote-ports-section">
        <div className="section-heading">
          <h2>Remote ports</h2>
          <Button
            loading={ports.isFetching}
            onClick={() => {
              const id = selected;
              void act(`ports-${id}`, async () => {
                await ports.discover();
              });
            }}
          >
            <Search size={15} />
            {ports.isFetching ? "Discovering…" : "Discover ports"}
          </Button>
        </div>
        <p className="section-description">
          Refreshes automatically every 30 seconds while this page is open.
        </p>
        {ports.error && (
          <p role="alert" className="section-empty">
            Could not refresh ports: {ports.error.message}
          </p>
        )}
        {ports.isFetching && !ports.data && (
          <p role="status" className="section-empty">
            Discovering listening ports…
          </p>
        )}
        {ports.data ? (
          ports.data.length ? (
            <Table
              className="cockpit-table remote-ports-table"
              aria-label="Remote ports"
            >
              <TableHeader>
                <TableRow>
                  <TableHead scope="col">Port</TableHead>
                  <TableHead scope="col">Application / address</TableHead>
                  <TableHead scope="col" className="numeric">
                    PID
                  </TableHead>
                  <TableHead scope="col">User</TableHead>
                  <TableHead scope="col">Forwarding</TableHead>
                  <TableHead scope="col">
                    <span className="sr-only">Actions</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {ports.data.map((p) => {
                  const forwards = forwardsForPort(tunnels, selected, p);
                  const details = processDetails(p);
                  return (
                    <TableRow key={`${p.address}:${p.port}`}>
                      <TableCell>
                        <code id={`remote-port-${p.address}-${p.port}`}>
                          {p.port}
                        </code>
                      </TableCell>
                      <TableCell>
                        <span>
                          {p.containerName
                            ? `${p.containerName} · Docker`
                            : p.applicationName ||
                              p.processName ||
                              "Owner not reported"}
                        </span>
                        <small>{p.address}</small>
                        {details.map((detail) => (
                          <div
                            className="port-project-directory"
                            key={detail.label}
                          >
                            <span>{detail.label}</span>
                            <code>{detail.value}</code>
                          </div>
                        ))}
                      </TableCell>
                      <TableCell className="numeric">{p.pid ?? "—"}</TableCell>
                      <TableCell>{p.user || "—"}</TableCell>
                      <TableCell>
                        {forwards.length ? (
                          forwards.map((t) => (
                            <div key={t.id} className="port-forward-state">
                              <StatusLabel
                                state={runtime.tunnels[t.id] ?? disconnected}
                              />
                              <small>
                                127.0.0.1:{t.localPort + p.port - t.remotePort}
                              </small>
                              {runtime.tunnels[t.id]?.status ===
                                "connected" && (
                                <DestinationStatus
                                  health={runtime.tunnelHealth?.[t.id]?.filter(
                                    (h) => h.remotePort === p.port,
                                  )}
                                />
                              )}
                              {runtime.tunnels[t.id]?.errorMessage && (
                                <small>
                                  {runtime.tunnels[t.id].errorMessage}
                                </small>
                              )}
                              <Button
                                variant="ghost"
                                onClick={() =>
                                  setEditor({
                                    kind: "tunnel",
                                    value: t,
                                    existing: true,
                                  })
                                }
                                aria-label={`Edit forward for port ${p.port}`}
                              >
                                Edit
                              </Button>
                            </div>
                          ))
                        ) : (
                          <span className="muted">Not forwarded</span>
                        )}
                      </TableCell>
                      <TableCell>
                        {!forwards.length && (
                          <Button
                            variant="ghost"
                            className="text-button"
                            aria-describedby={`remote-port-${p.address}-${p.port}`}
                            onClick={() =>
                              setEditor({
                                kind: "tunnel",
                                value: {
                                  ...newTunnel(selected, p.port),
                                  remoteHost: forwardingHost(p.address),
                                  name:
                                    p.containerName ||
                                    p.applicationName ||
                                    p.processName ||
                                    "",
                                },
                                existing: false,
                              })
                            }
                          >
                            <Plus size={14} />
                            Forward
                          </Button>
                        )}
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          ) : (
            <p className="section-empty">No listening TCP ports found.</p>
          )
        ) : null}
      </section>
    </div>
  );
}
