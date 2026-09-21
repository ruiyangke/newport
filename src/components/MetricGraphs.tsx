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
import { GAP_MS, withHistoryGaps, type MetricSample } from "../metricHistory";
const percent = (n: number) => `${n.toFixed(1)}%`;
const rate = (n: number) =>
  n >= 1048576
    ? `${(n / 1048576).toFixed(1)} MiB/s`
    : n >= 1024
      ? `${(n / 1024).toFixed(1)} KiB/s`
      : `${n.toFixed(0)} B/s`;
const time = (n: number) =>
  new Date(n).toLocaleTimeString([], {
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
}: {
  title: string;
  samples: MetricSample[];
  series: Series[];
  max?: number;
  format: (n: number) => string;
  start: number;
  end: number;
}) {
  const selected = samples.at(-1);
  const ceiling =
    max ??
    Math.max(1, ...samples.flatMap((p) => series.map((s) => s.read(p) ?? 0))) *
      1.15;
  const isolatedDot =
    (series: Series) =>
    (props: { cx?: number; cy?: number; payload?: { at?: number } }) => {
      const index = samples.findIndex((p) => p.at === props.payload?.at);
      const sample = samples[index];
      if (
        !sample ||
        series.read(sample) === null ||
        props.cx === undefined ||
        props.cy === undefined
      )
        return <g />;
      const connected = (neighbor: MetricSample | undefined) =>
        neighbor &&
        series.read(neighbor) !== null &&
        Math.abs(neighbor.at - sample.at) <= GAP_MS;
      if (connected(samples[index - 1]) || connected(samples[index + 1]))
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
            data={withHistoryGaps(
              samples.map((p) =>
                Object.assign(
                  { at: p.at },
                  Object.fromEntries(
                    series.map((s, i) => [`v${i}`, s.read(p)]),
                  ),
                ),
              ),
            )}
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
                new Date(n).toLocaleTimeString([], {
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
                    <time>{time(Number(label))}</time>
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
                  dot={isolatedDot(s)}
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
                  dot={isolatedDot(s)}
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
export function MetricGraphs({ history }: { history: MetricSample[] }) {
  const [minutes, setMinutes] = useState(5);
  const [iface, setIface] = useState("");
  const latest = history.at(-1);
  const end = latest?.at ?? Date.now();
  const start = end - minutes * 60000;
  const samples = history.filter((p) => p.at >= start);
  const interfaces = Object.keys(latest?.network ?? {});
  const selected = interfaces.includes(iface)
    ? iface
    : (interfaces.find((n) => n !== "lo") ?? interfaces[0] ?? "");
  const net = (key: "rx" | "tx") => (p: MetricSample) =>
    p.network[selected]?.[key] ?? null;
  // Network values are scaled together; the unit remains explicit on both axis and readings.
  const netMax = Math.max(
    1,
    ...samples.flatMap((p) => [net("rx")(p) ?? 0, net("tx")(p) ?? 0]),
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
          onValueChange={(value) => setMinutes(Number(value))}
        >
          <SelectItem value="5">Last 5 minutes</SelectItem>
          <SelectItem value="15">Last 15 minutes</SelectItem>
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
        />
        <Chart
          title="Memory"
          samples={samples}
          series={[{ name: "Used", read: (p) => p.memory }]}
          max={100}
          format={percent}
          start={start}
          end={end}
        />
        <Chart
          title="Load average"
          samples={samples}
          series={[{ name: "1 minute", read: (p) => p.load }]}
          format={(n) => n.toFixed(2)}
          start={start}
          end={end}
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
          />
        </div>
      </div>
      {samples.length > 0 && (
        <details className="history-data system-details">
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
