export interface MetricInput {
  cpu: number;
  memoryUsed: number;
  memoryTotal: number;
  load: number[];
  uptime: number;
  resolutionMs?: number;
  gap?: boolean;
  network: {
    name: string;
    received: number;
    sent: number;
    rx?: number | null;
    tx?: number | null;
  }[];
}
export interface MetricSample {
  at: number;
  resolutionMs?: number;
  incomplete?: number;
  cpu: number;
  memory: number;
  load: number;
  uptime: number;
  network: Record<
    string,
    { received: number; sent: number; rx: number | null; tx: number | null }
  >;
}
export const HISTORY_MS = 7 * 24 * 60 * 60 * 1000;
const MAX_SAMPLES = 120960;
export const GAP_MS = 30000;
export function addSample(
  previous: MetricSample[],
  input: MetricInput,
  at: number,
): MetricSample[] {
  const last = previous.at(-1);
  if (last && at <= last.at) return previous;
  const sample = makeSample(last, input, at);
  const next = [
    ...previous.filter((p) => at - p.at <= HISTORY_MS),
    sample,
  ].slice(-MAX_SAMPLES);
  return next;
}
function makeSample(
  last: MetricSample | undefined,
  input: MetricInput,
  at: number,
): MetricSample {
  const seconds = last ? (at - last.at) / 1000 : 0;
  const continuous =
    last &&
    (last.resolutionMs ?? 10000) === 10000 &&
    at - last.at <= GAP_MS &&
    input.uptime >= last.uptime;
  const network = Object.fromEntries(
    input.network.map((n) => {
      const old = continuous ? last.network[n.name] : undefined;
      const rate = (key: "received" | "sent") =>
        old && n[key] >= old[key] ? (n[key] - old[key]) / seconds : null;
      return [
        n.name,
        {
          received: n.received,
          sent: n.sent,
          rx: input.resolutionMs === 60000 ? (n.rx ?? null) : rate("received"),
          tx: input.resolutionMs === 60000 ? (n.tx ?? null) : rate("sent"),
        },
      ];
    }),
  );
  return {
    at,
    resolutionMs: input.resolutionMs ?? 10000,
    incomplete: input.gap ? 1 : 0,
    cpu: input.cpu,
    memory: input.memoryTotal
      ? (input.memoryUsed / input.memoryTotal) * 100
      : 0,
    load: input.load[0],
    uptime: input.uptime,
    network,
  };
}
export interface SavedSample {
  at: number;
  data: MetricInput;
}
export function restoreHistory(previous: MetricSample[], saved: SavedSample[]) {
  // A late disk read must never replace samples already received in this session.
  const merged = new Map<number, MetricInput>(saved.map((p) => [p.at, p.data]));
  const summarized = new Set(
    saved
      .filter((p) => p.data.resolutionMs === 60000)
      .map((p) => Math.floor(p.at / 60000)),
  );
  for (const p of previous) {
    if (summarized.has(Math.floor(p.at / 60000))) continue;
    merged.set(p.at, {
      cpu: p.cpu,
      resolutionMs: p.resolutionMs,
      gap: Boolean(p.incomplete),
      memoryUsed: p.memory,
      memoryTotal: 100,
      load: [p.load],
      uptime: p.uptime,
      network: Object.entries(p.network).map(([name, n]) => ({
        name,
        received: n.received,
        sent: n.sent,
        rx: n.rx,
        tx: n.tx,
      })),
    });
  }
  const result: MetricSample[] = [];
  const sorted = [...merged].sort(([a], [b]) => a - b);
  const cutoff = (sorted.at(-1)?.[0] ?? 0) - HISTORY_MS;
  for (const [at, data] of sorted
    .filter(([at]) => at >= cutoff)
    .slice(-MAX_SAMPLES))
    result.push(makeSample(result.at(-1), data, at));
  return result;
}
export type ChartRow = { at: number; [key: string]: number | null };
export function hasHistoryGap(
  a: { at: number; resolutionMs?: number | null; incomplete?: number | null },
  b: { at: number; resolutionMs?: number | null; incomplete?: number | null },
) {
  return (
    Boolean(a.incomplete || b.incomplete) ||
    Math.abs(b.at - a.at) >
      Math.max(GAP_MS, a.resolutionMs ?? 10000, b.resolutionMs ?? 10000)
  );
}
export function withHistoryGaps(points: ChartRow[]): ChartRow[] {
  return points.flatMap((p, i) => {
    const previous = points[i - 1];
    if (!previous || !hasHistoryGap(previous, p)) return [p];
    const gap: ChartRow = { at: previous.at + 1 };
    for (const key of Object.keys(p)) if (key !== "at") gap[key] = null;
    return [gap, p];
  });
}

