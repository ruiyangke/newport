import { SectionEmpty } from "./Feedback";
import { MonitoringSection } from "./MonitoringSection";
import { SectionHeader, CollectionCount } from "./SectionHeader";
import { useWorkspaceState } from "../state/workspace";
import { lazy, Suspense, useRef } from "react";
import { SearchField } from "./SearchField";
import { Disclosure } from "./Disclosure";
import { Checkbox, RefreshButton, Select, SelectItem } from "./controls";
import { PageHeader } from "./PageHeader";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "./MonitoringTable";
import { useMetricHistory } from "../hooks/useMetricHistory";
import { useCollection } from "../hooks/useCollection";

const MetricGraphs = lazy(() =>
  import("./MetricGraphs").then((m) => ({ default: m.MetricGraphs })),
);

const bytes = (n: number) => {
  if (n < 1024) return `${n} B`;
  const i = Math.min(4, Math.floor(Math.log(n) / Math.log(1024)));
  return `${(n / 1024 ** i).toFixed(1)} ${["B", "KiB", "MiB", "GiB", "TiB"][i]}`;
};
const uptime = (seconds: number) =>
  `${Math.floor(seconds / 86400)}d ${Math.floor((seconds % 86400) / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;

function Gauge({
  label,
  used,
  total,
  percent,
  detail,
}: {
  label: string;
  used?: number;
  total?: number;
  percent?: number;
  detail?: string;
}) {
  const raw =
    percent ?? (total && used !== undefined ? (used / total) * 100 : NaN);
  const known = Number.isFinite(raw);
  const value = known ? Math.min(100, Math.max(0, raw)) : 0;
  return (
    <div data-slot="usage-gauge" className="min-w-0 text-center">
      <div
        data-slot="gauge-dial"
        className="relative mx-auto h-[76px] w-[80px] max-[700px]:h-[70px] max-[700px]:w-[72px]"
        role={known ? "meter" : "img"}
        aria-label={`${label} usage${known ? "" : ": unavailable"}`}
        aria-valuemin={known ? 0 : undefined}
        aria-valuemax={known ? 100 : undefined}
        aria-valuenow={known ? Math.round(value) : undefined}
      >
        <svg
          className="size-[80px] overflow-visible max-[700px]:size-[72px]"
          viewBox="0 0 100 100"
          aria-hidden="true"
        >
          <circle
            data-slot="gauge-track"
            className="origin-center rotate-[135deg] fill-none stroke-border stroke-[5px] [stroke-dasharray:75_100]"
            cx="50"
            cy="50"
            r="40"
            pathLength="100"
          />
          <circle
            data-slot="gauge-fill"
            className="origin-center rotate-[135deg] fill-none stroke-primary stroke-[5px]"
            cx="50"
            cy="50"
            r="40"
            pathLength="100"
            strokeDasharray={`${value * 0.75} 100`}
          />
        </svg>
        <strong
          className="absolute inset-x-0 top-0 flex aspect-square items-center justify-center text-[22px] leading-none font-medium tabular-nums max-[700px]:text-[19px]"
          aria-hidden="true"
        >
          {known ? Math.round(value) : "—"}
          {known && (
            <small className="absolute top-[calc(50%+14px)] left-0 m-0 w-full text-center text-[11px] leading-none font-normal text-muted-foreground">
              %
            </small>
          )}
        </strong>
      </div>
      <h3 className="mb-[3px] text-[12px] font-medium wrap-anywhere">
        {label}
      </h3>
      <p className="m-0 text-[11px] text-muted-foreground wrap-anywhere">
        {total !== undefined
          ? `${bytes(used ?? 0)} of ${bytes(total)} used`
          : detail}
      </p>
      {total !== undefined && detail && (
        <small className="mt-[5px] block text-[11px] leading-[1.5] text-muted-foreground wrap-anywhere">
          {detail}
        </small>
      )}
    </div>
  );
}
export function OverviewPanel({ id }: { id: string }) {
  const scopeRef = useRef<HTMLDivElement>(null);
  const [auto, setAuto] = useWorkspaceState(id, "overview.auto");
  const [historyMinutes, setHistoryMinutes] = useWorkspaceState(
    id,
    "overview.historyMinutes",
  );
  const [filter, setFilter] = useWorkspaceState(id, "overview.filter");
  const [sort, setSort] = useWorkspaceState(id, "overview.sort");
  const state = useCollection(id, "overview", auto, scopeRef);
  const history = useMetricHistory(
    id,
    state.data,
    state.updated?.getTime(),
    auto,
    historyMinutes,
  );
  const historyError = history.error;
  const refresh = () => {
    void state.refresh();
    void history.refresh();
  };
  const d = state.data;
  const awaitingSample =
    state.error ===
    "Waiting for the background sampler’s first reading. Refresh shortly.";
  const error = awaitingSample ? "" : state.error;
  const rows =
    d?.processes
      .filter((p) =>
        `${p.name} ${p.user} ${p.pid}`
          .toLowerCase()
          .includes(filter.trim().toLowerCase()),
      )
      .sort(
        (a, b) =>
          (sort === "memory" ? b.memory - a.memory : b.cpu - a.cpu) ||
          a.pid - b.pid,
      ) ?? [];
  return (
    <div className="overview-panel" ref={scopeRef}>
      <PageHeader
        title="System health"
        subtitleRole="status"
        subtitle={
          d
            ? `${d.hostname} · Up ${uptime(d.uptime)}`
            : error
              ? "Live metrics unavailable"
              : "Waiting for first reading…"
        }
        actions={
          <>
            <label
              data-slot="auto-refresh"
              className="flex items-center gap-[6px] text-[12px] whitespace-nowrap text-muted-foreground"
            >
              <Checkbox
                checked={auto}
                onCheckedChange={(checked) => setAuto(checked === true)}
              />
              Auto-refresh
            </label>
            <RefreshButton busy={state.busy} onClick={refresh} />
          </>
        }
        meta={
          state.updated
            ? `${state.stale || !auto ? "Last reading" : "Updated"} ${state.updated.toLocaleTimeString()}`
            : auto
              ? "Every 10 seconds"
              : "Auto-refresh paused"
        }
      />
      {error && (
        <p
          data-slot="cockpit-error"
          className="text-[12px] leading-[1.6] text-(--red) wrap-anywhere"
          role="alert"
        >
          {error}
          {d && " Showing the last available reading."}
        </p>
      )}
      {(historyError || d?.historyError) && (
        <p
          data-slot="cockpit-error"
          className="text-[12px] leading-[1.6] text-(--red) wrap-anywhere"
          role="alert"
        >
          {d?.historyError || historyError}
        </p>
      )}
      {d && (
        <MonitoringSection flush>
          <div
            data-slot="gauge-grid"
            className="grid grid-cols-3 gap-[18px] max-[700px]:gap-[8px]"
          >
            <Gauge
              label="CPU"
              percent={d.cpu}
              detail={`${d.cores} ${d.cores === 1 ? "core" : "cores"}`}
            />
            <Gauge label="Memory" used={d.memoryUsed} total={d.memoryTotal} />
            {d.disks.map((disk) => (
              <Gauge
                key={disk.mount}
                label={`Storage · ${disk.mount}`}
                used={disk.used}
                total={disk.total}
              />
            ))}
          </div>
          {!d.disks.length && (
            <p className="text-[12px] leading-[1.65] text-muted-foreground">
              Storage information unavailable.
            </p>
          )}
          <div
            data-slot="utilization-notes"
            className="mt-[16px] flex flex-wrap gap-x-[18px] gap-y-[6px] text-[11px] text-muted-foreground"
          >
            <span>
              Load average · {d.load.map((n) => n.toFixed(2)).join(" / ")} (1 /
              5 / 15 min)
            </span>
            {d.swapTotal > 0 && (
              <span>
                Swap · {bytes(d.swapUsed)} / {bytes(d.swapTotal)}
              </span>
            )}
          </div>
          <Disclosure title="System details">
            <dl
              data-slot="system-facts"
              className="mt-[14px] mb-[10px] flex flex-wrap gap-x-[18px] gap-y-[10px]"
            >
              <div className="flex min-w-0 items-baseline gap-[7px]">
                <dt className="m-0 text-[11px] text-muted-foreground">
                  Operating system
                </dt>
                <dd className="m-0 text-[11px] wrap-anywhere">{d.os}</dd>
              </div>
              <div className="flex min-w-0 items-baseline gap-[7px]">
                <dt className="m-0 text-[11px] text-muted-foreground">Host</dt>
                <dd className="m-0 text-[11px] wrap-anywhere">{d.hostname}</dd>
              </div>
              <div className="flex min-w-0 items-baseline gap-[7px]">
                <dt className="m-0 text-[11px] text-muted-foreground">
                  Uptime
                </dt>
                <dd className="m-0 text-[11px] wrap-anywhere">
                  {uptime(d.uptime)}
                </dd>
              </div>
              <div className="flex min-w-0 items-baseline gap-[7px]">
                <dt className="m-0 text-[11px] text-muted-foreground">
                  Kernel
                </dt>
                <dd className="m-0 text-[11px] wrap-anywhere">{d.kernel}</dd>
              </div>
            </dl>
            <div
              data-slot="filesystem-details"
              className="text-[11px] text-muted-foreground wrap-anywhere"
            >
              {d.disks.map((disk) => (
                <p key={disk.mount} className="my-[6px]">
                  <strong>{disk.mount}</strong> · {disk.device} ·{" "}
                  {bytes(disk.available)} available to this account
                </p>
              ))}
            </div>
          </Disclosure>
        </MonitoringSection>
      )}
      {(d || history.samples.length > 0) && (
        <Suspense
          fallback={
            <p className="text-[12px] leading-[1.65] text-muted-foreground">
              Loading charts…
            </p>
          }
        >
          <MetricGraphs
            id={id}
            history={history.samples}
            minutes={historyMinutes}
            onMinutesChange={setHistoryMinutes}
          />
        </Suspense>
      )}
      {d ? (
        <>
          <MonitoringSection>
            <SectionHeader>
              <h2>
                Processes <span>{d.processCount}</span>
              </h2>
              <Select
                aria-label="Sort processes"
                value={sort}
                onValueChange={setSort}
              >
                <SelectItem value="cpu">Highest CPU</SelectItem>
                <SelectItem value="memory">Highest memory</SelectItem>
              </Select>
            </SectionHeader>
            <SearchField
              className="my-[16px]"
              aria-label="Filter processes"
              placeholder="Find a process, user, or PID"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />

            <CollectionCount>
              {filter.trim()
                ? `Showing ${rows.length} of ${d.processes.length} collected processes`
                : `Showing ${d.processes.length} of ${d.processCount} processes · Top ${d.processes.length} by average CPU`}
            </CollectionCount>
            <Disclosure title="About process usage">
              <p className="text-[12px] leading-[1.65] text-muted-foreground">
                CPU is averaged over each process’s lifetime and can exceed 100%
                across cores. Search and sorting apply to the collected list.
              </p>
            </Disclosure>
            <div
              data-slot="table-scroll"
              className="mt-[12px] overflow-visible"
            >
              <Table aria-label="Processes">
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Process</TableHead>
                    <TableHead scope="col" numeric>
                      PID
                    </TableHead>
                    <TableHead scope="col">User</TableHead>
                    <TableHead
                      scope="col"
                      numeric
                      aria-sort={sort === "cpu" ? "descending" : undefined}
                    >
                      CPU
                    </TableHead>
                    <TableHead
                      scope="col"
                      numeric
                      aria-sort={sort === "memory" ? "descending" : undefined}
                    >
                      Memory
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((p) => (
                    <TableRow key={p.pid}>
                      <TableCell>
                        <span className="font-medium">{p.name}</span>
                      </TableCell>
                      <TableCell numeric>
                        <code>{p.pid}</code>
                      </TableCell>
                      <TableCell>{p.user}</TableCell>
                      <TableCell numeric>{p.cpu.toFixed(1)}%</TableCell>
                      <TableCell numeric>{bytes(p.memory)}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
            {!rows.length && (
              <SectionEmpty>
                {filter.trim()
                  ? "No matches in the collected processes."
                  : "No process information available."}
              </SectionEmpty>
            )}
          </MonitoringSection>
          <MonitoringSection>
            <h2>Network totals</h2>
            <p className="text-[12px] leading-[1.65] text-muted-foreground">
              Total traffic since each interface started.
            </p>
            <div
              data-slot="table-scroll"
              className="mt-[12px] overflow-visible"
            >
              <Table aria-label="Network totals">
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Interface</TableHead>
                    <TableHead scope="col" numeric>
                      Received
                    </TableHead>
                    <TableHead scope="col" numeric>
                      Sent
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {!d.network.length && (
                    <TableRow>
                      <TableCell
                        colSpan={3}
                        className="rounded-[8px] bg-sidebar leading-[1.6] text-muted-foreground"
                      >
                        No network interfaces reported.
                      </TableCell>
                    </TableRow>
                  )}
                  {d.network.map((n) => (
                    <TableRow key={n.name}>
                      <TableCell>{n.name}</TableCell>
                      <TableCell numeric>{bytes(n.received)}</TableCell>
                      <TableCell numeric>{bytes(n.sent)}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          </MonitoringSection>
        </>
      ) : !state.busy && !state.error ? (
        <SectionEmpty>Refresh to load metrics.</SectionEmpty>
      ) : null}
    </div>
  );
}
