import { lazy, Suspense, useRef, type ReactNode } from "react";
import {
  ChevronRight,
  CircleCheck,
  OctagonAlert,
  TriangleAlert,
} from "lucide-react";
import { cn } from "../lib/utils";
import { SectionEmpty } from "./Feedback";
import { Checkbox, RefreshButton } from "./controls";
import { PageHeader } from "./PageHeader";
import { ProcessTable } from "./ProcessTable";
import { ToggleGroup, ToggleGroupItem } from "./ui/toggle-group";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "./MonitoringTable";
import { useMetricHistory } from "../hooks/useMetricHistory";
import {
  useMetricWindow,
  rate,
  type Metric,
  type Window,
} from "../hooks/useMetricWindow";
import { useCollection } from "../hooks/useCollection";
import { useWorkspaceState } from "../state/workspace";
import {
  THRESHOLDS,
  assess,
  bytes,
  disksByFullness,
  level as levelOf,
  loadShare,
  ratio,
  type Level,
} from "../overviewHealth";
import type { Overview } from "../domain/cockpit";
import { splitInterfaces } from "../networkInterfaces";

// Recharts loads with the first chart, not with the app.
const Sparkline = lazy(() =>
  import("./MetricGraphs").then((m) => ({ default: m.Sparkline })),
);
const HistoryChart = lazy(() =>
  import("./MetricGraphs").then((m) => ({ default: m.HistoryChart })),
);
const Readings = lazy(() =>
  import("./MetricGraphs").then((m) => ({ default: m.Readings })),
);
const InterfacePicker = lazy(() =>
  import("./MetricGraphs").then((m) => ({ default: m.InterfacePicker })),
);

const uptime = (seconds: number) => {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return days
    ? `${days}d ${hours}h`
    : hours
      ? `${hours}h ${minutes}m`
      : `${minutes}m`;
};
const RANGES = [
  { minutes: 5, short: "5m", label: "Last 5 minutes" },
  { minutes: 15, short: "15m", label: "Last 15 minutes" },
  { minutes: 60, short: "1h", label: "Last hour" },
  { minutes: 1440, short: "24h", label: "Last 24 hours" },
  { minutes: 10080, short: "7d", label: "Last 7 days" },
];
const METRIC_NAMES: Record<Metric, string> = {
  cpu: "CPU",
  memory: "Memory",
  load: "Load",
  network: "Network",
};

/** A large figure: proportional digits, the unit set smaller beside it. */
function Figure({ value, unit }: { value: number | string; unit: string }) {
  return (
    <>
      {value}
      <span className="ml-[1px] text-[14px] font-medium text-muted-foreground">
        {unit}
      </span>
    </>
  );
}
function splitRate(bytesPerSecond: number) {
  const [value, unit] = rate(bytesPerSecond).split(" ");
  return { value, unit: ` ${unit}` };
}
/** A series key matching the chart's stroke, so identity is not color alone. */
function LineKey({ slot }: { slot: 1 | 2 }) {
  return (
    <svg width="12" height="2" aria-hidden="true" className="shrink-0">
      <line
        x1="0"
        y1="1"
        x2="12"
        y2="1"
        stroke={`var(--series-${slot})`}
        strokeWidth="2"
        strokeDasharray={slot === 2 ? "3 2" : undefined}
      />
    </svg>
  );
}

/** Status always travels with an icon and words, never color alone. */
const tone: Record<Level, string> = {
  ok: "text-(--success)",
  warning: "text-(--warning)",
  critical: "text-(--danger)",
};
const fill: Record<Level, string> = {
  ok: "bg-(--series-1)",
  warning: "bg-(--warning)",
  critical: "bg-(--danger)",
};
function StatusIcon({ level, size = 14 }: { level: Level; size?: number }) {
  const Icon =
    level === "critical"
      ? OctagonAlert
      : level === "warning"
        ? TriangleAlert
        : CircleCheck;
  return (
    <Icon
      size={size}
      aria-hidden="true"
      className={cn("shrink-0", tone[level])}
    />
  );
}

/**
 * A ratio against its limit. The fill carries severity; the track is the
 * same hue, lighter, so the whole bar reads as one state.
 */
