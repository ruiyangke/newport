import { useWorkspaceState } from "../state/workspace";
import { historyStart, type MetricSample } from "../metricHistory";
import { splitInterfaces } from "../networkInterfaces";

export type Metric = "cpu" | "memory" | "load" | "network";

export const rate = (n: number) =>
  n >= 1048576
    ? `${(n / 1048576).toFixed(1)} MiB/s`
    : n >= 1024
      ? `${(n / 1024).toFixed(1)} KiB/s`
      : `${n.toFixed(0)} B/s`;

/** The window's samples, and the network interface the charts follow. */
export function useMetricWindow(
  id: string,
  history: MetricSample[],
  minutes: number,
) {
  const [iface, setIface] = useWorkspaceState(id, "metrics.interface");
  const latest = history.at(-1);
  const end = latest?.at ?? Date.now();
  const samples = history.filter((p) => p.at >= end - minutes * 60000);
  const { primary, folded } = splitInterfaces(
    Object.entries(latest?.network ?? {}).map(([name, n]) => ({ name, ...n })),
    (n) => n.received + n.sent,
  );
  // Physical uplinks first, busiest first; virtual links are rarely meant.
  const interfaces = [...primary, ...folded].map((n) => n.name);
  const selected = interfaces.includes(iface) ? iface : (interfaces[0] ?? "");
  const net = (key: "rx" | "tx") => (p: MetricSample) =>
    p.network[selected]?.[key] ?? null;
  return {
    samples,
    latest,
    end,
    start: historyStart(samples, end, minutes),
    interfaces,
    primaryInterfaces: primary.map((n) => n.name),
    iface: selected,
    setIface,
    net,
  };
}
export type Window = ReturnType<typeof useMetricWindow>;
