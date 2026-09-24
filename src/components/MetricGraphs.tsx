import {
  Table,
  TableHeader,
  TableRow,
  TableHead,
  TableBody,
  TableCell,
} from "./ui/table";
import { Select, SelectItem } from "./controls";
import {
  Area,
  CartesianGrid,
  ComposedChart,
  Line,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { useState } from "react";
import {
  chartPoints,
  averageChartPoints,
  historyStart,
  hasHistoryGap,
  withHistoryGaps,
  type MetricSample,
} from "../metricHistory";
const percent = (n: number) => `${n.toFixed(1)}%`;
const rate = (n: number) =>
  n >= 1048576
    ? `${(n / 1048576).toFixed(1)} MiB/s`
    : n >= 1024
      ? `${(n / 1024).toFixed(1)} KiB/s`
      : `${n.toFixed(0)} B/s`;
const time = (n: number, includeDate = false) =>
  new Date(n).toLocaleString([], {
    ...(includeDate ? ({ month: "short", day: "numeric" } as const) : {}),
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
type Series = {
  name: string;
  read: (p: MetricSample) => number | null;
  dashed?: boolean;
};
function Chart({
  title,
  samples,
  series,
  max,
  format,
  start,
  end,
  intervalMs,
}: {
  title: string;
  samples: MetricSample[];
  series: Series[];
  max?: number;
  format: (n: number) => string;
  start: number;
  end: number;
  intervalMs: number;
}) {
  const selected = samples.at(-1);
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
  const ceiling =
    max ??
    data.reduce(
      (highest, row) =>
        series.reduce(
          (value, _, i) => Math.max(value, row[`v${i}`] ?? 0),
          highest,
        ),
      1,
    ) * 1.15;
  const dotData = intervalMs ? data : rows;
  const sampleIndices = new Map(dotData.map((p, i) => [p.at, i]));
  const isolatedDot =
    (series: Series, seriesIndex: number) =>
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
          r={3}
          fill={series.dashed ? "var(--green)" : "var(--chart-1)"}
        />
      );
    };
  return (
    <figure className="metric-chart">
      <figcaption>
        <h3>{title}</h3>
      </figcaption>
      <div className="chart-readings">
        {series.map((s) => (
          <span key={s.name} className={s.dashed ? "chart-secondary" : ""}>
            <i className={s.dashed ? "dashed" : ""} />
            {s.name}{" "}
            <strong>
              {selected && s.read(selected) !== null
                ? format(s.read(selected)!)
                : "—"}
            </strong>
          </span>
        ))}
      </div>
      <div
        className="recharts-plot"
        role="img"
        aria-label={`${title} history, ${samples.length} samples. ${series.map((s) => `${s.name}: ${selected && s.read(selected) !== null ? format(s.read(selected)!) : "unavailable"}`).join(". ")}`}
      >
        <ResponsiveContainer width="100%" height="100%" minWidth={0}>
          <ComposedChart
            data={data}
            margin={{ top: 12, right: 12, bottom: 4, left: 0 }}
            accessibilityLayer={false}
          >
            <CartesianGrid
              vertical={false}
              stroke="var(--line)"
              strokeDasharray="3 5"
            />
            <XAxis
              dataKey="at"
              type="number"
              scale="time"
              domain={[start, end]}
              ticks={[start, (start + end) / 2, end]}
              tickFormatter={(n) =>
                new Date(start).toDateString() !== new Date(end).toDateString()
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
              tickMargin={10}
            />
            <YAxis
              domain={[0, ceiling]}
              ticks={[0, ceiling / 2, ceiling]}
              tickFormatter={(n) =>
                max === 100 ? `${n}%` : n.toFixed(ceiling < 10 ? 1 : 0)
              }
              tickLine={false}
              axisLine={false}
              width={42}
              tick={{ fill: "var(--muted-foreground)", fontSize: 10 }}
            />
            <Tooltip
              isAnimationActive={false}
              cursor={{
                stroke: "var(--muted-foreground)",
                strokeDasharray: "3 3",
              }}
              content={({ active, payload, label }) =>
                active && payload?.length ? (
                  <div className="metric-tooltip">
                    <time>
                      {new Date(start).toDateString() !==
                      new Date(end).toDateString()
                        ? new Date(Number(label)).toLocaleString()
                        : time(Number(label))}
                    </time>
                    {payload.map((p) => (
                      <div key={String(p.dataKey)}>
                        <span>{p.name}</span>
                        <strong>
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
                  stroke="var(--chart-1)"
                  strokeWidth={2}
                  fill="var(--chart-1)"
                  fillOpacity={0.09}
                  connectNulls={false}
                  isAnimationActive={false}
                  dot={isolatedDot(s, i)}
                  activeDot={{ r: 4, stroke: "var(--panel)", strokeWidth: 2 }}
                />
              ) : (
                <Line
                  key={s.name}
                  dataKey={`v${i}`}
                  name={s.name}
                  type="linear"
                  stroke={s.dashed ? "var(--green)" : "var(--chart-1)"}
                  strokeWidth={2}
                  strokeDasharray={s.dashed ? "5 4" : undefined}
                  connectNulls={false}
                  isAnimationActive={false}
                  dot={isolatedDot(s, i)}
                  activeDot={{ r: 4, stroke: "var(--panel)", strokeWidth: 2 }}
                />
              ),
            )}
          </ComposedChart>
        </ResponsiveContainer>
      </div>
    </figure>
  );
}
export function MetricGraphs({
  history,
  minutes,
  onMinutesChange,
}: {
  history: MetricSample[];
  minutes: number;
  onMinutesChange: (minutes: number) => void;
}) {
  const [iface, setIface] = useState("");
  const [showSamples, setShowSamples] = useState(false);
  const latest = history.at(-1);
  const end = latest?.at ?? Date.now();
  const samples = history.filter((p) => p.at >= end - minutes * 60000);
  const start = historyStart(samples, end, minutes);
  const interfaces = Object.keys(latest?.network ?? {});
  const selected = interfaces.includes(iface)
    ? iface
    : (interfaces.find((n) => n !== "lo") ?? interfaces[0] ?? "");
  const net = (key: "rx" | "tx") => (p: MetricSample) =>
    p.network[selected]?.[key] ?? null;
  // Network values are scaled together; the unit remains explicit on both axis and readings.
  const netMax = samples.reduce(
    (highest, p) => Math.max(highest, net("rx")(p) ?? 0, net("tx")(p) ?? 0),
    1,
  );
  const scale = netMax >= 1048576 ? 1048576 : netMax >= 1024 ? 1024 : 1;
  const unit = scale === 1048576 ? "MiB/s" : scale === 1024 ? "KiB/s" : "B/s";
  return (
    <section className="metric-history">
      <div className="section-heading">
        <h2>Usage history</h2>
        <Select
          aria-label="History time window"
          value={String(minutes)}
          onValueChange={(value) => onMinutesChange(Number(value))}
        >
          <SelectItem value="5">Last 5 minutes</SelectItem>
          <SelectItem value="15">Last 15 minutes</SelectItem>
          <SelectItem value="60">Last hour</SelectItem>
          <SelectItem value="1440">Last 24 hours</SelectItem>
          <SelectItem value="10080">Last 7 days</SelectItem>
        </Select>
      </div>
      <p className="history-caption">
        {latest ? `Last reading · ${time(latest.at)}` : "No readings yet"}
        <span>Gaps indicate missing readings</span>
      </p>
      <div className="metric-grid">
        <Chart
          title="CPU"
          samples={samples}
          series={[{ name: "Usage", read: (p) => p.cpu }]}
          max={100}
          format={percent}
          start={start}
          end={end}
          intervalMs={
            minutes === 10080 ? 30 * 60000 : minutes === 1440 ? 5 * 60000 : 0
          }
        />
        <Chart
          title="Memory"
          samples={samples}
          series={[{ name: "Used", read: (p) => p.memory }]}
          max={100}
          format={percent}
          start={start}
          end={end}
          intervalMs={
            minutes === 10080 ? 30 * 60000 : minutes === 1440 ? 5 * 60000 : 0
          }
        />
        <Chart
          title="Load average"
          samples={samples}
          series={[{ name: "1 minute", read: (p) => p.load }]}
          format={(n) => n.toFixed(2)}
          start={start}
          end={end}
          intervalMs={
            minutes === 10080 ? 30 * 60000 : minutes === 1440 ? 5 * 60000 : 0
          }
        />
        <div>
          <label className="network-interface">
            Interface{" "}
            <Select
              aria-label="Graph network interface"
              value={selected}
              onValueChange={setIface}
            >
              {interfaces.map((n) => (
                <SelectItem key={n} value={n}>
                  {n}
                </SelectItem>
              ))}
            </Select>
          </label>
          <Chart
            title={`Network · ${unit}`}
            samples={samples}
            series={[
              {
                name: "Receive",
                read: (p) =>
                  net("rx")(p) === null ? null : net("rx")(p)! / scale,
              },
              {
                name: "Send",
                read: (p) =>
                  net("tx")(p) === null ? null : net("tx")(p)! / scale,
                dashed: true,
              },
            ]}
            format={(n) => rate(n * scale)}
            start={start}
            end={end}
            intervalMs={
              minutes === 10080 ? 30 * 60000 : minutes === 1440 ? 5 * 60000 : 0
            }
          />
        </div>
      </div>
      {samples.length > 0 && (
        <details
          className="history-data system-details"
          onToggle={(event) => setShowSamples(event.currentTarget.open)}
        >
          <summary>View readings</summary>
          <p className="muted">
            Newest first · Network: {selected || "unavailable"} · — means
            unavailable
          </p>
          <div
            className="history-table"
            tabIndex={0}
            role="region"
            aria-label="Recorded metric samples"
          >
            {showSamples && (
              <Table
                className="cockpit-table recorded-samples-table"
                aria-label="Recorded metric samples"
              >
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
                      <TableHead
                        scope="col"
                        key={label}
                        className={label === "Time" ? undefined : "numeric"}
                      >
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
        </details>
      )}
      {samples.length < 2 && (
        <p className="muted">
          {samples.length === 0
            ? "No history yet. Readings will appear here as they arrive."
            : "Network speed needs two readings."}
        </p>
      )}
    </section>
  );
}
