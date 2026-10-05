import type { ReactNode } from "react";
import {
  Copy,
  ExternalLink,
  MoreHorizontal,
  Pencil,
  SlidersHorizontal,
  Trash2,
} from "lucide-react";
import { cn } from "../lib/utils";
import { destinationSummary } from "./DestinationStatus";
import { Button } from "./controls";
import { Switch } from "./ui/switch";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import { forwardingHost, localPortFor } from "../forwarding";
import { processDetails } from "../portDetails";
import {
  forwardName,
  serviceDetail,
  serviceName,
  type PortEntry,
} from "../portProjects";
import type { TunnelActions } from "../hooks/useTunnelActions";
import {
  active,
  direction,
  newTunnel,
  range,
  tunnelState,
  type ConnectionState,
  type DestinationHealth,
  type Snapshot,
  type Tunnel,
  type TunnelDeletion,
  type TunnelEditor,
} from "../types";

export type Shared = {
  serverId: string;
  /** The project's key, filed on new forwards; null for system services. */
  projectKey: string | null;
  /** The checkout these rows run from, so paths read relative to it. */
  base: string | null;
  runtime: Snapshot["runtime"];
  actions: TunnelActions;
  edit: (editor: Omit<TunnelEditor, "kind" | "projects">) => void;
  setDeleting: (target: TunnelDeletion) => void;
};

const savedName = (t: Tunnel) => t.name || `Port ${t.localPort}`;
// Row controls that only matter on the row in hand stay quiet until it is.
const quiet =
  "opacity-0 transition-opacity group-hover/row:opacity-100 group-focus-within/row:opacity-100 data-[state=open]:opacity-100 motion-reduce:transition-none";

/** The strongest state across a row's forwards, for its dot and switch. */
function rowState(states: ConnectionState[]): ConnectionState | null {
  for (const status of [
    "error",
    "reconnecting",
    "connecting",
    "connected",
  ] as const) {
    const found = states.find((s) => s.status === status);
    if (found) return found;
  }
  return states[0] ?? null;
}

export function StateDot({ state }: { state: ConnectionState | null }) {
  const status = state?.status;
  return (
    <span
      aria-hidden="true"
      className={cn(
        "inline-block size-[8px] shrink-0 rounded-full",
        (!state || status === "disconnected") &&
          "border-[1.5px] border-muted-foreground/35",
        status === "connected" &&
          "bg-(--success) shadow-[0_0_0_3px_color-mix(in_srgb,var(--success)_15%,transparent)]",
        (status === "connecting" || status === "reconnecting") &&
          "animate-pulse bg-(--warning) motion-reduce:animate-none",
        status === "error" && "bg-(--danger)",
      )}
    />
  );
}

/** Where a forward lands, read from this Mac's side. */
function mapping(t: Tunnel, remotePort?: number) {
  if (direction(t) === "remote") {
    const host =
      forwardingHost(t.remoteHost) === "127.0.0.1" ? "server" : t.remoteHost;
    return `${host}:${range(t.remotePort, t.remotePortEnd)}`;
  }
  return `localhost:${
    remotePort === undefined
      ? range(t.localPort, t.localPortEnd)
      : localPortFor(t, remotePort)
  }`;
}

/** The address a forward gives you: a link once it is up, plain until then. */
function Route({
  forwards,
  state,
  remotePort,
  label,
  actions,
}: {
  forwards: Tunnel[];
  state: ConnectionState | null;
  remotePort?: number;
  label: string;
  actions: TunnelActions;
}) {
  const local = forwards.find((t) => direction(t) === "local");
  const reverse = forwards.find((t) => direction(t) === "remote");
  const up = state?.status === "connected";
  if (local && up)
    return (
      <button
        type="button"
        aria-label={`Open ${label} in browser`}
        title={`Open http://localhost:${local.localPort}`}
        onClick={() => actions.open(local.id)}
        className="inline-flex h-[24px] items-center gap-[5px] rounded-[6px] bg-primary/8 px-[8px] font-mono text-[11.5px] text-primary outline-none transition-colors hover:bg-primary/14 focus-visible:outline-2 focus-visible:outline-ring motion-reduce:transition-none dark:bg-primary/15 dark:hover:bg-primary/25"
      >
        {mapping(local, remotePort)}
        <ExternalLink size={11} aria-hidden="true" />
      </button>
    );
  const shown = local ?? reverse;
  if (!shown) return null;
  return (
    <span
      className={cn(
        "px-[8px] font-mono text-[11.5px]",
        up ? "text-foreground" : "text-muted-foreground",
      )}
    >
      {mapping(shown, local ? remotePort : undefined)}
    </span>
  );
}

