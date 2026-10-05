import { Disclosure } from "./Disclosure";
import { MetricReading } from "./MetricReading";
import { useWorkspaceState } from "../state/workspace";
import {
  Table,
  TableHeader,
  TableRow,
  TableHead,
  TableBody,
  TableCell,
} from "./MonitoringTable";
import { Select, SelectItem } from "./controls";
import { SelectLabel, SelectSeparator } from "./ui/select";
import {
  Area,
  AreaChart,
  CartesianGrid,
  ComposedChart,
  Line,
  ReferenceLine,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import {
  chartPoints,
  averageChartPoints,
  hasHistoryGap,
  withHistoryGaps,
  type MetricSample,
} from "../metricHistory";
import { rate, type Metric, type Window } from "../hooks/useMetricWindow";

const percent = (n: number) => `${n.toFixed(1)}%`;
const time = (n: number) =>
  new Date(n).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
// Long windows are averaged into buckets so a week stays readable.
const bucket = (minutes: number) =>
  minutes === 10080 ? 30 * 60000 : minutes === 1440 ? 5 * 60000 : 0;

type Series = {
  name: string;
  read: (p: MetricSample) => number | null;
  /** Categorical slot; slot 2 is also dashed so identity is not color alone. */
  slot: 1 | 2;
};
const stroke = (slot: 1 | 2) => `var(--series-${slot})`;

function seriesFor(metric: Metric, window: Window): Series[] {
  switch (metric) {
    case "cpu":
      return [{ name: "CPU", read: (p) => p.cpu, slot: 1 }];
    case "memory":
      return [{ name: "Memory", read: (p) => p.memory, slot: 1 }];
    case "load":
      return [{ name: "Load, 1 minute", read: (p) => p.load, slot: 1 }];
    case "network":
      return [
        { name: "Receive", read: window.net("rx"), slot: 1 },
        { name: "Send", read: window.net("tx"), slot: 2 },
      ];
  }
}

/**
 * A tile's trend: the window's shape at a glance, without axes. The large
 * chart below carries the scale, the hover readout and the table view.
 */
export function Sparkline({
  metric,
  window,
  max,
}: {
  metric: Metric;
  window: Window;
  /** A fixed top, or a floor the data may exceed (load's core count). */
  max?: number | { atLeast: number };
}) {
  const series = seriesFor(metric, window);
  const data = chartPoints(
    withHistoryGaps(
      window.samples.map((p) => ({
        at: p.at,
        resolutionMs: p.resolutionMs ?? 10000,
        ...Object.fromEntries(series.map((s, i) => [`v${i}`, s.read(p)])),
      })),
    ),
    120,
  );
  return (
    <div data-slot="sparkline" className="h-[34px] w-full" aria-hidden="true">
      {data.length > 1 && (
        <ResponsiveContainer width="100%" height="100%" minWidth={0}>
          <AreaChart
            data={data}
            margin={{ top: 3, right: 0, bottom: 1, left: 0 }}
          >
            <YAxis
              hide
              domain={[
                0,
                typeof max === "number"
                  ? max
                  : max
                    ? (peak: number) => Math.max(peak, max.atLeast) * 1.15
                    : "auto",
              ]}
            />
            <XAxis
              hide
              dataKey="at"
              type="number"
              domain={[window.start, window.end]}
            />
            {series.map((s, i) => (
              <Area
                key={s.name}
                dataKey={`v${i}`}
                type="linear"
                stroke={stroke(s.slot)}
                strokeWidth={1.5}
                strokeDasharray={s.slot === 2 ? "3 2" : undefined}
                fill={stroke(s.slot)}
                fillOpacity={series.length === 1 ? 0.1 : 0}
                connectNulls={false}
                isAnimationActive={false}
                dot={false}
                activeDot={false}
              />
            ))}
          </AreaChart>
        </ResponsiveContainer>
      )}
    </div>
  );
}

/** The selected resource's history at full size, with a crosshair readout. */
export function HistoryChart({
  metric,
  window,
  minutes,
  cores,
}: {
  metric: Metric;
  window: Window;
  minutes: number;
  cores: number;
}) {
  const { samples, start, end } = window;
  const series = seriesFor(metric, window);
  const intervalMs = bucket(minutes);
  const rows = samples.map((p) =>
    Object.assign(
      {
        at: p.at,
        resolutionMs: p.resolutionMs ?? 10000,
        incomplete: p.incomplete ?? 0,
      },
      Object.fromEntries(series.map((s, i) => [`v${i}`, s.read(p)])),
    ),
  );
  const data = intervalMs
    ? averageChartPoints(rows, start, end, intervalMs)
    : chartPoints(withHistoryGaps(rows));
  const peak = data.reduce(
    (highest, row) =>
      series.reduce(
        (value, _, i) => Math.max(value, row[`v${i}`] ?? 0),
        highest,
      ),
    0,
  );
  // Network shares one byte scale across both directions, named on the axis.
  const scale =
    metric !== "network"
      ? 1
      : peak >= 1048576
        ? 1048576
        : peak >= 1024
          ? 1024
          : 1;
  const ceiling =
    metric === "cpu" || metric === "memory"
      ? 100
      : metric === "load"
        ? // Room for the core line, so a light load never looks like a full one.
          Math.max(cores, peak) * 1.15 || 1
        : Math.max(peak, 1) * 1.15;
  const format =
    metric === "network"
      ? rate
      : metric === "load"
        ? (n: number) => n.toFixed(2)
        : percent;
  const latest = samples.at(-1);
  const dotData = intervalMs ? data : rows;
  const sampleIndices = new Map(dotData.map((p, i) => [p.at, i]));
  // A reading with no neighbour would be invisible as a line; give it a dot.
  const isolatedDot =
    (slot: 1 | 2, seriesIndex: number) =>
    (props: { cx?: number; cy?: number; payload?: { at?: number } }) => {
      const index = sampleIndices.get(props.payload?.at ?? -1) ?? -1;
      const sample = dotData[index];
      const key = `v${seriesIndex}`;
      if (
        !sample ||
        sample[key] === null ||
        props.cx === undefined ||
        props.cy === undefined
      )
        return <g />;
      const connected = (neighbor: (typeof data)[number] | undefined) =>
        neighbor && neighbor[key] !== null && !hasHistoryGap(neighbor, sample);
      if (connected(dotData[index - 1]) || connected(dotData[index + 1]))
        return <g />;
      return (
        <circle
          className="isolated-metric-point"
          cx={props.cx}
          cy={props.cy}
          r={4}
          fill={stroke(slot)}
          stroke="var(--background)"
          strokeWidth={2}
        />
      );
    };
  const multiDay =
    new Date(start).toDateString() !== new Date(end).toDateString();
  const summary = series
    .map(
      (s) =>
        `${s.name}: ${latest && s.read(latest) !== null ? format(s.read(latest)!) : "unavailable"}`,
    )
    .join(". ");
  return (
    <figure data-slot="metric-chart" className="m-0 min-w-0">
      {series.length > 1 && (
        <figcaption
          data-slot="chart-readings"
          className="mb-[6px] flex flex-wrap justify-end gap-[14px] text-[11px]"
        >
          {series.map((s) => (
            <MetricReading key={s.name} name={s.name} dashed={s.slot === 2}>
              {latest && s.read(latest) !== null
                ? format(s.read(latest)!)
                : "—"}
            </MetricReading>
          ))}
        </figcaption>
      )}
      <div
        data-slot="recharts-plot"
        className="h-[208px] w-full min-w-0"
        role="img"
        aria-label={`${series.length > 1 ? "Network" : series[0].name} history, ${samples.length} samples. ${summary}`}
      >
        <ResponsiveContainer width="100%" height="100%" minWidth={0}>
          <ComposedChart
            data={data}
            margin={{ top: 10, right: 8, bottom: 0, left: 0 }}
            accessibilityLayer={false}
          >
            <CartesianGrid vertical={false} stroke="var(--line)" />
            <XAxis
              dataKey="at"
              type="number"
              scale="time"
              domain={[start, end]}
              ticks={[start, (start + end) / 2, end]}
              tickFormatter={(n) =>
                multiDay
                  ? new Date(n).toLocaleString([], {
                      month: "short",
                      day: "numeric",
                      ...(intervalMs === 5 * 60000
                        ? ({ hour: "numeric" } as const)
                        : {}),
                    })
                  : new Date(n).toLocaleTimeString([], {
                      hour: "2-digit",
                      minute: "2-digit",
                    })
              }
              tickLine={false}
              axisLine={false}
              tick={{ fill: "var(--muted-foreground)", fontSize: 10 }}
              minTickGap={20}
              tickMargin={8}
            />
            <YAxis
              domain={[0, ceiling]}
              ticks={[0, ceiling / 2, ceiling]}
              tickFormatter={(n: number) =>
                metric === "cpu" || metric === "memory"
                  ? `${n}%`
                  : metric === "network"
                    ? `${(n / scale).toFixed(n / scale < 10 ? 1 : 0)} ${scale === 1048576 ? "MiB/s" : scale === 1024 ? "KiB/s" : "B/s"}`
                    : n.toFixed(ceiling < 10 ? 1 : 0)
              }
              tickLine={false}
              axisLine={false}
              width={metric === "network" ? 64 : 40}
              tick={{ fill: "var(--muted-foreground)", fontSize: 10 }}
            />
            {metric === "load" && cores > 0 && (
              <ReferenceLine
                y={cores}
                stroke="var(--muted-foreground)"
                strokeOpacity={0.5}
                label={{
                  value: `${cores} ${cores === 1 ? "core" : "cores"}`,
                  position: "insideTopRight",
                  fill: "var(--muted-foreground)",
                  fontSize: 10,
                }}
              />
            )}
            <Tooltip
              isAnimationActive={false}
              cursor={{ stroke: "var(--muted-foreground)", strokeOpacity: 0.6 }}
              content={({ active, payload, label }) =>
                active && payload?.length ? (
                  <div
                    data-slot="metric-tooltip"
                    className="min-w-[150px] rounded-[8px] border border-border bg-popover p-[10px] text-[12px] text-foreground shadow-[0_4px_16px_rgb(0_0_0/0.08)]"
                  >
                    <time className="mb-[6px] block text-[10px] text-muted-foreground">
                      {multiDay
                        ? new Date(Number(label)).toLocaleString()
                        : time(Number(label))}
                    </time>
                    {payload.map((p, i) => (
                      <div
                        key={String(p.dataKey)}
                        className="mt-[4px] flex items-center justify-between gap-[18px]"
                      >
                        <span className="inline-flex items-center gap-[6px] text-muted-foreground">
                          <svg width="12" height="2" aria-hidden="true">
                            <line
                              x1="0"
                              y1="1"
                              x2="12"
                              y2="1"
                              stroke={stroke(series[i]?.slot ?? 1)}
                              strokeWidth="2"
                              strokeDasharray={
                                series[i]?.slot === 2 ? "3 2" : undefined
                              }
                            />
                          </svg>
                          {p.name}
                        </span>
                        <strong className="font-[550] tabular-nums">
                          {typeof p.value === "number" ? format(p.value) : "—"}
                        </strong>
                      </div>
                    ))}
                  </div>
                ) : null
              }
            />
            {series.map((s, i) =>
              series.length === 1 ? (
                <Area
                  key={s.name}
                  dataKey={`v${i}`}
                  name={s.name}
                  type="linear"
                  stroke={stroke(s.slot)}
                  strokeWidth={2}
                  fill={stroke(s.slot)}
                  fillOpacity={0.1}
                  connectNulls={false}
                  isAnimationActive={false}
                  dot={isolatedDot(s.slot, i)}
                  activeDot={{
                    r: 4,
                    stroke: "var(--background)",
                    strokeWidth: 2,
                  }}
                />
              ) : (
                <Line
                  key={s.name}
                  dataKey={`v${i}`}
                  name={s.name}
                  type="linear"
                  stroke={stroke(s.slot)}
                  strokeWidth={2}
                  strokeDasharray={s.slot === 2 ? "5 4" : undefined}
                  connectNulls={false}
                  isAnimationActive={false}
                  dot={isolatedDot(s.slot, i)}
                  activeDot={{
                    r: 4,
                    stroke: "var(--background)",
                    strokeWidth: 2,
                  }}
                />
              ),
            )}
          </ComposedChart>
        </ResponsiveContainer>
      </div>
    </figure>
  );
}

/** The table view behind every chart: each reading in the window. */
export function Readings({ id, window }: { id: string; window: Window }) {
  const [showSamples, setShowSamples] = useWorkspaceState(
    id,
    "metrics.samples",
  );
  const { samples, net, iface } = window;
  if (!samples.length) return null;
  return (
    <Disclosure
      title="View readings"
      data-slot="history-data"
      onToggle={(event) => setShowSamples(event.currentTarget.open)}
    >
      <p className="mt-[10px] text-[12px] leading-[1.65] text-muted-foreground">
        Newest first · Network: {iface || "unavailable"} · — means unavailable
      </p>
      <div
        data-slot="history-table"
        className="mt-[10px] max-h-[260px] overflow-auto rounded-[6px] border border-border tabular-nums focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
        tabIndex={0}
        role="region"
        aria-label="Recorded metric samples"
      >
        {showSamples && (
          <Table kind="history" aria-label="Recorded metric samples">
            <caption className="sr-only">
              Recorded metric samples for the selected history window
            </caption>
            <TableHeader>
              <TableRow>
                {[
                  "Time",
                  "CPU",
                  "Memory",
                  "Load",
                  "Received / s",
                  "Sent / s",
                ].map((label) => (
                  <TableHead scope="col" key={label} numeric={label !== "Time"}>
                    {label}
                  </TableHead>
                ))}
              </TableRow>
            </TableHeader>
            <TableBody>
              {[...samples].reverse().map((sample) => (
                <TableRow key={sample.at}>
                  <TableCell>{time(sample.at)}</TableCell>
                  <TableCell>{percent(sample.cpu)}</TableCell>
                  <TableCell>{percent(sample.memory)}</TableCell>
                  <TableCell>{sample.load.toFixed(2)}</TableCell>
                  <TableCell>
                    {net("rx")(sample) === null
                      ? "—"
                      : rate(net("rx")(sample)!)}
                  </TableCell>
                  <TableCell>
                    {net("tx")(sample) === null
                      ? "—"
                      : rate(net("tx")(sample)!)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </div>
    </Disclosure>
  );
}

/**
 * Picks the interface the network tile and chart follow: physical uplinks
 * and VPNs first, then the container and loopback links under their own
 * heading.
 */
export function InterfacePicker({ window }: { window: Window }) {
  if (window.interfaces.length < 2) return null;
  const primary = window.primaryInterfaces;
  const rest = window.interfaces.filter((n) => !primary.includes(n));
  return (
    <Select
      className="h-[26px] max-w-[160px] text-[11px]"
      aria-label="Graph network interface"
      value={window.iface}
      onValueChange={window.setIface}
    >
      {primary.map((n) => (
        <SelectItem key={n} value={n}>
          {n}
        </SelectItem>
      ))}
      {rest.length > 0 && primary.length > 0 && <SelectSeparator />}
      {rest.length > 0 && <SelectLabel>Virtual and loopback</SelectLabel>}
      {rest.map((n) => (
        <SelectItem key={n} value={n}>
          {n}
        </SelectItem>
      ))}
    </Select>
  );
}
