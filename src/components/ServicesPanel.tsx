import { CollectionCount } from "./SectionHeader";
import { useWorkspaceState } from "../state/workspace";
import { Button, RefreshButton, Select, SelectItem } from "./controls";
import { PageHeader } from "./PageHeader";
import { ServiceRow, ServiceColumns } from "./ServiceRow";
import { SearchField } from "./SearchField";

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

export function ServicesPanel({
  id,
  state,
  onLogs,
}: {
  id: string;
  state: ServicesCollection;
  onLogs: (log: ServiceLog, origin: HTMLElement) => void;
}) {
  const [query, setQuery] = useWorkspaceState(id, "services.query");
  const [filter, setFilter] = useWorkspaceState(id, "services.filter");
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
    <div data-slot="services-panel">
      <PageHeader
        title="System services"
        subtitle={
          state.data
            ? `${state.data.length} loaded · ${active} active · ${failed} failed`
            : "Loaded systemd services"
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
          {state.data
            ? " Showing previous results. Refresh to retry."
            : " Refresh to retry."}
        </p>
      )}
      <div className="mb-[16px] flex flex-wrap items-center gap-[8px]">
        <SearchField
          className="min-w-[170px] flex-1"
          aria-label="Filter services"
          placeholder="Find a service or state"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <Select
          className="w-[112px]"
          aria-label="Filter by service state"
          value={filter}
          onValueChange={(value) =>
            setFilter(value as "all" | "failed" | "active" | "inactive")
          }
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
      {filtered && (
        <div className="flex min-h-[40px] items-center justify-between gap-[8px]">
          <CollectionCount className="m-0">
            {rows
              ? `Showing ${rows.length} of ${state.data?.length} services`
              : state.error
                ? "Service inventory unavailable"
                : "Reading services…"}
          </CollectionCount>
          <Button
            variant="ghost"

            onClick={() => {
              setQuery("");
              setFilter("all");
            }}
          >
            Clear filters
          </Button>
        </div>
      )}
      <div
        className="overflow-hidden rounded-[6px] border border-border"
        aria-busy={state.busy}
      >
        <ServiceColumns />
        <ul
          className="m-0 list-none p-0"
          role="list"
          aria-label="System services"
        >
          {rows?.map((service) => (
            <ServiceRow key={service.name} service={service} onLogs={onLogs} />
          ))}
        </ul>
        {!state.data && !state.error && (
          <p
            className="m-0 px-[16px] py-[24px] text-[12px] text-muted-foreground"
            role="status"
          >
            Reading system services…
          </p>
        )}
        {!state.data && state.error && (
          <p className="m-0 px-[16px] py-[24px] text-[12px] text-muted-foreground">
            Refresh to load this server’s services.
          </p>
        )}
        {rows?.length === 0 && (
          <p
            className="m-0 px-[16px] py-[24px] text-[12px] text-muted-foreground"
            role="status"
          >
            {filtered
              ? "No matching services. Try another name or state."
              : "No loaded services found."}
          </p>
        )}
      </div>
      <p className="mt-[14px] text-[11px] leading-[1.5] text-muted-foreground">
        Up to 500 loaded services, including inactive ones. Failed services
        first. Monitoring is read-only; no agent is installed on the server.
      </p>
    </div>
  );
}