function RowMenu({ label, children }: { label: string; children: ReactNode }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className={cn("size-[26px]", quiet)}
          aria-label={`Actions for ${label}`}
        >
          <MoreHorizontal size={15} />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">{children}</DropdownMenuContent>
    </DropdownMenu>
  );
}

function ForwardMenuItems({
  forwards,
  edit,
  setDeleting,
}: Pick<Shared, "edit" | "setDeleting"> & { forwards: Tunnel[] }) {
  const local = forwards.find((t) => direction(t) === "local");
  const several = forwards.length > 1;
  return (
    <>
      {forwards.map((t) => (
        <DropdownMenuItem
          key={`edit-${t.id}`}
          aria-label={`Edit ${savedName(t)}`}
          onSelect={() => edit({ value: t, existing: true })}
        >
          <Pencil size={14} /> Edit {several ? savedName(t) : "forward"}
        </DropdownMenuItem>
      ))}
      {local && (
        <DropdownMenuItem
          onSelect={() =>
            void navigator.clipboard?.writeText(`localhost:${local.localPort}`)
          }
        >
          <Copy size={14} /> Copy local address
        </DropdownMenuItem>
      )}
      <DropdownMenuSeparator />
      {forwards.map((t) => (
        <DropdownMenuItem
          key={`delete-${t.id}`}
          variant="destructive"
          aria-label={`Delete ${savedName(t)}`}
          onSelect={() =>
            setDeleting({ kind: "tunnel", id: t.id, name: savedName(t) })
          }
        >
          <Trash2 size={14} /> Delete {several ? savedName(t) : "forward"}
        </DropdownMenuItem>
      ))}
    </>
  );
}

/** What is wrong with a row's forwards, if anything, in one sentence. */
function problemFor(
  state: ConnectionState | null,
  health: DestinationHealth[],
) {
  if (state?.status === "error" || state?.errorMessage)
    return state.errorMessage ?? "The forward stopped.";
  if (state?.status !== "connected") return null;
  const issue = health.find((h) => h.message);
  return issue ? `${destinationSummary(health)}. ${issue.message}` : null;
}

/** A row's settled or moving state, in words. */
export function statusText(state: ConnectionState | null) {
  if (!state) return "Not forwarded";
  if (state.status === "reconnecting" && state.reconnectAttempt > 0)
    return `Retrying ${state.reconnectAttempt}/10…`;
  if (state.status === "connecting") return "Connecting…";
  return state.status[0].toUpperCase() + state.status.slice(1);
}

/**
 * Everything a row shows and does, independent of layout: the Projects list
 * and the Ports table render the same model, so they cannot drift apart.
 */
export interface RowModel {
  key: string;
  label: string;
  state: ConnectionState | null;
  /** The port shown in the port column, and its number for sorting. */
  port: string;
  portNumber: number;
  title: string;
  detail: string;
  tooltip?: string;
  problem: string | null;
  onFix?: () => void;
  route: ReactNode;
  menu: ReactNode;
  control: ReactNode;
  /** A saved forward exists for this row. */
  live: boolean;
}

