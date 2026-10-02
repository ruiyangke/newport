import { SectionEmpty } from "./Feedback";
import {
  ContainerProject,
  ContainerProjectDetails,
  ContainerProjectSummary,
  ContainerProjectControls,
} from "./ContainerProject";
import { SearchField } from "./SearchField";
import { EditorBody, EditorFooter } from "./EditorLayout";
import { ContainerRow, ContainerColumns } from "./ContainerRow";
import { CollectionCount } from "./SectionHeader";
import { useWorkspaceState } from "../state/workspace";
import { useSharedMutation } from "../hooks/useSharedMutation";
import { keys, useServerScope } from "../query/keys";
import { useRef, useState } from "react";
import { desktop } from "../api/desktop";
import { MoreHorizontal, Layers, Box } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "./ui/dropdown-menu";
import { Button, RefreshButton } from "./controls";
import { PageHeader } from "./PageHeader";
import { Modal } from "./Editors";
import { groupContainers, type Container } from "../docker";
import { type Server } from "../types";
import { type Log } from "../domain/cockpit";
import { useCollection } from "../hooks/useCollection";

export function ContainersPanel({
  server,
  onLogs,
}: {
  server: Server;
  onLogs: (log: Log, origin: HTMLElement) => void;
}) {
  const scopeRef = useRef<HTMLDivElement>(null);
  const id = server.id;
  const state = useCollection(id, "containers", false, scopeRef);
  const [collapsed, setCollapsed] = useWorkspaceState(
    server.id,
    "containers.collapsed",
  );
  const [query, setQuery] = useWorkspaceState(server.id, "containers.query");
  const [confirmation, setConfirmation] = useState<{
    project: string;
    action: "start" | "stop" | "restart";
    containers: Container[];
  } | null>(null);
  const scope = useServerScope(id);
  const mutation = useSharedMutation({
    mutationKey: [...keys.server(scope), "project-action"],
    mutationFn: async (target: NonNullable<typeof confirmation>) => {
      await desktop("cockpit_project_action", {
        id,
        project: target.project,
        action: target.action,
        expectedIds: target.containers.map((c) => c.id),
      });
    },
    onSuccess: () => {
      setConfirmation(null);
    },
    // A failed command may have partially changed the project too.
    onSettled: () => state.refresh(),
  });
  const notice =
    mutation.isSuccess && mutation.variables
      ? `${mutation.variables.project}: ${mutation.variables.action} completed.`
      : "";
  const actionBusy = mutation.isPending;
  const actionError = mutation.error ? String(mutation.error) : "";
  const perform = () => {
    if (confirmation && !actionBusy) mutation.mutate(confirmation);
  };
  const grouped = groupContainers(state.data ?? []);
  const matches = (c: Container) =>
    `${c.name} ${c.image} ${c.composeService ?? ""}`
      .toLowerCase()
      .includes(query.trim().toLowerCase());
  const projects = grouped.projects.filter(
    (p) =>
      p.name.toLowerCase().includes(query.trim().toLowerCase()) ||
      p.containers.some(matches),
  );
  const standalone = grouped.standalone.filter(matches);

  return (
    <div className="containers-panel" ref={scopeRef}>
      <PageHeader
        title="Docker & Compose"
        subtitle={
          state.data
            ? `${state.data.length} containers · ${grouped.projects.length} ${grouped.projects.length === 1 ? "project" : "projects"}`
            : "Container inventory"
        }
        actions={<RefreshButton busy={state.busy} onClick={state.refresh} />}
        meta={
          state.busy
            ? "Reading server…"
            : state.updated
              ? `${state.error ? "Last success" : "Updated"} ${state.updated.toLocaleTimeString()}`
              : "Not loaded"
        }
      />
      {state.error && (
        <p
          data-slot="cockpit-error"
          className="text-[12px] leading-[1.6] text-(--red) wrap-anywhere"
          role="alert"
        >
          {state.error}
          {state.updated && " Showing the last available reading."}
        </p>
      )}
      {notice && (
        <p
          role="status"
          data-slot="container-notice"
          className="text-[12px] text-(--green)"
        >
          {notice}
        </p>
      )}
      <div
        data-slot="container-filterbar"
        className="mb-[16px] flex flex-wrap items-center gap-[16px]"
      >
        <SearchField
          className="min-w-[200px] flex-1"
          aria-label="Filter Docker projects and containers"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Find a project, service, or container"
        />

        <div
          data-slot="container-tools"
          className="flex items-center gap-[8px] text-[11px] max-[800px]:w-full [&>span]:whitespace-nowrap max-[800px]:[&>span]:mr-auto"
        >
          <span className="text-[12px] leading-[1.65] text-muted-foreground">
            {state.data
              ? `${state.data.filter((c) => c.state === "running").length} running · ${state.data.filter((c) => c.state !== "running").length} not running`
              : state.error
                ? "Containers unavailable"
                : "Reading containers…"}
          </span>
          {query && (
            <Button
              variant="ghost"

              onClick={() => setQuery("")}
            >
              Clear filter
            </Button>
          )}
          {projects.length > 0 && (
            <Button
              variant="ghost"

              onClick={() =>
                setCollapsed((previous) => {
                  const next = new Set(previous);
                  const expand = projects.every((p) => previous.has(p.name));
                  projects.forEach((p) =>
                    expand ? next.delete(p.name) : next.add(p.name),
                  );
                  return next;
                })
              }
            >
              {projects.every((p) => collapsed.has(p.name))
                ? "Expand all"
                : "Collapse all"}
            </Button>
          )}
        </div>
      </div>
      {query.trim() && state.data && (
        <CollectionCount>
          Showing {projects.length}{" "}
          {projects.length === 1 ? "project" : "projects"} and{" "}
          {standalone.length} standalone{" "}
          {standalone.length === 1 ? "container" : "containers"}. Matching
          projects include all of their containers; project actions apply to the
          whole project.
        </CollectionCount>
      )}
      {projects.map((p) => {
        const services = p.containers.filter(
          (c) => c.composeOneoff?.toLowerCase() !== "true",
        );
        const running = services.filter((c) => c.state === "running").length;
        const unhealthy = services.some((c) =>
          c.status.toLowerCase().includes("unhealthy"),
        );
        const projectStatus = !services.length
          ? "Jobs only"
          : unhealthy
            ? "Unhealthy"
            : running === services.length
              ? "Running"
              : running === 0
                ? "Stopped"
                : "Partially running";
        return (
          <ContainerProject key={p.name}>
            <ContainerProjectDetails
              open={!collapsed.has(p.name)}
              onToggle={(event) => {
                const open = event.currentTarget.open;
                setCollapsed((previous) => {
                  if (previous.has(p.name) === !open) return previous;
                  const next = new Set(previous);
                  if (open) next.delete(p.name);
                  else next.add(p.name);
                  return next;
                });
              }}
            >
              <ContainerProjectSummary>
                <Layers
                  size={17}
                  data-slot="project-icon"
                  className="shrink-0 text-muted-foreground"
                />
                <span className="min-w-0 flex-1 wrap-anywhere">
                  <strong className="text-[13px] font-[550]">{p.name}</strong>
                  <small className="mt-[3px] block text-[11px]">
                    {p.services.length} service
                    {p.services.length === 1 ? "" : "s"} · {running}/
                    {services.length} service containers running
                    {p.containers.length > services.length &&
                      ` · ${p.containers.length - services.length} one-off ${p.containers.length - services.length === 1 ? "job" : "jobs"}`}
                  </small>
                </span>
                <span
                  data-slot="container-project-status"
                  className={`inline-flex items-center gap-[6px] text-[11px] whitespace-nowrap max-[800px]:ml-[26px] max-[800px]:basis-full ${projectStatus === "Running" ? "text-(--green)" : unhealthy ? "text-(--orange)" : "text-muted-foreground"}`}
                >
                  {projectStatus}
                </span>
              </ContainerProjectSummary>
              <div data-slot="compose-project-body" className="p-0">
                <ContainerColumns />
                <ul
                  data-slot="service-list"
                  className="m-0 list-none p-0"
                  role="list"
                  aria-label={`${p.name} containers`}
                >
                  {p.containers.map((container) => (
                    <ContainerRow
                      key={container.id}
                      container={container}
                      onLogs={onLogs}
                    />
                  ))}
                </ul>
              </div>
            </ContainerProjectDetails>
            <ContainerProjectControls>
              {" "}
              <Button
                onClick={(event) =>
                  onLogs(
                    {
                      source: "compose",
                      target: p.name,
                      title: `${p.name} · Project logs`,
                    },
                    event.currentTarget,
                  )
                }
                aria-label={`View project logs for ${p.name}`}
              >
                Project logs
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"

                    aria-label={`Actions for project ${p.name}`}
                  >
                    <MoreHorizontal size={16} />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  {" "}
                  {(["start", "stop", "restart"] as const).map((action) => (
                    <DropdownMenuItem
                      key={action}
                      disabled={
                        actionBusy ||
                        state.busy ||
                        (action === "start" && running === services.length) ||
                        (action === "stop" && running === 0) ||
                        !p.containers.some(
                          (c) => c.composeOneoff?.toLowerCase() !== "true",
                        )
                      }
                      aria-label={`${action[0].toUpperCase() + action.slice(1)} project ${p.name}`}
                      onSelect={() => {
                        mutation.reset();
                        setConfirmation({
                          project: p.name,
                          action,
                          containers: p.containers.filter(
                            (c) => c.composeOneoff?.toLowerCase() !== "true",
                          ),
                        });
                      }}
                    >
                      {action[0].toUpperCase() + action.slice(1)}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            </ContainerProjectControls>
          </ContainerProject>
        );
      })}
      {standalone.length > 0 && (
        <section
          data-slot="container-standalone"
          className="mt-[20px] overflow-hidden rounded-[6px] border border-border"
        >
          <h2 className="m-0 flex items-center gap-[8px] bg-(--native-toolbar) px-[16px] py-[14px] text-[13px]">
            <Box size={16} /> Standalone containers{" "}
            <span className="ml-auto font-normal text-muted-foreground">
              {standalone.length}
            </span>
          </h2>
          <ContainerColumns />
          <ul
            data-slot="service-list"
            className="m-0 list-none p-0"
            role="list"
            aria-label="Standalone containers"
          >
            {standalone.map((container) => (
              <ContainerRow
                key={container.id}
                container={container}
                onLogs={onLogs}
              />
            ))}
          </ul>
        </section>
      )}
      {state.data && !projects.length && !standalone.length && (
        <SectionEmpty>
          {query
            ? "No matches. Try another name or clear the filter."
            : "No Docker containers on this server."}
        </SectionEmpty>
      )}
      {state.data && (
        <details
          data-slot="container-scope"
          className="my-[16px] text-[11px] text-muted-foreground [&>p]:my-[8px] [&>p]:max-w-[70ch]"
        >
          <summary>About this inventory</summary>
          <p>
            Up to 500 containers, including stopped ones. Empty projects are
            hidden. Project controls exclude one-off jobs.
          </p>
        </details>
      )}
      {confirmation && (
        <Modal
          title={`${confirmation.action[0].toUpperCase() + confirmation.action.slice(1)} ${confirmation.project}?`}
          busy={actionBusy}
          onClose={() => setConfirmation(null)}
          describedBy="container-action-description"
        >
          <EditorBody>
            <p id="container-action-description">
              This will {confirmation.action} {confirmation.containers.length}{" "}
              existing service containers in{" "}
              <strong>{confirmation.project}</strong> on{" "}
              <strong>{server.name || server.sshHost}</strong> ({server.sshUser}
              @{server.sshHost}).
              {confirmation.action !== "start" &&
                " Active connections to these services may be interrupted."}
            </p>
            <ul
              data-slot="compose-confirmation-list"
              className="max-h-[180px] list-disc overflow-auto pl-[20px] leading-[1.7] wrap-anywhere focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
              tabIndex={0}
              aria-label="Affected service containers"
            >
              {confirmation.containers.map((c) => (
                <li key={c.id}>
                  {c.composeService || c.name} <small>({c.name})</small>
                </li>
              ))}
            </ul>
            <p className="text-[12px] leading-[1.65] text-muted-foreground">
              Existing containers only; one-off jobs are excluded. No image
              pulls, recreation, or configuration changes. Compose dependency
              order is not applied.
            </p>
            {actionError && (
              <p
                role="alert"
                data-slot="cockpit-error"
                className="text-[12px] leading-[1.6] text-(--red) wrap-anywhere"
              >
                {actionError}
              </p>
            )}
          </EditorBody>
          <EditorFooter>
            <Button disabled={actionBusy} onClick={() => setConfirmation(null)}>
              Cancel
            </Button>
            <Button
              loading={actionBusy}
              variant={
                confirmation.action === "start" ? "default" : "destructive"
              }
              onClick={() => void perform()}
            >
              {actionBusy
                ? "Working…"
                : `${confirmation.action[0].toUpperCase() + confirmation.action.slice(1)} project`}
            </Button>
          </EditorFooter>
        </Modal>
      )}
    </div>
  );
}
