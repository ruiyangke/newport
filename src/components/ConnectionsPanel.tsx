import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { ListTree, Plus, RefreshCw, Table2 } from "lucide-react";
import { PageHeader } from "./PageHeader";
import { Button } from "./controls";
import { SearchField } from "./SearchField";
import { SectionEmpty } from "./Feedback";
import { PortProjectSection } from "./PortProjectSection";
import { PortsTable } from "./PortsTable";
import { ToggleGroup, ToggleGroupItem } from "./ui/toggle-group";
import { useTunnelActions } from "../hooks/useTunnelActions";
import { usePorts } from "../hooks/usePorts";
import { useWorkspaceState } from "../state/workspace";
import { useServerScope } from "../query/keys";
import { gitQueries } from "../query/git";
import { processDetails } from "../portDetails";
import {
  OTHER,
  groupPorts,
  type PortEntry,
  type PortProject,
} from "../portProjects";
import {
  active,
  newTunnel,
  tunnelState,
  type Server,
  type Snapshot,
  type Tunnel,
  type TunnelDeletion,
  type TunnelEditor,
} from "../types";

type Props = {
  server: Server;
  tunnels: Tunnel[];
  runtime: Snapshot["runtime"];
  setEditor: (editor: TunnelEditor) => void;
  setDeleting: (target: TunnelDeletion) => void;
};

type Filter = "all" | "forwarded" | "attention";

function entryText(project: PortProject, entry: PortEntry) {
  const p = entry.port;
  return [
    project.name,
    project.path,
    p.port,
    ...entry.addresses,
    p.containerName,
    p.composeService,
    p.applicationName,
    p.processName,
    p.user,
    p.pid,
    ...entry.kinds,
    ...processDetails(p).map((detail) => detail.value),
    entry.worktree?.branch,
    entry.worktree?.path,
    ...entry.forwards.map((t) => `${t.name} ${t.localPort}`),
  ]
    .filter((value) => value !== null && value !== undefined)
    .join(" ")
    .toLowerCase();
}
function forwardText(project: PortProject, t: Tunnel) {
  return [
    project.name,
    project.path,
    t.name,
    t.localPort,
    t.remotePort,
    t.remoteHost,
  ]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
}

