import { useState } from "react";
import { ArrowLeft, ServerOff } from "lucide-react";
import { PageHeader } from "../PageHeader";
import { SearchField } from "../SearchField";
import { Button, RefreshButton } from "../controls";
import { DetailSkeleton } from "../master/Detail";
import { ServiceList, ServiceListSkeleton, JOURNAL } from "./ServiceList";
import { ServiceDetail, TimerDetail } from "./ServiceDetail";
import { LogToolbar, LogView } from "./ServiceLogs";
import { useServicesSnapshot } from "../../hooks/useServices";
import { useWorkspaceState } from "../../state/workspace";
import { cn } from "../../lib/utils";
import {
  groupServices,
  isProblem,
  unitKey,
  type ServiceUnit,
  type TimerUnit,
} from "../../domain/services";

function matches(needle: string, unit: ServiceUnit | TimerUnit) {
  return `${unit.name} ${unit.description} ${"active" in unit ? unit.active : ""} ${"sub" in unit ? unit.sub : ""}`
    .toLowerCase()
    .includes(needle);
}

/**
 * Services, end to end: what needs attention and what is the person's own,
 * on the left; on the right the chosen unit's state, why it failed, its
 * controls, facts, and its logs and unit file filling the rest. The page
 * fits the window and each side scrolls on its own, so the detail stays in
 * view however far down the list the choice was.
 */
