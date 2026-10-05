import type { Overview } from "./domain/cockpit";

/** Reserved meaning: always shown with an icon and words, never color alone. */
export type Level = "ok" | "warning" | "critical";
export interface Issue {
  level: Exclude<Level, "ok">;
  /** What the issue is about, so the matching tile or disk can mark itself. */
  subject: "cpu" | "memory" | "swap" | "load" | `disk:${string}`;
  message: string;
}

/**
 * Where "fine" ends. Memory is MemTotal - MemAvailable, so reclaimable cache
 * never trips it; load is per core, so a busy 64-core host is not alarming
 * at a load of 8.
 */
export const THRESHOLDS = {
  cpu: { warning: 80, critical: 95 },
  memory: { warning: 85, critical: 95 },
  // Swap in use is normal; swap nearly full means memory is exhausted.
  swap: { warning: 50, critical: 90 },
  loadPerCore: { warning: 1, critical: 2 },
  disk: { warning: 85, critical: 95 },
} as const;

export function level(
  value: number,
  limits: { warning: number; critical: number },
): Level {
  if (!Number.isFinite(value)) return "ok";
  return value >= limits.critical
    ? "critical"
    : value >= limits.warning
      ? "warning"
      : "ok";
}

export const bytes = (n: number) => {
  if (n < 1024) return `${Math.round(n)} B`;
  const i = Math.min(4, Math.floor(Math.log(n) / Math.log(1024)));
  return `${(n / 1024 ** i).toFixed(1)} ${["B", "KiB", "MiB", "GiB", "TiB"][i]}`;
};
export const ratio = (used: number, total: number) =>
  total > 0 ? (used / total) * 100 : NaN;
/** The 1-minute load as a share of the cores; above 100 means queued work. */
export const loadShare = (overview: Overview) =>
  overview.cores > 0 ? ((overview.load[0] ?? 0) / overview.cores) * 100 : NaN;

/** Disks as people read them: fullest first. */
export function disksByFullness(overview: Overview) {
  return [...overview.disks].sort(
    (a, b) => ratio(b.used, b.total) - ratio(a.used, a.total),
  );
}

/**
 * The page's verdict: every resource checked against its threshold, worst
 * first. Only what this reading proves is claimed.
 */
export function assess(overview: Overview) {
  const issues: Issue[] = [];
  const check = (
    subject: Issue["subject"],
    value: number,
    limits: { warning: number; critical: number },
    message: (value: number) => string,
  ) => {
    const found = level(value, limits);
    if (found !== "ok")
      issues.push({ level: found, subject, message: message(value) });
  };
  check(
    "cpu",
    overview.cpu,
    THRESHOLDS.cpu,
    (v) => `CPU is ${Math.round(v)}% busy`,
  );
  check(
    "memory",
    ratio(overview.memoryUsed, overview.memoryTotal),
    THRESHOLDS.memory,
    (v) =>
      `Memory is ${Math.round(v)}% used · ${bytes(overview.memoryTotal - overview.memoryUsed)} available`,
  );
  if (overview.swapTotal > 0)
    check(
      "swap",
      ratio(overview.swapUsed, overview.swapTotal),
      THRESHOLDS.swap,
      (v) => `Swap is ${Math.round(v)}% used, a sign of memory pressure`,
    );
  if (overview.cores > 0)
    check(
      "load",
      (overview.load[0] ?? 0) / overview.cores,
      THRESHOLDS.loadPerCore,
      (v) =>
        `Load is ${v.toFixed(1)}× the ${overview.cores} ${overview.cores === 1 ? "core" : "cores"}; work is waiting`,
    );
  for (const disk of overview.disks)
    check(
      `disk:${disk.mount}`,
      ratio(disk.used, disk.total),
      THRESHOLDS.disk,
      (v) =>
        `${disk.mount} is ${Math.round(v)}% full · ${bytes(disk.available)} free`,
    );
  issues.sort(
    (a, b) => Number(b.level === "critical") - Number(a.level === "critical"),
  );
  const worst: Level = issues.some((i) => i.level === "critical")
    ? "critical"
    : issues.length
      ? "warning"
      : "ok";
  return {
    level: worst,
    issues,
    of: (subject: Issue["subject"]): Level =>
      issues.find((i) => i.subject === subject)?.level ?? "ok",
  };
}
