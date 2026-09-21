import { useMutation } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { desktop } from "../api/desktop";
import { Search, MoreHorizontal, Layers, Box } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "./ui/dropdown-menu";
import { Button, Input } from "./controls";
import { Modal } from "./Editors";
import { groupContainers, type Container } from "../docker";
import { type Server } from "../types";
import { type Log } from "../domain/cockpit";
import { useCollection } from "../hooks/useCollection";
import { CollectionBar } from "./CollectionBar";

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
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState("");
  const [confirmation, setConfirmation] = useState<{
    project: string;
    action: "start" | "stop" | "restart";
    containers: Container[];
  } | null>(null);
  const [notice, setNotice] = useState("");
  const mutation = useMutation({
    mutationFn: async (target: NonNullable<typeof confirmation>) => {
      await desktop("cockpit_project_action", {
        id,
        project: target.project,
        action: target.action,
        expectedIds: target.containers.map((c) => c.id),
      });
    },
    onMutate: () => setNotice(""),
    onSuccess: (_, target) => {
      setNotice(`${target.project}: ${target.action} completed.`);
      setConfirmation(null);
    },
    // A failed command may have partially changed the project too.
    onSettled: () => state.refresh(),
  });
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
  const row = (c: Container) => (
    <li data-scroll-anchor className="service-row container-row" key={c.id}>
      <div className="container-identity">
        <strong>{c.composeService || c.name}</strong>
        {c.composeService && (
          <small>
            {c.name}
            {c.composeOneoff?.toLowerCase() === "true" ? " · One-off task" : ""}
          </small>
        )}
      </div>
      <div className="container-image">
        <span>{c.image}</span>
        {c.ports && <code title="Container ports">{c.ports}</code>}
      </div>
      <div className="container-state">
        <span
          className={`status ${c.status.toLowerCase().includes("unhealthy") ? "container-warning" : c.state === "running" ? "connected" : ""}`}
        >
          <span className="status-dot" />
          {c.status}
        </span>
      </div>
      <Button
        variant="ghost"
        aria-label={`View logs for ${c.name}`}
        onClick={(event) =>
          onLogs(
            { source: "container", target: c.id, title: c.name },
            event.currentTarget,
          )
        }
      >
        Logs
      </Button>
    </li>
  );
  return (
    <div className="containers-panel" ref={scopeRef}>
      <div className="container-masthead">
        <div>
          <h2>Docker & Compose</h2>
          <p>
            {state.data
              ? `${state.data.length} containers · ${grouped.projects.length} ${grouped.projects.length === 1 ? "project" : "projects"}`
              : "Container inventory"}
          </p>
        </div>
        <CollectionBar {...state} />
      </div>
      {notice && (
        <p role="status" className="container-notice">
          {notice}
        </p>
      )}
      <div className="container-filterbar">
        <div className="search cockpit-search">
          <Search size={14} />
          <Input
            aria-label="Filter Docker projects and containers"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Find a project, service, or container"
          />
        </div>
        <div className="container-tools">
          <span className="muted">
            {state.data
              ? `${state.data.filter((c) => c.state === "running").length} running · ${state.data.filter((c) => c.state !== "running").length} not running`
              : state.error
                ? "Containers unavailable"
                : "Reading containers…"}
          </span>
          {query && (
            <Button
              variant="ghost"
              className="text-button"
              onClick={() => setQuery("")}
            >
              Clear filter
            </Button>
          )}
          {projects.length > 0 && (
            <Button
              variant="ghost"
              className="text-button"
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
        <p className="collection-count">
          Showing {projects.length} matching projects and {standalone.length}{" "}
          standalone containers. Matching projects include all their containers;
          project actions apply to the whole project.
        </p>
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
          <div className="container-project" key={p.name}>
            <details
              className="compose-project"
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
              <summary>
                <Layers size={17} className="project-icon" />
                <span>
                  <strong>{p.name}</strong>
                  <small>
                    {p.services.length} service
                    {p.services.length === 1 ? "" : "s"} · {running}/
                    {services.length} service containers running
                    {p.containers.length > services.length &&
                      ` · ${p.containers.length - services.length} one-off ${p.containers.length - services.length === 1 ? "job" : "jobs"}`}
                  </small>
                </span>
                <span
                  className={`status ${projectStatus === "Running" ? "connected" : unhealthy ? "container-warning" : ""}`}
                >
                  {projectStatus}
                </span>
              </summary>
              <div className="compose-project-body">
                <div className="container-columns" aria-hidden="true">
                  <span>Container</span>
                  <span>Image / ports</span>
                  <span>Status</span>
                  <span />
                </div>
                <ul
                  className="service-list"
                  role="list"
                  aria-label={`${p.name} containers`}
                >
                  {p.containers.map(row)}
                </ul>
              </div>
            </details>
            <div className="container-project-controls">
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
                    className="icon"
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
            </div>
          </div>
        );
      })}
      {standalone.length > 0 && (
        <section className="container-standalone">
          <h2>
            <Box size={16} /> Standalone containers{" "}
            <span>{standalone.length}</span>
          </h2>
          <div className="container-columns" aria-hidden="true">
            <span>Container</span>
            <span>Image / ports</span>
            <span>Status</span>
            <span />
          </div>
          <ul
            className="service-list"
            role="list"
            aria-label="Standalone containers"
          >
            {standalone.map(row)}
          </ul>
        </section>
      )}
      {state.data && !projects.length && !standalone.length && (
        <p className="section-empty">
          {query
            ? "No matches. Try another name or clear the filter."
            : "No Docker containers on this server."}
        </p>
      )}
      {state.data && (
        <details className="container-scope">
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
          <div className="form-body">
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
              className="compose-confirmation-list"
              tabIndex={0}
              aria-label="Affected service containers"
            >
              {confirmation.containers.map((c) => (
                <li key={c.id}>
                  {c.composeService || c.name} <small>({c.name})</small>
                </li>
              ))}
            </ul>
            <p className="muted">
              Existing containers only; one-off jobs are excluded. No image
              pulls, recreation, or configuration changes. Compose dependency
              order is not applied.
            </p>
            {actionError && (
              <p role="alert" className="cockpit-error">
                {actionError}
              </p>
            )}
          </div>
          <footer className="dialog-footer">
            <Button disabled={actionBusy} onClick={() => setConfirmation(null)}>
              Cancel
            </Button>
            <Button
              loading={actionBusy}
              variant={
                confirmation.action === "start" ? "default" : "destructive"
              }
              className={
                confirmation.action === "start" ? "primary" : "destructive"
              }
              onClick={() => void perform()}
            >
              {actionBusy
                ? "Working…"
                : `${confirmation.action[0].toUpperCase() + confirmation.action.slice(1)} project`}
            </Button>
          </footer>
        </Modal>
      )}
    </div>
  );
}
