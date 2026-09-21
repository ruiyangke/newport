export interface MetricInput {
  cpu: number;
  memoryUsed: number;
  memoryTotal: number;
  load: number[];
  uptime: number;
  network: { name: string; received: number; sent: number }[];
}
export interface MetricSample {
  at: number;
  cpu: number;
  memory: number;
  load: number;
  uptime: number;
  network: Record<
    string,
    { received: number; sent: number; rx: number | null; tx: number | null }
  >;
}
export const HISTORY_MS = 15 * 60 * 1000;
export const GAP_MS = 30000;
export function addSample(
  previous: MetricSample[],
  input: MetricInput,
  at: number,
): MetricSample[] {
  const last = previous.at(-1);
  if (last && at <= last.at) return previous;
  const seconds = last ? (at - last.at) / 1000 : 0;
  const continuous =
    last && at - last.at <= GAP_MS && input.uptime >= last.uptime;
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
          rx: rate("received"),
          tx: rate("sent"),
        },
      ];
    }),
  );
  const sample: MetricSample = {
    at,
    cpu: input.cpu,
    memory: input.memoryTotal
      ? (input.memoryUsed / input.memoryTotal) * 100
      : 0,
    load: input.load[0],
    uptime: input.uptime,
    network,
  };
  const next = [
    ...previous.filter((p) => at - p.at <= HISTORY_MS),
    sample,
  ].slice(-180);
  return next;
}
export interface SavedSample {
  at: number;
  data: MetricInput;
}
export function restoreHistory(previous: MetricSample[], saved: SavedSample[]) {
  // A late disk read must never replace samples already received in this session.
  const merged = new Map<number, MetricInput>(saved.map((p) => [p.at, p.data]));
  for (const p of previous)
    merged.set(p.at, {
      cpu: p.cpu,
      memoryUsed: p.memory,
      memoryTotal: 100,
      load: [p.load],
      uptime: p.uptime,
      network: Object.entries(p.network).map(([name, n]) => ({
        name,
        received: n.received,
        sent: n.sent,
      })),
    });
  let result: MetricSample[] = [];
  for (const [at, data] of [...merged].sort(([a], [b]) => a - b))
    result = addSample(result, data, at);
  return result;
}
export type ChartRow = { at: number; [key: string]: number | null };
export function withHistoryGaps(points: ChartRow[]): ChartRow[] {
  return points.flatMap((p, i) => {
    const previous = points[i - 1];
    if (!previous || p.at - previous.at <= GAP_MS) return [p];
    const gap: ChartRow = { at: previous.at + 1 };
    for (const key of Object.keys(p)) if (key !== "at") gap[key] = null;
    return [gap, p];
  });
}
