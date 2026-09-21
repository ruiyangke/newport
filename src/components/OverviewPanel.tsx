import { lazy, Suspense, useRef, useState } from "react";
import { Search } from "lucide-react";
import { Checkbox, Input, RefreshButton, Select, SelectItem } from "./controls";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "./ui/table";
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
    <div className="usage-gauge">
      <div
        className="gauge-dial"
        role={known ? "meter" : "img"}
        aria-label={`${label} usage${known ? "" : ": unavailable"}`}
        aria-valuemin={known ? 0 : undefined}
        aria-valuemax={known ? 100 : undefined}
        aria-valuenow={known ? Math.round(value) : undefined}
      >
        <svg viewBox="0 0 100 100" aria-hidden="true">
          <circle
            className="gauge-track"
            cx="50"
            cy="50"
            r="40"
            pathLength="100"
          />
          <circle
            className="gauge-fill"
            cx="50"
            cy="50"
            r="40"
            pathLength="100"
            strokeDasharray={`${value * 0.75} 100`}
          />
        </svg>
        <strong aria-hidden="true">
          {known ? Math.round(value) : "—"}
          {known && <small>%</small>}
        </strong>
      </div>
      <h3>{label}</h3>
      <p>
        {total !== undefined
          ? `${bytes(used ?? 0)} of ${bytes(total)} used`
          : detail}
      </p>
      {total !== undefined && detail && <small>{detail}</small>}
    </div>
  );
}
export function OverviewPanel({ id }: { id: string }) {
  const scopeRef = useRef<HTMLDivElement>(null);
  const [auto, setAuto] = useState(true);
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState("cpu");
  const state = useCollection(id, "overview", auto, scopeRef);
  const history = useMetricHistory(
    id,
    state.data,
    state.updated?.getTime(),
    auto,
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
      <div className="workspace-masthead overview-masthead">
        <div>
          <h2>System health</h2>
          <p role="status">
            {d
              ? `${d.hostname} · Up ${uptime(d.uptime)}`
              : error
                ? "Live metrics unavailable"
                : "Waiting for first reading…"}
          </p>
        </div>
        <div className="overview-refresh">
          <div className="section-actions">
            <label className="auto-refresh">
              <Checkbox
                checked={auto}
                onCheckedChange={(checked) => setAuto(checked === true)}
              />
              Auto-refresh
            </label>
            <RefreshButton busy={state.busy} onClick={refresh} />
          </div>
          <span className="muted">
            {state.updated
              ? `${error ? "Last reading" : "Updated"} ${state.updated.toLocaleTimeString()}`
              : auto
                ? "Every 10 seconds"
                : "Auto-refresh paused"}
          </span>
        </div>
      </div>
      {error && (
        <p className="cockpit-error" role="alert">
          {error}
          {d && " Showing the last available reading."}
        </p>
      )}
      {(historyError || d?.historyError) && (
        <p className="cockpit-error" role="alert">
          {d?.historyError || historyError}
        </p>
      )}
      {d && (
        <section className="cockpit-section utilization-section">
          <div className="gauge-grid">
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
            <p className="muted">Storage information unavailable.</p>
          )}
          <div className="utilization-notes">
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
          <details className="system-details">
            <summary>System details</summary>
            <dl className="system-facts">
              <div>
                <dt>Operating system</dt>
                <dd>{d.os}</dd>
              </div>
              <div>
                <dt>Host</dt>
                <dd>{d.hostname}</dd>
              </div>
              <div>
                <dt>Uptime</dt>
                <dd>{uptime(d.uptime)}</dd>
              </div>
              <div>
                <dt>Kernel</dt>
                <dd>{d.kernel}</dd>
              </div>
            </dl>
            <div className="filesystem-details">
              {d.disks.map((disk) => (
                <p key={disk.mount}>
                  <strong>{disk.mount}</strong> · {disk.device} ·{" "}
                  {bytes(disk.available)} available to this account
                </p>
              ))}
            </div>
          </details>
        </section>
      )}
      {(d || history.samples.length > 0) && (
        <Suspense fallback={<p className="muted">Loading charts…</p>}>
          <MetricGraphs history={history.samples} />
        </Suspense>
      )}
      {d ? (
        <>
          <section className="cockpit-section">
            <div className="section-heading">
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
            </div>
            <div className="search cockpit-search">
              <Search size={14} />
              <Input
                aria-label="Filter processes"
                placeholder="Find a process, user, or PID"
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
              />
            </div>
            <p className="muted">
              {filter.trim()
                ? `${rows.length} matching · ${d.processes.length} collected`
                : `${d.processes.length} shown · ${d.processCount} total`}
              {" · Top 50 by average CPU"}
            </p>
            <details className="system-details">
              <summary>About process usage</summary>
              <p className="muted">
                CPU is averaged over each process’s lifetime and can exceed 100%
                across cores. Search and sorting apply to the collected list.
              </p>
            </details>
            <div className="table-scroll">
              <Table className="cockpit-table" aria-label="Processes">
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Process</TableHead>
                    <TableHead scope="col" className="numeric">
                      PID
                    </TableHead>
                    <TableHead scope="col">User</TableHead>
                    <TableHead
                      scope="col"
                      className="numeric"
                      aria-sort={sort === "cpu" ? "descending" : undefined}
                    >
                      CPU
                    </TableHead>
                    <TableHead
                      scope="col"
                      className="numeric"
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
                        <span className="row-name">{p.name}</span>
                      </TableCell>
                      <TableCell className="numeric">
                        <code>{p.pid}</code>
                      </TableCell>
                      <TableCell>{p.user}</TableCell>
                      <TableCell className="numeric">
                        {p.cpu.toFixed(1)}%
                      </TableCell>
                      <TableCell className="numeric">
                        {bytes(p.memory)}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
            {!rows.length && (
              <p className="section-empty">
                {filter.trim()
                  ? "No matches in the collected processes."
                  : "No process information available."}
              </p>
            )}
          </section>
          <section className="cockpit-section">
            <h2>Network totals</h2>
            <p className="muted">Total traffic since each interface started.</p>
            <div className="table-scroll">
              <Table className="cockpit-table" aria-label="Network totals">
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Interface</TableHead>
                    <TableHead scope="col" className="numeric">
                      Received
                    </TableHead>
                    <TableHead scope="col" className="numeric">
                      Sent
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {!d.network.length && (
                    <TableRow>
                      <TableCell colSpan={3} className="section-empty">
                        No network interfaces reported.
                      </TableCell>
                    </TableRow>
                  )}
                  {d.network.map((n) => (
                    <TableRow key={n.name}>
                      <TableCell>{n.name}</TableCell>
                      <TableCell className="numeric">
                        {bytes(n.received)}
                      </TableCell>
                      <TableCell className="numeric">{bytes(n.sent)}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          </section>
        </>
      ) : !state.busy && !state.error ? (
        <p className="section-empty">Refresh to load metrics.</p>
      ) : null}
    </div>
  );
}