/** The name, supporting line and announced state of a row. */
export function Identity({ model }: { model: RowModel }) {
  const { state, title, detail, tooltip, problem, onFix, live } = model;
  const moving =
    state?.status === "connecting" || state?.status === "reconnecting";
  return (
    <div className="min-w-0" title={tooltip}>
      <div
        className={cn(
          "truncate text-[13px] leading-[1.35]",
          live
            ? "font-semibold text-foreground"
            : "font-medium text-foreground/80",
        )}
      >
        {title}
      </div>
      {problem ? (
        <div className="mt-[1px] line-clamp-2 text-[11.5px] leading-[1.45] text-(--danger) wrap-anywhere">
          {problem}
          {onFix && (
            <>
              {" "}
              <button
                type="button"
                onClick={onFix}
                className="font-medium underline-offset-2 hover:underline focus-visible:underline focus-visible:outline-none"
              >
                Edit forward
              </button>
            </>
          )}
        </div>
      ) : (
        <div
          className={cn(
            "mt-[1px] truncate text-[11.5px] leading-[1.45]",
            moving ? "text-(--warning)" : "text-muted-foreground",
          )}
        >
          {moving ? statusText(state) : detail}
        </div>
      )}
      {/* Settled states are announced; the dot and switch show them. */}
      {!moving && <span className="sr-only">{statusText(state)}</span>}
    </div>
  );
}

export function PortNumber({ model }: { model: RowModel }) {
  return (
    <code
      className={cn(
        "text-[13px] font-medium tabular-nums",
        model.live ? "text-foreground" : "text-muted-foreground",
      )}
    >
      {model.port}
    </code>
  );
}

/**
 * One row of the Projects list. Every row has the same columns, so ports,
 * names, addresses and switches line up across projects and worktrees. A
 * forwarded row reads at full strength; a port that is only listening
 * recedes.
 */
function Row({ model }: { model: RowModel }) {
  return (
    <li
      data-slot="port-row"
      aria-label={model.label}
      className="group/row grid min-h-[50px] grid-cols-[8px_50px_minmax(0,1fr)_auto_26px_36px] items-center gap-x-[12px] rounded-[8px] px-[12px] py-[7px] transition-colors hover:bg-accent/50 focus-within:bg-accent/50 motion-reduce:transition-none @max-[560px]/ports:grid-cols-[8px_44px_minmax(0,1fr)_26px_36px] @max-[560px]/ports:gap-x-[10px]"
    >
      <StateDot state={model.state} />
      <PortNumber model={model} />
      <Identity model={model} />
      <div className="flex justify-end @max-[560px]/ports:col-start-3 @max-[560px]/ports:row-start-2 @max-[560px]/ports:justify-start @max-[560px]/ports:[&>*]:-ml-[8px] @max-[560px]/ports:empty:hidden">
        {model.route}
      </div>
      <div className="@max-[560px]/ports:col-start-4 @max-[560px]/ports:row-start-1">
        {model.menu}
      </div>
      <div className="flex justify-end @max-[560px]/ports:col-start-5 @max-[560px]/ports:row-start-1">
        {model.control}
      </div>
    </li>
  );
}

/**
 * A port listening on the server. Its switch is the one control: on forwards
 * it (saving a forward on the same local port the first time), off stops it.
 */