export function ConnectionsPanel({
  server,
  tunnels,
  runtime,
  setEditor,
  setDeleting,
}: Props) {
  const actions = useTunnelActions(server.id);
  const ports = usePorts(server.id);
  const scope = useServerScope(server.id);
  // Saved Git projects name the roots discovery finds; they are optional.
  const known = useQuery({
    ...gitQueries.projects(scope),
    select: (projects) =>
      (projects ?? []).map((p) => ({ name: p.name, path: p.path.display })),
  });
  const [query, setQuery] = useWorkspaceState(server.id, "ports.query");
  const [filter, setFilter] = useWorkspaceState(server.id, "ports.filter");
  const [collapsed, setCollapsed] = useWorkspaceState(
    server.id,
    "ports.collapsed",
  );
  const [view, setView] = useWorkspaceState(server.id, "ports.view");
  const [sorting, setSorting] = useWorkspaceState(server.id, "ports.sort");
  const [systemOpen, setSystemOpen] = useWorkspaceState(
    server.id,
    "ports.otherOpen",
  );
  const projects = useMemo(
    () => groupPorts(ports.data ?? [], tunnels, server.id, known.data),
    [ports.data, tunnels, server.id, known.data],
  );
  const choices = projects
    .filter((p) => p.key !== OTHER)
    .map((p) => ({ key: p.key, name: p.name }));
  const edit = (editor: Omit<TunnelEditor, "kind" | "projects">) =>
    setEditor({ kind: "tunnel", ...editor, projects: choices });

  const needle = query.trim().toLowerCase();
  // Forwards that failed, or are up but cannot reach their destination.
  const troubled = new Set(
    tunnels
      .filter((t) => {
        const state = tunnelState(runtime, t.id);
        return (
          state.status === "error" ||
          (state.status === "connected" &&
            runtime.tunnelHealth?.[t.id]?.some((h) => h.message))
        );
      })
      .map((t) => t.id),
  );
  const shown = troubled.size
    ? filter
    : filter === "attention"
      ? "all"
      : filter;
  const keep = (forwards: Tunnel[]) =>
    shown === "all" ||
    (shown === "forwarded"
      ? forwards.length > 0
      : forwards.some((t) => troubled.has(t.id)));
  const visible = projects
    .map((project) => ({
      project,
      ports: project.ports.filter(
        (entry) =>
          keep(entry.forwards) &&
          (!needle || entryText(project, entry).includes(needle)),
      ),
      forwards: project.forwards.filter(
        (t) =>
          keep([t]) && (!needle || forwardText(project, t).includes(needle)),
      ),
    }))
    .filter((group) => group.ports.length + group.forwards.length > 0);

  // Dual-stack listeners count once, as they show.
  const listening = projects.reduce((sum, p) => sum + p.ports.length, 0);
  const forwarding = tunnels.filter((t) =>
    active(tunnelState(runtime, t.id).status),
  ).length;
  const named = projects.filter((p) => p.key !== OTHER).length;
  const summary = (
    <>
      {forwarding} forwarding
      {ports.data && ` · ${listening} listening`}
      {ports.data &&
        named > 0 &&
        ` in ${named} ${named === 1 ? "project" : "projects"}`}
      {troubled.size > 0 && (
        <span className="text-(--danger)">
          {" "}
          · {troubled.size} {troubled.size === 1 ? "needs" : "need"} attention
        </span>
      )}
    </>
  );
  const empty = !projects.length;

  return (
    <div data-slot="connections-panel" className="@container/ports">
      <PageHeader
        title="Port forwarding"
        subtitle={summary}
        subtitleRole="status"
        meta={
          ports.updated
            ? `Updated ${ports.updated.toLocaleTimeString()} · refreshes every 30s`
            : undefined
        }
        actions={
          <>
            <Button
              variant="ghost"
              size="icon"
              loading={ports.discovering}
              onClick={ports.discover}
              aria-label="Discover ports"
              title="Discover listening ports now"
            >
              <RefreshCw size={14} />
            </Button>
            <Button
              variant="default"
              onClick={() =>
                edit({ value: newTunnel(server.id), existing: false })
              }
            >
              <Plus size={15} />
              New forward
            </Button>
          </>
        }
      />
      {actions.error && (
        <p
          role="alert"
          data-slot="connections-error"
          className="mb-[14px] flex items-start gap-[10px] rounded-[8px] border border-(--danger)/30 bg-(--danger)/5 px-[12px] py-[8px] text-[12px] leading-[1.6] text-(--danger) wrap-anywhere"
        >
          <span className="flex-1">{actions.error}</span>
          <Button variant="ghost" onClick={actions.dismissError}>
            Dismiss
          </Button>
        </p>
      )}
      {ports.error && (
        <p
          role="alert"
          data-slot="cockpit-error"
          className="mb-[14px] text-[12px] leading-[1.6] text-(--danger) wrap-anywhere"
        >
          Could not refresh ports: {ports.error.message}
          {ports.data && " Showing the last available reading."}
        </p>
      )}
      {!empty && (
        <div
          data-slot="ports-filterbar"
          className="mb-[10px] flex flex-wrap items-center gap-[10px] border-b border-border/70 pb-[12px]"
        >
          <SearchField
            className="max-w-[340px] min-w-[200px] flex-1"
            aria-label="Filter ports"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Find a port, project, process, or kind"
          />
          <span className="flex-1" />
          <ToggleGroup
            type="single"
            variant="outline"
            spacing={0}
            size="sm"
            aria-label="Show"
            value={shown}
            onValueChange={(value) => value && setFilter(value as Filter)}
          >
            <ToggleGroupItem value="all" className="px-[10px] text-[12px]">
              All
            </ToggleGroupItem>
            <ToggleGroupItem
              value="forwarded"
              className="px-[10px] text-[12px]"
            >
              Forwarded
            </ToggleGroupItem>
            {troubled.size > 0 && (
              <ToggleGroupItem
                value="attention"
                className="px-[10px] text-[12px] text-(--danger)"
              >
                Needs attention
              </ToggleGroupItem>
            )}
          </ToggleGroup>
          <ToggleGroup
            type="single"
            variant="outline"
            spacing={0}
            size="sm"
            aria-label="View"
            value={view}
            onValueChange={(value) =>
              value && setView(value as "projects" | "ports")
            }
          >
            <ToggleGroupItem
              value="projects"
              aria-label="Projects view"
              title="Group by project"
              className="px-[8px]"
            >
              <ListTree size={14} />
            </ToggleGroupItem>
            <ToggleGroupItem
              value="ports"
              aria-label="Ports view"
              title="All ports in one table"
              className="px-[8px]"
            >
              <Table2 size={14} />
            </ToggleGroupItem>
          </ToggleGroup>
        </div>
      )}
      {empty ? (
        ports.data || ports.error ? (
          <SectionEmpty as="div" className="text-center">
            <p className="text-[13px] font-medium text-foreground">
              Nothing to forward yet
            </p>
            <small>
              No TCP ports are listening on this server, and no forwards are
              saved. Start a service, or create a forward yourself.
            </small>
          </SectionEmpty>
        ) : (
          <SectionEmpty role="status">
            Discovering listening ports…
          </SectionEmpty>
        )
      ) : visible.length && view === "ports" ? (
        <PortsTable
          groups={visible}
          sorting={sorting}
          onSortingChange={setSorting}
          shared={{
            serverId: server.id,
            runtime,
            actions,
            edit,
            setDeleting,
          }}
        />
      ) : visible.length ? (
        <div data-slot="port-projects" className="flex flex-col">
          {visible.map(({ project, ports: rows, forwards }) => (
            <PortProjectSection
              key={project.key || "other"}
              project={project}
              ports={rows}
              forwards={forwards}
              serverId={server.id}
              tunnels={tunnels}
              runtime={runtime}
              actions={actions}
              open={
                !!needle ||
                (project.key === OTHER
                  ? // Other services stay out of the way once projects exist.
                    (systemOpen ?? named === 0)
                  : !collapsed.has(project.key))
              }
              onOpenChange={(open: boolean) =>
                project.key === OTHER
                  ? setSystemOpen(open)
                  : setCollapsed((previous) => {
                      const next = new Set(previous);
                      if (open) next.delete(project.key);
                      else next.add(project.key);
                      return next;
                    })
              }
              edit={edit}
              setDeleting={setDeleting}
            />
          ))}
        </div>
      ) : (
        <SectionEmpty>
          No matches. Try another port, project, process, or kind.
        </SectionEmpty>
      )}
      {!ports.data && !empty && ports.isFetching && (
        <p
          role="status"
          className="mt-[12px] text-[11px] text-muted-foreground"
        >
          Discovering listening ports…
        </p>
      )}
    </div>
  );
}
