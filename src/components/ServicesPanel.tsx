import { useState } from "react";
import { Search } from "lucide-react";
import { Button, RefreshButton, Input, Select, SelectItem } from "./controls";
import "./services.css";

export type { Service as SystemService } from "../domain/cockpit";
import type { Service as SystemService, Log } from "../domain/cockpit";
export interface ServicesCollection {
  data: SystemService[] | null;
  error: string;
  busy: boolean;
  updated: Date | null;
  refresh: () => void;
}

export type ServiceLog = Log;

type ServiceFilter = "all" | "failed" | "active" | "inactive";

export function ServicesPanel({
  state,
  onLogs,
}: {
  state: ServicesCollection;
  onLogs: (log: ServiceLog, origin: HTMLElement) => void;
}) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<ServiceFilter>("all");
  const normalizedQuery = query.trim().toLowerCase();
  const filtered = normalizedQuery !== "" || filter !== "all";
  const rows = state.data
    ?.filter(
      (service) =>
        (filter === "all" || service.active === filter) &&
        `${service.name} ${service.description} ${service.active} ${service.sub} ${service.load}`
          .toLowerCase()
          .includes(normalizedQuery),
    )
    .sort(
      (a, b) =>
        Number(b.active === "failed") - Number(a.active === "failed") ||
        a.name.localeCompare(b.name),
    );
  const failed =
    state.data?.filter((service) => service.active === "failed").length ?? 0;
  const active =
    state.data?.filter((service) => service.active === "active").length ?? 0;

  return (
    <div className="services-panel">
      <div className="services-masthead">
        <div>
          <h2>System services</h2>
          <p>
            {state.data
              ? `${state.data.length} loaded · ${active} active · ${failed} failed`
              : "Loaded systemd services"}
          </p>
        </div>
        <div className="services-refresh">
          <span className="muted">
            {state.busy
              ? "Reading server…"
              : state.updated
                ? `${state.error ? "Last success" : "Updated"} ${state.updated.toLocaleTimeString()}`
                : "Not loaded"}
          </span>
          <RefreshButton busy={state.busy} onClick={state.refresh} />
        </div>
      </div>
      {state.error && (
        <p className="cockpit-error" role="alert">
          {state.error}
          {state.data
            ? " Showing previous results. Refresh to retry."
            : " Refresh to retry."}
        </p>
      )}
      <div className="services-filterbar">
        <div className="search cockpit-search">
          <Search size={14} aria-hidden="true" />
          <Input
            aria-label="Filter services"
            placeholder="Find a service or state"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </div>
        <Select
          aria-label="Filter by service state"
          value={filter}
          onValueChange={(value) => setFilter(value as ServiceFilter)}
        >
          <SelectItem value="all">All states</SelectItem>
          <SelectItem value="failed">Failed</SelectItem>
          <SelectItem value="active">Active</SelectItem>
          <SelectItem value="inactive">Inactive</SelectItem>
        </Select>
        <Button
          onClick={(event) =>
            onLogs(
              { source: "journal", target: "", title: "System journal" },
              event.currentTarget,
            )
          }
        >
          System journal
        </Button>
      </div>
      <div className="services-results">
        <p className="collection-count">
          {rows
            ? `Showing ${rows.length} of ${state.data?.length} services`
            : state.error
              ? "Service inventory unavailable"
              : "Reading services…"}
        </p>
        {filtered && (
          <Button
            variant="ghost"
            className="text-button"
            onClick={() => {
              setQuery("");
              setFilter("all");
            }}
          >
            Clear filters
          </Button>
        )}
      </div>
      <div className="services-ledger" aria-busy={state.busy}>
        <div className="services-columns" aria-hidden="true">
          <span>Service</span>
          <span>State</span>
          <span>Logs</span>
        </div>
        <ul className="services-list" role="list" aria-label="System services">
          {rows?.map((service) => (
            <li data-scroll-anchor className="services-row" key={service.name}>
              <div className="services-identity">
                <strong>{service.name}</strong>
                {service.description && <small>{service.description}</small>}
              </div>
              <div className="services-state">
                <span
                  className={`status ${service.active === "failed" ? "error" : service.active === "active" ? "connected" : ""}`}
                >
                  <span className="status-dot" aria-hidden="true" />
                  {service.active[0]?.toUpperCase()}
                  {service.active.slice(1)}
                </span>
                {service.sub !== service.active && <small>{service.sub}</small>}
                {service.load !== "loaded" && (
                  <small>Load: {service.load}</small>
                )}
              </div>
              <Button
                variant="ghost"
                aria-label={`View logs for ${service.name}`}
                onClick={(event) =>
                  onLogs(
                    {
                      source: "service",
                      target: service.name,
                      title: service.name,
                    },
                    event.currentTarget,
                  )
                }
              >
                Logs
              </Button>
            </li>
          ))}
        </ul>
        {!state.data && !state.error && (
          <p className="services-empty" role="status">
            Loading system services…
          </p>
        )}
        {!state.data && state.error && (
          <p className="services-empty">
            Refresh to load this server’s services.
          </p>
        )}
        {rows?.length === 0 && (
          <p className="services-empty" role="status">
            {filtered
              ? "No matching services. Try another name or state."
              : "No loaded services found."}
          </p>
        )}
      </div>
      <p className="services-scope">
        Up to 500 loaded services, including inactive ones. Failed services
        first.
      </p>
    </div>
  );
}
