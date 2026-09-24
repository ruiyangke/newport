import { describe, expect, it } from "vitest";
import {
  addSample,
  chartPoints,
  averageChartPoints,
  historyStart,
  restoreHistory,
  withHistoryGaps,
  type MetricInput,
  type MetricSample,
} from "./metricHistory";
const input = (received = 1000, sent = 500, uptime = 100): MetricInput => ({
  cpu: 25,
  memoryUsed: 4,
  memoryTotal: 8,
  load: [1, 2, 3],
  uptime,
  network: [{ name: "eth0", received, sent }],
});
describe("metric history", () => {
  it("restores saved samples without losing newer live samples", () => {
    const live = addSample([], input(9000, 4500), 21000);
    const samples = restoreHistory(live, [
      { at: 1000, data: input() },
      { at: 11000, data: input(5000, 2500) },
    ]);
    expect(samples.map((s) => s.at)).toEqual([1000, 11000, 21000]);
    expect(samples[2].network.eth0.rx).toBe(400);
    expect(restoreHistory(samples, [{ at: 1000, data: input() }])).toHaveLength(
      3,
    );
  });
  it("derives rates from elapsed time and isolates servers", () => {
    const initial = addSample([], input(), 1000);
    const data = addSample(initial, input(5000, 2500), 11000);
    expect(data[0].network.eth0.rx).toBeNull();
    expect(data[1].network.eth0.rx).toBe(400);
    expect(data[1].network.eth0.tx).toBe(200);
    expect(data[1].memory).toBe(50);
    expect(initial).toHaveLength(1);
  });
  it("does not turn counter resets, reboot, or pauses into traffic", () => {
    const initial = addSample([], input(), 1000);
    expect(
      addSample(initial, input(100, 100), 11000).at(-1)?.network.eth0.rx,
    ).toBeNull();
    expect(
      addSample(initial, input(200, 200, 1), 21000).at(-1)?.network.eth0.rx,
    ).toBeNull();
    expect(
      addSample(initial, input(10000, 10000, 60), 81000).at(-1)?.network.eth0
        .rx,
    ).toBeNull();
  });
  it("bounds history and ignores out-of-order samples", () => {
    let samples: MetricSample[] = [];
    for (let i = 0; i < 200; i++)
      samples = addSample(samples, input(), i * 10000);
    expect(samples.length).toBe(200);
    expect(addSample(samples, input(), 8 * 86400000)).toHaveLength(1);
    expect(addSample(samples, input(), 0)).toBe(samples);
  });
  it("inserts null chart rows at collection gaps without changing actual samples", () => {
    const points = [
      { at: 0, v0: 0 },
      { at: 10000, v0: 50 },
      { at: 20000, v0: null },
      { at: 90000, v0: 40 },
    ];
    const rows = withHistoryGaps(points);
    expect(rows).toHaveLength(5);
    expect(rows[3]).toEqual({ at: 20001, v0: null });
    expect(rows[4]).toEqual(points[3]);
  });
});

it("restores minute rates and preserves missing minute gaps", () => {
  const minute = (at: number) => ({
    at,
    data: {
      ...input(),
      resolutionMs: 60000,
      network: [{ name: "eth0", received: 5000, sent: 2500, rx: 100, tx: 50 }],
    },
  });
  const samples = restoreHistory([], [minute(0), minute(60000)]);
  expect(samples[1].network.eth0.rx).toBe(100);
  expect(restoreHistory(samples, [minute(0)])[1].network.eth0.tx).toBe(50);
  expect(
    withHistoryGaps(
      samples.map((p) => ({
        at: p.at,
        resolutionMs: p.resolutionMs!,
        v0: p.cpu,
      })),
    ),
  ).toHaveLength(2);
  expect(
    withHistoryGaps([
      { at: 0, resolutionMs: 60000, v0: 10 },
      { at: 120000, resolutionMs: 60000, v0: 20 },
    ]),
  ).toHaveLength(3);
  expect(
    withHistoryGaps([
      { at: 0, resolutionMs: 60000, incomplete: 1, v0: 10 },
      { at: 60000, resolutionMs: 60000, v0: 20 },
    ]),
  ).toHaveLength(3);
  const live = addSample(samples, input(9000, 4500), 70000);
  expect(live.at(-1)?.network.eth0.rx).toBeNull();
});

it("keeps seven-day history and chart extrema with gap boundaries", () => {
  const saved = [0, 86400000, 6 * 86400000].map((at) => ({
    at,
    data: input(),
  }));
  expect(restoreHistory([], saved)).toHaveLength(3);
  const points = Array.from({ length: 2000 }, (_, at) => ({
    at,
    v0: at === 333 ? 100 : at === 777 ? null : 5,
  }));
  const reduced = chartPoints(points, 20);
  expect(reduced.length).toBeLessThan(200);
  expect(reduced).toContainEqual(points[333]);
  expect(reduced).toContainEqual(points[776]);
  expect(reduced).toContainEqual(points[777]);
  expect(reduced).toContainEqual(points[778]);
});

it("limits a week to 336 weighted averages and retains empty periods", () => {
  const rows = averageChartPoints(
    [
      { at: 0, resolutionMs: 10000, v0: 100, v1: null },
      { at: 10000, resolutionMs: 60000, v0: 30, v1: 50 },
      { at: 3600000, resolutionMs: 10000, v0: 20, v1: null },
    ],
    0,
    7 * 86400000,
  );
  expect(rows).toHaveLength(336);
  expect(rows[0].v0).toBe(40);
  expect(rows[0].v1).toBe(50);
  expect(rows[1].v0).toBeNull();
  expect(rows[2].v0).toBe(20);
  expect(rows[2].v1).toBeNull();
});

it("fits available history without extending beyond the selected range", () => {
  const end = 7 * 86400000;
  expect(historyStart([{ at: end - 2 * 86400000 }], end, 10080)).toBe(
    end - 2 * 86400000,
  );
  expect(historyStart([{ at: 0 }], end, 60)).toBe(end - 3600000);
  expect(historyStart([{ at: end }], end, 10080)).toBe(end - 60000);
  expect(historyStart([], end, 10080)).toBe(0);
});

it("reduces the 24-hour view to five-minute averages", () => {
  const rows = averageChartPoints(
    [
      { at: 0, resolutionMs: 10000, v0: 20 },
      { at: 10000, resolutionMs: 10000, v0: 60 },
      { at: 600000, resolutionMs: 10000, v0: 10 },
    ],
    0,
    86400000,
    5 * 60000,
  );
  expect(rows).toHaveLength(288);
  expect(rows[0].v0).toBe(40);
  expect(rows[1].v0).toBeNull();
  expect(rows[2].v0).toBe(10);
});
