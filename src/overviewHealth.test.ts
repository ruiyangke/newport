import { describe, expect, it } from "vitest";
import { assess, disksByFullness, duration, level, loadShare } from "./overviewHealth";
import type { Overview } from "./domain/cockpit";

const GiB = 1024 ** 3;
// An invented, comfortable server; each case strains one resource.
const server = (value: Partial<Overview> = {}): Overview => ({
  hostname: "atlas-01",
  os: "Debian 13",
  kernel: "6.12.0",
  cores: 4,
  uptime: 86400,
  load: [0.5, 0.4, 0.3],
  cpu: 12,
  memoryTotal: 8 * GiB,
  memoryUsed: 2 * GiB,
  swapTotal: 2 * GiB,
  swapUsed: 0,
  disks: [
    {
      mount: "/",
      device: "/dev/vda1",
      total: 100 * GiB,
      used: 40 * GiB,
      available: 60 * GiB,
    },
  ],
  network: [],
  processes: [],
  processCount: 90,
  ...value,
});

describe("levels", () => {
  it("are inclusive at each threshold and ignore unknown values", () => {
    const limits = { warning: 80, critical: 95 };
    expect(level(79.9, limits)).toBe("ok");
    expect(level(80, limits)).toBe("warning");
    expect(level(95, limits)).toBe("critical");
    expect(level(NaN, limits)).toBe("ok");
  });
});

describe("the verdict", () => {
  it("is healthy when every resource is within range", () => {
    const health = assess(server());
    expect(health.level).toBe("ok");
    expect(health.issues).toEqual([]);
  });

  it("names each strained resource, critical ones first", () => {
    const health = assess(
      server({
        cpu: 85,
        memoryUsed: 7.8 * GiB,
        disks: [
          {
            mount: "/",
            device: "/dev/vda1",
            total: 100 * GiB,
            used: 40 * GiB,
            available: 60 * GiB,
          },
          {
            mount: "/srv",
            device: "/dev/vdb1",
            total: 500 * GiB,
            used: 440 * GiB,
            available: 60 * GiB,
          },
        ],
      }),
    );
    expect(health.level).toBe("critical");
    expect(health.issues.map((i) => [i.level, i.subject])).toEqual([
      ["critical", "memory"],
      ["warning", "cpu"],
      ["warning", "disk:/srv"],
    ]);
    expect(health.issues[0].message).toBe(
      "Memory is 98% used · 204.8 MiB available",
    );
    expect(health.issues[2].message).toBe("/srv is 88% full · 60.0 GiB free");
    expect(health.of("disk:/")).toBe("ok");
  });

  it("judges load per core, so a big host is not alarming at the same load", () => {
    expect(assess(server({ load: [6, 5, 4] })).of("load")).toBe("warning");
    expect(assess(server({ load: [6, 5, 4], cores: 32 })).of("load")).toBe(
      "ok",
    );
    expect(assess(server({ load: [9, 5, 4] })).issues[0].message).toBe(
      "Load is 2.3× the 4 cores; work is waiting",
    );
    expect(loadShare(server({ load: [6, 5, 4] }))).toBe(150);
  });

  it("treats swap in use as normal and nearly full swap as pressure", () => {
    expect(assess(server({ swapUsed: 0.5 * GiB })).of("swap")).toBe("ok");
    expect(assess(server({ swapUsed: 1.9 * GiB })).of("swap")).toBe("critical");
    expect(assess(server({ swapTotal: 0, swapUsed: 0 })).issues).toEqual([]);
  });
});

it("orders disks fullest first", () => {
  const disks = disksByFullness(
    server({
      disks: [
        { mount: "/", device: "a", total: 10, used: 4, available: 6 },
        { mount: "/srv", device: "b", total: 10, used: 9, available: 1 },
        { mount: "/boot", device: "c", total: 10, used: 2, available: 8 },
      ],
    }),
  );
  expect(disks.map((d) => d.mount)).toEqual(["/srv", "/", "/boot"]);
});

it("formats process running times compactly", () => {
  expect(duration(0)).toBe("0s");
  expect(duration(45)).toBe("45s");
  expect(duration(90)).toBe("1m 30s");
  expect(duration(3600)).toBe("1h 0m");
  expect(duration(90061)).toBe("1d 1h");
  expect(duration(-5)).toBe("0s");
});
