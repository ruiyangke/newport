import { describe, expect, it } from "vitest";
import {
  composeWords,
  displayName,
  exitMeaning,
  exposedPorts,
  forwardable,
  groupContainers,
  isProblem,
  listNote,
  logTone,
  portLabel,
  problemReason,
  projectState,
  publishedPorts,
  shellCommand,
  stateOf,
  type Container,
} from "./containers";

const c = (name: string, extra: Partial<Container> = {}): Container => ({
  id: name.padEnd(64, "0").slice(0, 64),
  name,
  image: "example/app:1",
  imageId: "sha256:1",
  command: "app",
  state: "running",
  status: "Up 2 hours",
  health: null,
  exitCode: null,
  oomKilled: false,
  error: null,
  restartCount: 0,
  restartPolicy: "unless-stopped",
  created: 0,
  startedAt: 0,
  finishedAt: null,
  ports: [],
  compose: null,
  cpu: 1.5,
  memory: 1024,
  memoryLimit: null,
  ...extra,
});
const svc = (
  project: string,
  service: string,
  extra: Partial<Container> = {},
) =>
  c(`${project}-${service}-1`, {
    compose: {
      project,
      service,
      oneoff: false,
      number: 1,
      workingDir: `/srv/${project}`,
      configFiles: null,
    },
    ...extra,
  });
const exited = (code: number, extra: Partial<Container> = {}) => ({
  state: "exited" as const,
  exitCode: code,
  cpu: null,
  memory: null,
  finishedAt: 0,
  ...extra,
});

describe("problems", () => {
  it("counts what went down on its own, not what someone stopped", () => {
    expect(isProblem(c("a", exited(1)))).toBe(true);
    expect(isProblem(c("a", exited(139)))).toBe(true);
    for (const code of [0, 130, 137, 143])
      expect(isProblem(c("a", exited(code)))).toBe(false);
    expect(isProblem(c("a", exited(137, { oomKilled: true })))).toBe(true);
    expect(isProblem(c("a", { health: "unhealthy" }))).toBe(true);
    // A stopped container keeps its last health status; that's not a crash.
    expect(isProblem(c("a", exited(0, { health: "unhealthy" })))).toBe(false);
    expect(isProblem(c("a", { state: "restarting" }))).toBe(true);
    expect(isProblem(c("a", { state: "dead" }))).toBe(true);
    expect(isProblem(c("a"))).toBe(false);
  });

  it("explains why, in words", () => {
    expect(problemReason(c("a", { health: "unhealthy" }))).toBe(
      "Failing its health check",
    );
    expect(problemReason(c("a", exited(137, { oomKilled: true })))).toBe(
      "Killed for running out of memory",
    );
    expect(problemReason(c("a", exited(127)))).toBe(
      "Its command wasn't found (code 127)",
    );
    expect(
      problemReason(c("a", { state: "restarting", restartCount: 6 })),
    ).toBe("Keeps exiting; restarted 6×");
    expect(problemReason(c("a", exited(143)))).toBeNull();
    expect(exitMeaning(132)).toBe("Killed by signal 4 (code 132)");
  });

  it("names the state and how loud it is", () => {
    expect(stateOf(c("a"))).toEqual({ label: "Running", tone: "success" });
    expect(stateOf(c("a", { health: "unhealthy" })).label).toBe("Unhealthy");
    expect(stateOf(c("a", exited(1))).label).toBe("Crashed");
    expect(stateOf(c("a", exited(0))).label).toBe("Stopped");
    expect(stateOf(c("a", { state: "paused" })).label).toBe("Paused");
  });
});

describe("grouping", () => {
  it("leads with problems, then projects, then standalone containers", () => {
    const groups = groupContainers([
      c("portainer"),
      svc("atlas", "worker", { health: "unhealthy" }),
      svc("atlas", "api"),
      svc("ledger", "web", exited(1)),
      c("atlas-migrate-run-1", {
        ...exited(0),
        compose: {
          project: "atlas",
          service: "migrate",
          oneoff: true,
          number: null,
          workingDir: null,
          configFiles: null,
        },
      }),
    ]);
    expect(groups.problems.map((x) => x.name)).toEqual([
      "ledger-web-1",
      "atlas-worker-1",
    ]);
    expect(groups.projects.map((p) => p.name)).toEqual(["atlas", "ledger"]);
    const atlas = groups.projects[0];
    expect(atlas.services.map(displayName)).toEqual(["api", "worker"]);
    expect(atlas.jobs.map((x) => x.name)).toEqual(["atlas-migrate-run-1"]);
    expect(atlas.workingDir).toBe("/srv/atlas");
    expect(projectState(atlas).label).toBe("Needs attention");
    expect(projectState(groups.projects[1]).label).toBe("Needs attention");
    expect(groups.standalone.map((x) => x.name)).toEqual(["portainer"]);
  });

  it("summarises a project from its long-running services only", () => {
    const [p] = groupContainers([
      svc("a", "api"),
      svc("a", "db", exited(0)),
    ]).projects;
    expect(projectState(p)).toEqual({
      label: "Partly running",
      tone: "warning",
    });
  });
});