export function listeningModel({
  entry,
  serverId,
  projectKey,
  base,
  runtime,
  actions,
  edit,
  setDeleting,
}: Shared & { entry: PortEntry }): RowModel {
  const { port: p, forwards } = entry;
  const states = forwards.map((t) => tunnelState(runtime, t.id));
  const state = rowState(states);
  const name = serviceName(entry);
  const label = `port ${p.port}${name ? ` (${name})` : ""}`;
  const draft = (): Tunnel => ({
    ...newTunnel(serverId, p.port),
    remoteHost: forwardingHost(p.address),
    name: forwardName(entry),
    project: projectKey,
  });
  const health = forwards.flatMap((t) =>
    (runtime.tunnelHealth?.[t.id] ?? []).filter((h) => h.remotePort === p.port),
  );
  const pending = actions.pendingTargets.has(entry.key);
  const detail = serviceDetail(entry, base);
  // A saved name hides the service's own; keep it in the supporting line.
  if (forwards[0]?.name && name && forwards[0].name !== name)
    detail.unshift(name);
  return {
    key: entry.key,
    label,
    state,
    port: String(p.port),
    portNumber: p.port,
    title: forwards[0]?.name || name || "Owner not reported",
    detail: detail.join(" · "),
    tooltip: [
      p.containerName && `Container: ${p.containerName}`,
      ...processDetails(p).map((d) => `${d.label}: ${d.value}`),
      p.workingDirectoryDeleted &&
        "Its working directory was deleted while it kept running; it may be left over.",
      p.executableDeleted &&
        "Its program file changed since it started; restart it to run the new one.",
      `Listening on ${entry.addresses.join(", ")}`,
      p.pid ? `PID ${p.pid}${p.user ? ` · ${p.user}` : ""}` : null,
      p.command,
    ]
      .filter(Boolean)
      .join("\n"),
    problem: problemFor(state, health),
    onFix:
      forwards[0] && state?.status === "error"
        ? () => edit({ value: forwards[0], existing: true })
        : undefined,
    live: forwards.length > 0,
    route: forwards.length > 0 && (
      <Route
        forwards={forwards}
        state={state}
        remotePort={p.port}
        label={label}
        actions={actions}
      />
    ),
    menu: (
      <RowMenu label={label}>
        {forwards.length ? (
          <ForwardMenuItems
            forwards={forwards}
            edit={edit}
            setDeleting={setDeleting}
          />
        ) : (
          <DropdownMenuItem
            onSelect={() => edit({ value: draft(), existing: false })}
          >
            <SlidersHorizontal size={14} /> Forward with options…
          </DropdownMenuItem>
        )}
      </RowMenu>
    ),
    control: (
      <Switch
        aria-label={`Forward ${label}`}
        checked={(!!state && active(state.status)) || pending}
        disabled={
          pending ||
          forwards.some((t) => actions.pending.has(t.id)) ||
          actions.allPending
        }
        onCheckedChange={(checked) => {
          if (!forwards.length) {
            if (checked) actions.forward([draft()], [], entry.key);
            return;
          }
          forwards.forEach((t, i) => {
            if (active(states[i]?.status) !== checked)
              actions.toggle(t.id, checked);
          });
        }}
      />
    ),
  };
}

/** A saved forward no listening port accounts for: reverse, or idle. */
export function savedModel({
  tunnel: t,
  runtime,
  actions,
  edit,
  setDeleting,
}: Shared & { tunnel: Tunnel }): RowModel {
  const state = tunnelState(runtime, t.id);
  const reverse = direction(t) === "remote";
  const label = savedName(t);
  return {
    key: t.id,
    label,
    state,
    port: range(
      reverse ? t.localPort : t.remotePort,
      reverse ? t.localPortEnd : t.remotePortEnd,
    ),
    portNumber: reverse ? t.localPort : t.remotePort,
    title: label,
    detail: reverse
      ? `This Mac's port ${range(t.localPort, t.localPortEnd)}, opened on the server`
      : `Nothing listening on ${t.remoteHost}:${range(t.remotePort, t.remotePortEnd)} yet`,
    problem: problemFor(state, runtime.tunnelHealth?.[t.id] ?? []),
    onFix:
      state.status === "error"
        ? () => edit({ value: t, existing: true })
        : undefined,
    live: true,
    route: (
      <Route forwards={[t]} state={state} label={label} actions={actions} />
    ),
    menu: (
      <RowMenu label={label}>
        <ForwardMenuItems
          forwards={[t]}
          edit={edit}
          setDeleting={setDeleting}
        />
      </RowMenu>
    ),
    control: (
      <Switch
        aria-label={`Forward ${label}`}
        checked={active(state.status)}
        disabled={actions.pending.has(t.id) || actions.allPending}
        onCheckedChange={(checked) => actions.toggle(t.id, checked)}
      />
    ),
  };
}

export function ListeningPortRow(props: Shared & { entry: PortEntry }) {
  return <Row model={listeningModel(props)} />;
}
export function SavedForwardRow(props: Shared & { tunnel: Tunnel }) {
  return <Row model={savedModel(props)} />;
}
