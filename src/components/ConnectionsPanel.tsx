import { CodeBlock } from "./CodeBlock";
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
  Clipboard,
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
  const clip = runtime.clipboard[selected] ?? disconnected;
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
            loading={pending.has(`ports-${selected}`)}
            onClick={() => {
              const id = selected;
              void act(`ports-${id}`, async () => {
                await ports.discover();
              });
            }}
          >
            <Search size={15} />
            {pending.has(`ports-${selected}`)
              ? "Discovering…"
              : "Discover ports"}
          </Button>
        </div>
        {ports.data ? (
          ports.data.length ? (
            <Table
              className="cockpit-table remote-ports-table"
              aria-label="Remote ports"
            >
              <TableHeader>
                <TableRow>
                  <TableHead scope="col">Port</TableHead>
                  <TableHead scope="col">Process / address</TableHead>
                  <TableHead scope="col">
                    <span className="sr-only">Actions</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {ports.data.map((p) => (
                  <TableRow key={`${p.address}:${p.port}`}>
                    <TableCell>
                      <code id={`remote-port-${p.address}-${p.port}`}>
                        {p.port}
                      </code>
                    </TableCell>
                    <TableCell>
                      {p.containerName
                        ? `${p.containerName} · Docker`
                        : p.processName || "Owner not reported"}
                      <small>
                        {p.address}
                        {p.pid ? ` · PID ${p.pid}` : ""}
                      </small>
                    </TableCell>
                    <TableCell>
                      <Button
                        variant="ghost"
                        className="text-button"
                        aria-describedby={`remote-port-${p.address}-${p.port}`}
                        onClick={() =>
                          setEditor({
                            kind: "tunnel",
                            value: {
                              ...newTunnel(selected, p.port),
                              remoteHost:
                                p.address === "::"
                                  ? "::1"
                                  : ["0.0.0.0", "*"].includes(p.address)
                                    ? "127.0.0.1"
                                    : p.address,
                              name: p.containerName || p.processName || "",
                            },
                            existing: false,
                          })
                        }
                      >
                        <Plus size={14} />
                        Forward
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          ) : (
            <p className="section-empty">No listening TCP ports found.</p>
          )
        ) : null}
      </section>
      <section className="clipboard-section">
        <div className="section-heading">
          <h2>
            <Clipboard size={17} />
            Clipboard sync
          </h2>
          <StatusLabel state={clip} />
        </div>
        <p className="section-description">
          Read your Mac clipboard on this server with{" "}
          <code>xclip -selection clipboard -o</code>.
        </p>
        <p className="clipboard-note">
          Sends clipboard changes over SSH and resumes on app launch. Use only
          trusted servers; clipboard files may remain after a lost connection.
        </p>
        <div className="clipboard-actions">
          <Button
            loading={pending.has(`clip-${selected}`)}
            onClick={() =>
              void act(`clip-${selected}`, () =>
                desktop("set_clipboard_enabled", {
                  id: selected,
                  enabled: !server.clipboardEnabled,
                }),
              )
            }
          >
            {server.clipboardEnabled ? (
              <Square size={13} />
            ) : (
              <Play size={13} />
            )}
            {server.clipboardEnabled ? "Disable sync" : "Enable sync"}
          </Button>
          {server.clipboardEnabled && !active(clip.status) && (
            <Button
              loading={pending.has(`clip-${selected}`)}
              onClick={() =>
                void act(`clip-${selected}`, () =>
                  desktop("set_clipboard_enabled", {
                    id: selected,
                    enabled: true,
                  }),
                )
              }
            >
              Retry sync
            </Button>
          )}
        </div>
        {active(clip.status) && runtime.clipboardMessages?.[selected] && (
          <p className="clipboard-note">
            {runtime.clipboardMessages[selected]}
          </p>
        )}
        {clip.status === "connected" &&
          runtime.clipboardPathNeeded?.[selected] && (
            <div className="clipboard-path-setup">
              <h3>Set up xclip on your server</h3>
              <p className="clipboard-note">
                SSH can’t find the clipboard shim first. Add this line to
                ~/.bashrc (Bash) or ~/.zshrc (Zsh) on the server. Run it in your
                terminal to apply it now.
              </p>
              <CodeBlock
                label="Shell configuration"
                code={'export PATH="$HOME/.local/bin:$PATH"'}
              />
              <p className="clipboard-note">
                Until then, use{" "}
                <code>~/.local/bin/xclip -selection clipboard -o</code>. Disable
                and enable sync to check the SSH PATH again.
              </p>
            </div>
          )}
        {clip.errorMessage && (
          <p className="connection-error">{clip.errorMessage}</p>
        )}
      </section>
    </div>
  );
}
