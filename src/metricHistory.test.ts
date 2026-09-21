import { describe, expect, it } from "vitest";
import {
  addSample,
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
    expect(samples.length).toBe(91);
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
