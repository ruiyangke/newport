import { SectionEmpty } from "./Feedback";
import {
  SectionHeader,
  SectionActions,
  CollectionCount,
} from "./SectionHeader";
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
} from "./MonitoringTable";
import { Button } from "./controls";
import { PageHeader } from "./PageHeader";
import { StatusLabel } from "./StatusLabel";
import { Plus, Search } from "lucide-react";
import { TunnelRow } from "./TunnelRow";
import { desktop } from "../api/desktop";
import {
  active,
  newTunnel,
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
      <PageHeader
        title="Port forwarding"
        subtitle="Forward local ports to services on this server."
        actions={
          <Button
            variant="default"
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
        }
      />
      <section>
        <SectionHeader>
          <h2>
            Tunnels <span>{tunnels.length}</span>
          </h2>
          <SectionActions>
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
          </SectionActions>
        </SectionHeader>
        {tunnels.length === 0 ? (
          <SectionEmpty as="div">
            <p>No tunnels yet</p>
            <small>Use Add tunnel above, or discover ports below.</small>
          </SectionEmpty>
        ) : (
          <ul
            className="m-0 list-none rounded-[6px] border border-border p-0"
            role="list"
            aria-label="Tunnels"
          >
            {tunnels.map((t) => {
              const state = runtime.tunnels[t.id] ?? disconnected;
              return (
                <TunnelRow
                  key={t.id}
                  tunnel={t}
                  state={state}
                  health={runtime.tunnelHealth?.[t.id]}
                  pending={pending}
                  act={act}
                  setEditor={setEditor}
                  setDeleting={setDeleting}
                />
              );
            })}
          </ul>
        )}
      </section>
      <section data-slot="remote-ports-section" className="mt-[27px]">
        <SectionHeader>
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
        </SectionHeader>
        <p
          data-slot="section-description"
          className="max-w-[70ch] text-[12px] leading-[1.65] text-muted-foreground"
        >
          Refreshes automatically every 30 seconds while this page is open.
        </p>
        {ports.error && (
          <p
            role="alert"
            data-slot="cockpit-error"
            className="text-[12px] leading-[1.6] text-(--red) wrap-anywhere"
          >
            Could not refresh ports: {ports.error.message}
          </p>
        )}
        {ports.isFetching && !ports.data && (
          <SectionEmpty role="status">
            Discovering listening ports…
          </SectionEmpty>
        )}
        {ports.data ? (
          ports.data.length ? (
            <>
              <CollectionCount>
                Showing {ports.data.length} listening{" "}
                {ports.data.length === 1 ? "port" : "ports"}
              </CollectionCount>
              <Table kind="ports" aria-label="Remote ports">
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Port</TableHead>
                    <TableHead scope="col">Application / address</TableHead>
                    <TableHead scope="col" numeric>
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
                              data-slot="port-project-directory"
                              className="mt-[6px] text-muted-foreground [&>span]:block [&>code]:block [&>code]:select-text [&>code]:[font:inherit] [&>code]:whitespace-pre-wrap [&>code]:wrap-anywhere"
                              key={detail.label}
                            >
                              <span>{detail.label}</span>
                              <code>{detail.value}</code>
                            </div>
                          ))}
                        </TableCell>
                        <TableCell numeric>{p.pid ?? "—"}</TableCell>
                        <TableCell>{p.user || "—"}</TableCell>
                        <TableCell>
                          {forwards.length ? (
                            forwards.map((t) => (
                              <div
                                key={t.id}
                                data-slot="port-forward-state"
                                className="not-first:mt-[8px] [&>small:first-of-type]:whitespace-nowrap"
                              >
                                <StatusLabel
                                  state={runtime.tunnels[t.id] ?? disconnected}
                                />
                                <small>
                                  127.0.0.1:
                                  {t.localPort + p.port - t.remotePort}
                                </small>
                                {runtime.tunnels[t.id]?.status ===
                                  "connected" && (
                                  <DestinationStatus
                                    health={runtime.tunnelHealth?.[
                                      t.id
                                    ]?.filter((h) => h.remotePort === p.port)}
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
                            <span className="text-[12px] leading-[1.65] text-muted-foreground">
                              Not forwarded
                            </span>
                          )}
                        </TableCell>
                        <TableCell>
                          {!forwards.length && (
                            <Button
                              variant="ghost"

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
            </>
          ) : (
            <SectionEmpty>No listening TCP ports found.</SectionEmpty>
          )
        ) : null}
      </section>
    </div>
  );
}