function Meter({
  label,
  value,
  level,
  className,
}: {
  label: string;
  value: number;
  level: Level;
  className?: string;
}) {
  const known = Number.isFinite(value);
  const shown = known ? Math.min(100, Math.max(0, value)) : 0;
  return (
    <div
      role={known ? "meter" : "img"}
      aria-label={`${label}${known ? "" : ": unavailable"}`}
      aria-valuemin={known ? 0 : undefined}
      aria-valuemax={known ? 100 : undefined}
      aria-valuenow={known ? Math.round(shown) : undefined}
      className={cn("relative h-[4px] overflow-hidden rounded-full", className)}
    >
      <span className={cn("absolute inset-0 opacity-15", fill[level])} />
      <span
        className={cn("absolute inset-y-0 left-0 rounded-full", fill[level])}
        style={{ width: `${shown}%` }}
      />
    </div>
  );
}

/**
 * One resource at a glance. Every tile has the same four rows -- name, value,
 * one line of context, trend -- so the four read as a set; a strained one
 * says so in words beside its name. Choosing a tile puts its history below.
 */
function Tile({
  metric,
  selected,
  onSelect,
  level,
  value,
  context,
  trend,
  hint,
}: {
  metric: Metric;
  selected: boolean;
  onSelect: () => void;
  level: Level;
  value: ReactNode;
  context: ReactNode;
  trend: ReactNode;
  /** Secondary facts, on hover. */
  hint?: string;
}) {
  const name = METRIC_NAMES[metric];
  return (
    <section
      data-slot="resource-tile"
      aria-label={name}
      title={hint}
      className={cn(
        "relative flex min-w-0 flex-col rounded-[10px] border bg-background px-[13px] pt-[11px] pb-[8px] transition-colors motion-reduce:transition-none",
        selected
          ? "border-(--series-1)/60 shadow-[0_0_0_3px_color-mix(in_srgb,var(--series-1)_12%,transparent)]"
          : "border-border hover:border-foreground/20",
      )}
    >
      {/* The whole tile picks the chart; its parts stay readable to AT. */}
      <button
        type="button"
        aria-pressed={selected}
        aria-label={`Show ${name.toLowerCase()} history`}
        onClick={onSelect}
        className="absolute inset-0 z-[1] rounded-[10px] outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
      />
      <h3 className="flex items-center justify-between gap-[6px] text-[12px] font-medium text-muted-foreground">
        {name}
        {level !== "ok" && (
          <span
            className={cn(
              "inline-flex items-center gap-[4px] text-[11px]",
              tone[level],
            )}
          >
            <StatusIcon level={level} size={12} />
            {level === "critical" ? "Critical" : "High"}
          </span>
        )}
      </h3>
      <div className="mt-[3px] text-[26px] leading-[1.15] font-semibold tracking-[-0.02em] whitespace-nowrap">
        {value}
      </div>
      <div className="mt-[1px] flex min-h-[18px] min-w-0 items-center gap-[5px] truncate text-[11.5px] text-muted-foreground">
        {context}
      </div>
      <div className="mt-auto pt-[10px]">{trend}</div>
    </section>
  );
}

function Section({
  title,
  aside,
  children,
  className,
  label,
}: {
  title: ReactNode;
  aside?: ReactNode;
  children: ReactNode;
  className?: string;
  label?: string;
}) {
  return (
    <section aria-label={label} className={cn("min-w-0", className)}>
      <header className="mb-[10px] flex min-h-[28px] flex-wrap items-center justify-between gap-[10px]">
        <h2>{title}</h2>
        {aside}
      </header>
      {children}
    </section>
  );
}