describe("ports", () => {
  const ports = c("a", {
    ports: [
      { ip: "0.0.0.0", hostPort: 8080, containerPort: 80, protocol: "tcp" },
      { ip: "::", hostPort: 8080, containerPort: 80, protocol: "tcp" },
      { ip: null, hostPort: null, containerPort: 5432, protocol: "tcp" },
      { ip: "10.0.0.5", hostPort: 9000, containerPort: 9000, protocol: "tcp" },
      { ip: "0.0.0.0", hostPort: 53, containerPort: 53, protocol: "udp" },
    ],
  });
  it("lists each published port once, preferring the IPv4 binding", () => {
    expect(publishedPorts(ports).map(portLabel)).toEqual([
      "53/udp",
      "8080→80",
      "9000",
    ]);
    expect(publishedPorts(ports)[1].ip).toBe("0.0.0.0");
    expect(exposedPorts(ports).map(portLabel)).toEqual(["5432"]);
  });
  it("forwards TCP on loopback or every address only", () => {
    const [udp, web, pinned] = publishedPorts(ports);
    expect(forwardable(web)).toBe(true);
    expect(forwardable(udp)).toBe(false);
    expect(forwardable(pinned)).toBe(false);
  });
});

describe("words", () => {
  it("tones log lines by what they say", () => {
    expect(logTone("ERROR connection refused")).toBe("danger");
    expect(logTone('{"level":"error","msg":"x"}')).toBe("danger");
    expect(logTone("level=warn msg=slow")).toBe("warning");
    expect(logTone("GET /health 200")).toBe("muted");
    expect(logTone("no errors found")).toBe("muted");
  });
  it("keeps list notes short", () => {
    expect(listNote(c("a", { cpu: 12.4 }), 0)).toBe("12%");
    expect(listNote(c("a", { cpu: 0.42 }), 0)).toBe("0.4%");
    expect(listNote(c("a", exited(1)), 0)).toBe("exit 1");
    expect(listNote(c("a", exited(0, { finishedAt: 0 })), 3_600_000)).toBe(
      "1h ago",
    );
  });
  it("opens bash when the image has it, else sh", () => {
    expect(shellCommand(c("atlas-api-1"))).toBe(
      "docker exec -it atlas-api-1 sh -c 'command -v bash >/dev/null && exec bash || exec sh'",
    );
    expect(shellCommand(c("atlas-api-1"), true)).toMatch(
      /^podman exec -it atlas-api-1 /,
    );
  });
});

describe("compose words", () => {
  const up = (
    services: string[],
    o: Partial<{ build: boolean; pull: boolean; forceRecreate: boolean }> = {},
  ) =>
    composeWords({
      kind: "up",
      services,
      build: false,
      pull: false,
      forceRecreate: false,
      ...o,
    });
  it("names each action the way its button does", () => {
    expect(up([]).label).toBe("Up");
    expect(up([], { pull: true }).label).toBe("Pull & up");
    expect(up([], { build: true }).label).toBe("Rebuild & up");
    expect(up(["api"], { forceRecreate: true }).label).toBe("Recreate");
    expect(up(["api"], { forceRecreate: true, pull: true }).label).toBe(
      "Pull & recreate",
    );
    expect(up(["api"], { forceRecreate: true, build: true }).label).toBe(
      "Rebuild",
    );
    expect(
      composeWords({ kind: "down", volumes: true, removeOrphans: false }).label,
    ).toBe("Down & delete volumes");
  });
  it("says what is happening, to whom", () => {
    expect(up(["api"], { forceRecreate: true, pull: true }).progress).toBe(
      "Pulling and recreating api…",
    );
    expect(up(["api", "db"]).progress).toBe("Bringing 2 services up…");
    expect(
      composeWords({ kind: "scale", service: "worker", replicas: 3 }).progress,
    ).toBe("Scaling worker to 3…");
  });
});