export function ServicesPage({ id }: { id: string }) {
  const snapshot = useServicesSnapshot(id);
  const [query, setQuery] = useWorkspaceState(id, "services.query");
  const [selected, setSelected] = useWorkspaceState(id, "services.selected");
  const [systemOpen, setSystemOpen] = useWorkspaceState(
    id,
    "services.systemOpen",
  );
  // Only a refresh someone asked for spins; the 5 s poll stays quiet.
  const [refreshing, setRefreshing] = useState(false);
  const refresh = () => {
    setRefreshing(true);
    void snapshot.refetch().finally(() => setRefreshing(false));
  };
  const data = snapshot.data;
  const now = snapshot.dataUpdatedAt || Date.now();
  const needle = query.trim().toLowerCase();
  const services = (data?.services ?? []).filter(
    (u) => !needle || matches(needle, u),
  );
  const timers = (data?.timers ?? [])
    .filter((t) => !needle || matches(needle, t))
    .sort((a, b) => (a.next ?? Infinity) - (b.next ?? Infinity));
  const groups = groupServices(services);
  const all = data?.services ?? [];
  // The first problem, else the first of the person's own services.
  const fallback =
    groups.problems[0] ?? groups.owned[0] ?? groups.system[0] ?? null;
  const key = selected ?? (fallback ? unitKey(fallback) : JOURNAL);
  const unit = all.find((u) => unitKey(u) === key);
  const timer = data?.timers.find((t) => unitKey(t) === key);
  const failed = all.filter((u) => u.active === "failed").length;
  const restarting = all.filter(
    (u) => isProblem(u) && u.active !== "failed",
  ).length;
  const running = all.filter(
    (u) => u.active === "active" && u.sub === "running",
  ).length;
  const summary = data
    ? [
        `${running} running`,
        failed && `${failed} failed`,
        restarting && `${restarting} restarting`,
        `${data.timers.length} timers`,
      ]
        .filter(Boolean)
        .join(" · ")
    : snapshot.error
      ? "Services unavailable"
      : "Reading services…";
  const choose = (next: string) => setSelected(next);
  return (
    <div
      data-slot="services-page"
      className="@container/services flex min-h-0 flex-1 flex-col px-[22px] pt-[16px] max-[700px]:px-[16px] max-[700px]:pt-[14px]"
    >
      <PageHeader
        className="mb-[14px]"
        title="Services"
        subtitle={summary}
        subtitleRole="status"
        actions={<RefreshButton busy={refreshing} onClick={refresh} />}
        // Reserved from the start so the page doesn't shift on first data.
        meta={
          snapshot.dataUpdatedAt
            ? `Updated ${new Date(snapshot.dataUpdatedAt).toLocaleTimeString()}`
            : "\u00a0"
        }
      />
      {snapshot.error && data && (
        <p
          role="alert"
          className="mb-[12px] text-[12px] leading-[1.6] text-(--danger) wrap-anywhere"
        >
          {String(snapshot.error)} Showing the last reading.
        </p>
      )}
      {data && !data.user.available && data.user.reason && (
        <p className="mb-[12px] text-[11.5px] text-muted-foreground">
          User services aren’t shown: {data.user.reason}
        </p>
      )}
      {data?.privilege === "password" && (
        <p className="mb-[12px] text-[11.5px] text-muted-foreground">
          System services need your sudo password to change; those actions open
          in the Terminal. Your user services change directly.
        </p>
      )}
      {snapshot.error && !data ? (
        <div
          role="alert"
          className="mb-[22px] flex flex-1 animate-in flex-col items-center justify-center gap-[10px] text-center duration-200 fade-in motion-reduce:animate-none"
        >
          <ServerOff
            size={30}
            strokeWidth={1.4}
            aria-hidden="true"
            className="text-muted-foreground"
          />
          <h3 className="text-[14px]">Services aren’t available here</h3>
          <p className="max-w-[440px] text-[12px] leading-[1.6] text-muted-foreground wrap-anywhere">
            {String(snapshot.error)}
          </p>
          <Button loading={refreshing} onClick={refresh}>
            Try again
          </Button>
        </div>
      ) : (
        <div className="grid min-h-0 flex-1 grid-cols-[minmax(210px,280px)_minmax(0,1fr)] grid-rows-[minmax(0,1fr)] gap-[20px] @max-[600px]/services:grid-cols-1">
          <aside
            aria-label="Service list"
            className={cn(
              "flex min-h-0 min-w-0 flex-col",
              selected && "@max-[600px]/services:hidden",
            )}
          >
            <SearchField
              className="mb-[8px] shrink-0"
              aria-label="Find a service"
              placeholder="Find a service or timer"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <div
              data-slot="services-list-scroll"
              className="-mx-[4px] min-h-0 flex-1 overflow-auto overscroll-contain px-[4px] pb-[16px]"
            >
              {data ? (
                <ServiceList
                  groups={groups}
                  timers={timers}
                  selected={key}
                  onSelect={choose}
                  systemOpen={systemOpen}
                  onToggleSystem={() => setSystemOpen(!systemOpen)}
                  searching={!!needle}
                  now={now}
                />
              ) : (
                <ServiceListSkeleton />
              )}
            </div>
          </aside>
          <section
            aria-label="Service detail"
            data-slot="services-detail-scroll"
            className={cn(
              "flex min-h-0 min-w-0 flex-col overflow-auto overscroll-contain pb-[16px]",
              !selected && "@max-[600px]/services:hidden",
            )}
          >
            <Button
              variant="ghost"
              className="mb-[10px] hidden h-[26px] self-start px-[6px] text-[12px] @max-[600px]/services:inline-flex"
              onClick={() => setSelected(null)}
            >
              <ArrowLeft size={13} /> All services
            </Button>
            {!data ? (
              <DetailSkeleton />
            ) : unit ? (
              <ServiceDetail
                key={key}
                id={id}
                unit={unit}
                privilege={data.privilege}
                timers={data.timers}
                onSelect={choose}
                now={now}
              />
            ) : timer ? (
              <TimerDetail
                key={key}
                id={id}
                timer={timer}
                service={all.find(
                  (u) => u.name === timer.unit && u.scope === timer.scope,
                )}
                onSelect={choose}
                now={now}
              />
            ) : key === JOURNAL ? (
              <article
                key={key}
                aria-label="System journal"
                className="flex flex-1 animate-in flex-col gap-[12px] duration-200 fade-in motion-reduce:animate-none"
              >
                <header className="flex flex-wrap items-end justify-between gap-[10px]">
                  <div>
                    <h2 className="text-[17px]">System journal</h2>
                    <p className="mt-[2px] text-[12px] text-muted-foreground">
                      Everything the server logs, newest last.
                    </p>
                  </div>
                  <LogToolbar id={id} />
                </header>
                <div className="flex min-h-[300px] flex-1 flex-col">
                  <LogView
                    id={id}
                    target={{ unit: null, scope: "system" }}
                    label="Journal"
                  />
                </div>
              </article>
            ) : (
              <p className="text-[12px] text-muted-foreground">
                {selected
                  ? "That service is no longer loaded."
                  : "Choose a service."}
              </p>
            )}
          </section>
        </div>
      )}
    </div>
  );
}