function Storage({
  d,
  health,
}: {
  d: Overview;
  health: ReturnType<typeof assess>;
}) {
  const disks = disksByFullness(d);
  return (
    <Section title="Storage" label="Storage">
      {disks.length ? (
        <ul className="m-0 grid list-none grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-x-[28px] gap-y-[12px] p-0">
          {disks.map((disk) => {
            const used = ratio(disk.used, disk.total);
            const level = health.of(`disk:${disk.mount}`);
            return (
              <li
                key={disk.mount}
                title={`${disk.device} · ${bytes(disk.available)} available to this account`}
              >
                <div className="flex items-baseline justify-between gap-[10px] text-[12px]">
                  <span className="flex min-w-0 items-center gap-[6px]">
                    <code className="truncate text-[12px] font-medium text-foreground">
                      {disk.mount}
                    </code>
                    {level !== "ok" && <StatusIcon level={level} size={12} />}
                  </span>
                  <span className="shrink-0 text-[11.5px] text-muted-foreground">
                    <span
                      className={cn(
                        "font-medium",
                        level === "ok" ? "text-foreground" : tone[level],
                      )}
                    >
                      {Math.round(used)}%
                    </span>{" "}
                    · {bytes(disk.available)} free of {bytes(disk.total)}
                  </span>
                </div>
                <Meter
                  label={`Storage ${disk.mount} usage`}
                  value={used}
                  level={levelOf(used, THRESHOLDS.disk)}
                  className="mt-[6px] h-[6px]"
                />
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="text-[12px] text-muted-foreground">
          Storage information unavailable.
        </p>
      )}
    </Section>
  );
}

function NetworkTotals({ id, d }: { id: string; d: Overview }) {
  const [open, setOpen] = useWorkspaceState(id, "overview.virtualOpen");
  const { primary, folded } = splitInterfaces(
    d.network,
    (n) => n.received + n.sent,
  );
  const sum = (key: "received" | "sent") =>
    folded.reduce((total, n) => total + n[key], 0);
  const row = (n: Overview["network"][number]) => (
    <TableRow key={n.name}>
      <TableCell>{n.name}</TableCell>
      <TableCell numeric>{bytes(n.received)}</TableCell>
      <TableCell numeric>{bytes(n.sent)}</TableCell>
    </TableRow>
  );
  return (
    <Section
      title="Network totals"
      label="Network totals"
      aside={
        <span className="text-[11px] text-muted-foreground">
          Since each interface started
        </span>
      }
    >
      <Table aria-label="Network totals" containerClassName="max-h-none">
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
              <TableCell colSpan={3} className="text-muted-foreground">
                No network interfaces reported.
              </TableCell>
            </TableRow>
          )}
          {primary.map((n) => row(n))}
          {folded.length > 0 && (
            // Container hosts report dozens of veth and bridge links; they
            // fold into one line until asked for.
            <TableRow>
              <TableCell>
                <button
                  type="button"
                  aria-expanded={open}
                  onClick={() => setOpen(!open)}
                  className="-ml-[2px] inline-flex items-center gap-[5px] rounded-[4px] px-[2px] text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
                >
                  <ChevronRight
                    size={12}
                    aria-hidden="true"
                    className={cn(
                      "transition-transform motion-reduce:transition-none",
                      open && "rotate-90",
                    )}
                  />
                  {folded.length} virtual and loopback
                </button>
              </TableCell>
              <TableCell numeric className="text-muted-foreground">
                {bytes(sum("received"))}
              </TableCell>
              <TableCell numeric className="text-muted-foreground">
                {bytes(sum("sent"))}
              </TableCell>
            </TableRow>
          )}
        </TableBody>
      </Table>
      {open && (
        // The links flow into columns across the row, so dozens of veth
        // pairs read as one compact block.
        <ul
          aria-label="Virtual and loopback interfaces"
          className="m-0 mt-[8px] grid list-none grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-x-[24px] p-0"
        >
          {folded.map((n) => (
            <li
              key={n.name}
              className="flex min-w-0 items-baseline gap-[10px] border-b border-border/60 py-[6px] text-[12px]"
            >
              <span
                className="min-w-0 flex-1 truncate font-mono text-[11.5px]"
                title={n.name}
              >
                {n.name}
              </span>
              <span className="shrink-0 text-muted-foreground tabular-nums">
                <span className="sr-only">Received </span>↓ {bytes(n.received)}
              </span>
              <span className="shrink-0 text-muted-foreground tabular-nums">
                <span className="sr-only">Sent </span>↑ {bytes(n.sent)}
              </span>
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}

/**
 * A core's last minute as a small filled area, scaled 0–100% so cells
 * compare at a glance. Drawn as one SVG path: a grid of 64 charting
 * components would cost more than the data is worth.
 */
function CoreGraph({ history, usage }: { history: number[]; usage: number }) {
  const known = (history.length ? history : [usage]).map((v) =>
    Math.min(100, Math.max(0, v)),
  );
  // One reading (before the agent's history fills) spans the cell as a level.
  const values = known.length > 1 ? known : [known[0], known[0]];
  // Newest at the right edge; a short history starts part-way across.
  const step = known.length > 1 ? 60 / 59 : 60;
  const start = 60 - (values.length - 1) * step;
  const line = `M${values
    .map((v, i) => `${(start + i * step).toFixed(2)},${(100 - v).toFixed(2)}`)
    .join("L")}`;
  return (
    <svg
      viewBox="0 0 60 100"
      preserveAspectRatio="none"
      aria-hidden="true"
      className="absolute inset-0 size-full"
    >
      <line
        x1="0"
        x2="60"
        y1="50"
        y2="50"
        stroke="var(--line)"
        vectorEffect="non-scaling-stroke"
      />
      <path
        d={`${line}L60,100L${start.toFixed(2)},100Z`}
        fill="var(--series-1)"
        fillOpacity={0.32}
      />
      <path
        d={line}
        fill="none"
        stroke="var(--series-1)"
        strokeWidth={1}
        vectorEffect="non-scaling-stroke"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/**
 * Every core at a glance, as Task Manager's logical-processor view shows
 * them: one small graph of the last minute per core, in the kernel's order.
 * A busy core is not a problem by itself, so graphs carry the series hue,
 * not a status color.
 */
function Cores({ cpus }: { cpus: NonNullable<Overview["cpus"]> }) {
  const cores = [...cpus].sort((a, b) => a.id - b.id);
  const busiest = Math.max(...cores.map((c) => c.usage));
  return (
    <section
      aria-label="CPU cores"
      data-slot="cpu-cores"
      className="mt-[10px] rounded-[10px] border border-border px-[14px] pt-[9px] pb-[12px]"
    >
      <h3 className="mb-[8px] flex items-baseline justify-between gap-[10px] text-[12px] font-medium text-muted-foreground">
        Cores
        <span className="text-[11px] font-normal">
          {cores.length} online · busiest {Math.round(busiest)}% · last minute
        </span>
      </h3>
      <ul
        role="list"
        className={cn(
          "m-0 grid list-none gap-[6px] p-0",
          cores.length > 32
            ? "grid-cols-[repeat(auto-fill,minmax(76px,1fr))]"
            : "grid-cols-[repeat(auto-fill,minmax(112px,1fr))]",
        )}
      >
        {cores.map((core) => {
          const history = core.history ?? [];
          const peak = Math.max(core.usage, ...history);
          return (
            <li
              key={core.id}
              role="img"
              aria-label={`Core ${core.id}: ${Math.round(core.usage)}% now, peak ${Math.round(peak)}% in the last minute`}
              className="relative aspect-[3/2] overflow-hidden rounded-[5px] border border-(--series-1)/25 bg-(--series-1)/[0.03]"
            >
              <CoreGraph history={history} usage={core.usage} />
              <span className="absolute top-[3px] left-[4px] rounded-[3px] bg-background/75 px-[2px] font-mono text-[10px] leading-[1.3] text-muted-foreground">
                {core.id}
              </span>
              <span className="absolute top-[3px] right-[4px] rounded-[3px] bg-background/75 px-[2px] text-[10px] leading-[1.3] font-medium tabular-nums">
                {Math.round(core.usage)}%
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

function Resources({
  id,
  d,
  window,
  health,
  minutes,
  setMinutes,
}: {
  id: string;
  /** The live reading; without one, saved history still shows. */
  d?: Overview;
  window: Window;
  health?: ReturnType<typeof assess>;
  minutes: number;
  setMinutes: (minutes: number) => void;
}) {
  const [metric, setMetric] = useWorkspaceState(id, "overview.metric");
  const tilesFor = (d: Overview, health: ReturnType<typeof assess>) => {
    const memory = ratio(d.memoryUsed, d.memoryTotal);
    const load = loadShare(d);
    const rx = window.latest ? window.net("rx")(window.latest) : null;
    const tx = window.latest ? window.net("tx")(window.latest) : null;
    const worst = (...levels: Level[]): Level =>
      levels.includes("critical")
        ? "critical"
        : levels.includes("warning")
          ? "warning"
          : "ok";
    const trend = (which: Metric, max?: number | { atLeast: number }) => (
      <Suspense fallback={<div className="h-[34px]" />}>
        <Sparkline metric={which} window={window} max={max} />
      </Suspense>
    );
    return (
      <div
        data-slot="resource-tiles"
        className="grid grid-cols-4 gap-[10px] @max-[720px]/overview:grid-cols-2"
      >
        <Tile
          metric="cpu"
          selected={metric === "cpu"}
          onSelect={() => setMetric("cpu")}
          level={health.of("cpu")}
          value={<Figure value={Math.round(d.cpu)} unit="%" />}
          context={`${d.cores} ${d.cores === 1 ? "core" : "cores"}`}
          trend={trend("cpu", 100)}
        />
        <Tile
          metric="memory"
          selected={metric === "memory"}
          onSelect={() => setMetric("memory")}
          level={worst(health.of("memory"), health.of("swap"))}
          value={<Figure value={Math.round(memory)} unit="%" />}
          context={`${bytes(d.memoryUsed)} of ${bytes(d.memoryTotal)}`}
          hint={
            d.swapTotal > 0
              ? `Swap ${bytes(d.swapUsed)} of ${bytes(d.swapTotal)}`
              : undefined
          }
          trend={trend("memory", 100)}
        />
        <Tile
          metric="load"
          selected={metric === "load"}
          onSelect={() => setMetric("load")}
          level={health.of("load")}
          value={
            Number.isFinite(load) ? (
              <Figure value={Math.round(load)} unit="%" />
            ) : (
              "—"
            )
          }
          context={`of ${d.cores} ${d.cores === 1 ? "core" : "cores"} · load ${(d.load[0] ?? 0).toFixed(2)}`}
          hint={`Load average ${d.load.map((n) => n.toFixed(2)).join(" / ")} over 1 / 5 / 15 minutes`}
          trend={trend("load", { atLeast: d.cores })}
        />
        <Tile
          metric="network"
          selected={metric === "network"}
          onSelect={() => setMetric("network")}
          level="ok"
          value={rx === null ? "—" : <Figure {...splitRate(rx)} />}
          // The keys name the trend's two lines: received solid, sent dashed.
          context={
            <>
              <LineKey slot={1} /> in
              <LineKey slot={2} /> out {tx === null ? "—" : rate(tx)}
            </>
          }
          hint={window.iface ? `Interface ${window.iface}` : undefined}
          trend={trend("network")}
        />
      </div>
    );
  };
  // Without a live reading there are no tiles to choose from; a plain
  // switch picks the chart instead.
  const tiles =
    d && health ? (
      tilesFor(d, health)
    ) : (
      <ToggleGroup
        type="single"
        variant="outline"
        spacing={0}
        size="sm"
        aria-label="Chart"
        value={metric}
        onValueChange={(value) => value && setMetric(value as Metric)}
      >
        {(Object.keys(METRIC_NAMES) as Metric[]).map((key) => (
          <ToggleGroupItem
            key={key}
            value={key}
            className="px-[10px] text-[12px]"
          >
            {METRIC_NAMES[key]}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
    );
  return (
    <>
      <Section
        label="Resources"
        title={d ? "Resources" : "Saved history"}
        aside={
          <ToggleGroup
            type="single"
            variant="outline"
            spacing={0}
            size="sm"
            aria-label="History time window"
            value={String(minutes)}
            onValueChange={(value) => value && setMinutes(Number(value))}
          >
            {RANGES.map((range) => (
              <ToggleGroupItem
                key={range.minutes}
                value={String(range.minutes)}
                aria-label={range.label}
                title={range.label}
                className="px-[9px] text-[12px]"
              >
                {range.short}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
        }
      >
        {tiles}
        {d?.cpus?.length ? <Cores cpus={d.cpus} /> : null}
      </Section>
      <section
        data-slot="metric-history"
        aria-label={`${METRIC_NAMES[metric]} history`}
        className="mt-[12px] rounded-[10px] border border-border px-[14px] pt-[12px] pb-[10px]"
      >
        <header className="mb-[6px] flex flex-wrap items-center justify-between gap-[8px]">
          <h3 className="text-[12.5px] font-semibold">
            {METRIC_NAMES[metric]}{" "}
            <span className="font-normal text-muted-foreground">
              · {RANGES.find((r) => r.minutes === minutes)?.label.toLowerCase()}
            </span>
          </h3>
          <span className="flex items-center gap-[10px] text-[11px] text-muted-foreground">
            {metric === "network" && (
              <Suspense fallback={null}>
                <InterfacePicker window={window} />
              </Suspense>
            )}
            {window.latest
              ? `Last reading ${new Date(window.latest.at).toLocaleTimeString()}`
              : "No readings yet"}
          </span>
        </header>
        {window.samples.length ? (
          <Suspense
            fallback={
              <p className="h-[208px] text-[12px] text-muted-foreground">
                Loading chart…
              </p>
            }
          >
            <HistoryChart
              metric={metric}
              window={window}
              minutes={minutes}
              cores={d?.cores ?? 0}
            />
          </Suspense>
        ) : (
          <p className="flex h-[120px] items-center justify-center text-[12px] text-muted-foreground">
            No history yet. Readings will appear here as they arrive.
          </p>
        )}
        <Suspense fallback={null}>
          <Readings id={id} window={window} />
        </Suspense>
      </section>
    </>
  );
}

export function OverviewPanel({ id }: { id: string }) {
  const scopeRef = useRef<HTMLDivElement>(null);
  const [auto, setAuto] = useWorkspaceState(id, "overview.auto");
  const [minutes, setMinutes] = useWorkspaceState(
    id,
    "overview.historyMinutes",
  );
  const state = useCollection(id, "overview", auto, scopeRef);
  const history = useMetricHistory(
    id,
    state.data,
    state.updated?.getTime(),
    auto,
    minutes,
  );
  const window = useMetricWindow(id, history.samples, minutes);
  const refresh = () => {
    void state.refresh();
    void history.refresh();
  };
  const d = state.data;
  const awaitingSample =
    state.error ===
    "Waiting for the background sampler’s first reading. Refresh shortly.";
  const error = awaitingSample ? "" : state.error;
  const historyError = d?.historyError || history.error;
  const health = d ? assess(d) : null;
  return (
    <div className="overview-panel @container/overview" ref={scopeRef}>
      <PageHeader
        title="System health"
        subtitleRole="status"
        subtitle={
          d ? (
            <span title={`Kernel ${d.kernel}`}>
              {[d.hostname, d.os, `up ${uptime(d.uptime)}`]
                .filter(Boolean)
                .join(" · ")}
            </span>
          ) : error ? (
            "Live metrics unavailable"
          ) : (
            "Waiting for first reading…"
          )
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
          className="mb-[12px] text-[12px] leading-[1.6] text-(--danger) wrap-anywhere"
          role="alert"
        >
          {error}
          {d && " Showing the last available reading."}
        </p>
      )}
      {historyError && (
        <p
          data-slot="cockpit-error"
          className="mb-[12px] text-[12px] leading-[1.6] text-(--danger) wrap-anywhere"
          role="alert"
        >
          {historyError}
        </p>
      )}
      {(d || history.samples.length > 0) && (
        <Resources
          id={id}
          d={d ?? undefined}
          window={window}
          health={health ?? undefined}
          minutes={minutes}
          setMinutes={setMinutes}
        />
      )}
      {d && health ? (
        <>
          <div className="mt-[26px] flex flex-col gap-[26px]">
            <Storage d={d} health={health} />
            <NetworkTotals id={id} d={d} />
          </div>
          <div className="mt-[26px]">
            <ProcessTable id={id} d={d} />
          </div>
          <p
            data-slot="cockpit-footnote"
            className="mt-[28px] text-[11px] leading-[1.6] text-muted-foreground"
          >
            Collected by the Newport agent on the server. Monitoring is
            read-only.
          </p>
        </>
      ) : !state.busy && !state.error ? (
        <SectionEmpty>Refresh to load metrics.</SectionEmpty>
      ) : null}
    </div>
  );
}