// Reduce drawing work without removing extrema or the boundaries of gaps.
export function chartPoints(points: ChartRow[], buckets = 400): ChartRow[] {
  if (points.length <= buckets * 2) return points;
  const keys = Object.keys(points[0]).filter((key) => key.startsWith("v"));
  const keep = new Set<number>();
  const size = Math.ceil(points.length / buckets);
  for (let start = 0; start < points.length; start += size) {
    const end = Math.min(start + size, points.length);
    keep.add(start);
    keep.add(end - 1);
    for (const key of keys) {
      let min = start,
        max = start;
      for (let i = start; i < end; i++) {
        const value = points[i][key];
        if (value === null) {
          if (i > 0 && points[i - 1][key] !== null) {
            keep.add(i - 1);
            keep.add(i);
          }
          if (i + 1 < points.length && points[i + 1][key] !== null) {
            keep.add(i);
            keep.add(i + 1);
          }
        } else {
          if (points[min][key] === null || value < points[min][key]!) min = i;
          if (points[max][key] === null || value > points[max][key]!) max = i;
        }
      }
      keep.add(min);
      keep.add(max);
    }
  }
  return [...keep].sort((a, b) => a - b).map((i) => points[i]);
}

/** Display-only, duration-weighted averages; empty intervals remain gaps. */
export function averageChartPoints(
  points: ChartRow[],
  start: number,
  end: number,
  interval = 30 * 60000,
): ChartRow[] {
  if (!points.length || end <= start || interval <= 0) return [];
  const count = Math.ceil((end - start) / interval);
  const keys = Object.keys(points[0]).filter((key) => /^v\d+$/.test(key));
  const buckets = Array.from({ length: count }, (_, i) => ({
    at:
      start + i * interval + Math.min(interval, end - start - i * interval) / 2,
    sums: new Map<string, number>(),
    weights: new Map<string, number>(),
  }));
  for (const point of points) {
    if (point.at < start || point.at > end) continue;
    const bucket =
      buckets[Math.min(count - 1, Math.floor((point.at - start) / interval))];
    const weight = point.resolutionMs ?? 10000;
    for (const key of keys) {
      const value = point[key];
      if (value === null || value === undefined || !Number.isFinite(value))
        continue;
      bucket.sums.set(key, (bucket.sums.get(key) ?? 0) + value * weight);
      bucket.weights.set(key, (bucket.weights.get(key) ?? 0) + weight);
    }
  }
  return buckets.map((bucket) =>
    Object.assign(
      { at: bucket.at, resolutionMs: interval },
      Object.fromEntries(
        keys.map((key) => [
          key,
          bucket.weights.has(key)
            ? bucket.sums.get(key)! / bucket.weights.get(key)!
            : null,
        ]),
      ),
    ),
  );
}

/** Fit available history inside the selected range, keeping singleton charts usable. */
export function historyStart(
  samples: { at: number }[],
  end: number,
  minutes: number,
): number {
  const earliest = samples[0]?.at;
  const requested = end - minutes * 60000;
  return earliest === undefined
    ? requested
    : Math.max(requested, Math.min(earliest, end - 60000));
}
