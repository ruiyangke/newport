import { describe, expect, it } from "vitest";
import {
  OTHER,
  forwardName,
  groupPorts,
  planForwards,
  projectKey,
  serviceDetail,
  serviceName,
  shortPath,
  tildify,
  worktreeSections,
} from "./portProjects";
import { newTunnel, type DiscoveredPort, type Tunnel } from "./types";

// Invented layout: an "atlas" repository with a linked worktree beside it,
// a "ledger" Compose project, and a few unrelated services.
const ATLAS = "/home/ci/work/atlas";
const SEARCH = "/home/ci/work/atlas-search";
const listener = (value: Partial<DiscoveredPort>): DiscoveredPort => ({
  port: 8000,
  address: "127.0.0.1",
  processName: null,
  pid: null,
  ...value,
});
const inAtlas = (path: string, branch: string, main: boolean) => ({
  projectRoot: ATLAS,
  projectMarker: ".git",
  worktreePath: path,
  worktreeBranch: branch,
  worktreeMain: main,
});
const vite = (port: number, pid: number, checkout: string, main: boolean) =>
  listener({
    port,
    pid,
    user: "ci",
    processName: "node",
    executable: "/usr/bin/node",
    workingDirectory: `${checkout}/app`,
    arguments: ["node", `${checkout}/node_modules/.bin/vite`],
    ...inAtlas(checkout, main ? "main" : "feat/search", main),
  });
const db = listener({
  port: 5432,
  address: "0.0.0.0",
  containerName: "ledger-db-1",
  image: "postgres:16",
  composeProject: "ledger",
  composeService: "db",
  composeWorkingDir: "/srv/ledger/",
  projectRoot: "/srv/ledger",
  projectMarker: "compose",
});
const sshd = listener({
  port: 22,
  pid: 1,
  processName: "sshd",
  executable: "/usr/sbin/sshd",
});
const forward = (port: number, extra: Partial<Tunnel> = {}): Tunnel => ({
  ...newTunnel("server", port),
  ...extra,
});

describe("project keys", () => {
  it("prefer the root discovery found, then Compose's directory or name", () => {
    expect(projectKey(vite(4173, 1, ATLAS, true))).toBe(ATLAS);
    // A Git root wins over a Compose directory inside it.
    expect(projectKey({ ...db, projectRoot: "/srv" })).toBe("/srv");
    expect(projectKey({ ...db, projectRoot: null })).toBe("/srv/ledger");
    expect(
      projectKey({ ...db, projectRoot: null, composeWorkingDir: null }),
    ).toBe("compose:ledger");
    expect(projectKey(sshd)).toBe(OTHER);
  });
});

describe("grouping", () => {
  it("orders named projects first and other services last", () => {
    const groups = groupPorts(
      [sshd, db, vite(4173, 1, ATLAS, true)],
      [],
      "server",
    );
    expect(groups.map((g) => [g.name, g.source])).toEqual([
      ["atlas", "vcs"],
      ["ledger", "compose"],
      ["Other services", "other"],
    ]);
  });

  it("lets a saved Git project name its repository and absorb nested roots", () => {
    const nested = listener({
      port: 9000,
      pid: 9,
      projectRoot: `${ATLAS}/api`,
    });
    const [atlas] = groupPorts(
      [vite(4173, 1, ATLAS, true), nested],
      [],
      "server",
      [{ name: "Atlas", path: `${ATLAS}/` }],
    );
    expect(atlas).toMatchObject({ name: "Atlas", source: "saved", key: ATLAS });
    expect(atlas.ports).toHaveLength(2);
  });

  it("puts every worktree of a repository in one project, main first", () => {
    const [atlas] = groupPorts(
      [vite(4174, 2, SEARCH, false), vite(4173, 1, ATLAS, true)],
      [],
      "server",
    );
    expect(atlas.worktrees.map((w) => [w.branch, w.main])).toEqual([
      ["main", true],
      ["feat/search", false],
    ]);
    expect(
      worktreeSections(atlas.ports).map((s) => [
        s.worktree?.branch,
        s.entries.map((e) => e.port.port),
      ]),
    ).toEqual([
      ["main", [4173]],
      ["feat/search", [4174]],
    ]);
  });

  it("names a bare repository without its .git suffix", () => {
    const [bare] = groupPorts(
      [listener({ pid: 3, projectRoot: "/srv/git/atlas.git" })],
      [],
      "server",
    );
    expect(bare).toMatchObject({ key: "/srv/git/atlas.git", name: "atlas" });
  });

  it("merges one owner's IPv4 and IPv6 listeners, preferring loopback", () => {
    const api = listener({ port: 9000, pid: 7, address: "::" });
    const [group] = groupPorts(
      [api, { ...api, address: "0.0.0.0" }],
      [],
      "server",
    );
    expect(group.ports).toHaveLength(1);
    expect(group.ports[0].addresses).toEqual(["0.0.0.0", "::"]);
  });

  it("keeps saved forwards with their project, and reverse ones apart", () => {
    const reaching = forward(5432);
    const filed = forward(9999, { project: ATLAS });
    const reverse = forward(5432, { direction: "remote" });
    const elsewhere = { ...forward(5432), serverId: "other-server" };
    const groups = groupPorts(
      [db, vite(4173, 1, ATLAS, true)],
      [reaching, filed, reverse, elsewhere],
      "server",
    );
    const by = (name: string) => groups.find((g) => g.name === name)!;
    expect(by("ledger").ports[0].forwards).toEqual([reaching]);
    expect(by("atlas").forwards).toEqual([filed]);
    // A reverse forward opens a server port; it never reaches one.
    expect(by("Other services").forwards).toEqual([reverse]);
  });
});

describe("forward all", () => {
  it("plans one forward per unforwarded service on the same local port", () => {
    const [atlas] = groupPorts(
      [vite(4173, 1, ATLAS, true), vite(4174, 2, SEARCH, false)],
      [],
      "server",
    );
    expect(
      planForwards(atlas, [], "server").map((t) => [
        t.name,
        t.localPort,
        t.remotePort,
        t.project,
      ]),
    ).toEqual([
      ["Vite", 4173, 4173, ATLAS],
      // A linked worktree's forward carries its branch.
      ["Vite · feat/search", 4174, 4174, ATLAS],
    ]);
  });

  it("moves past local ports saved forwards already claim", () => {
    const [atlas] = groupPorts([vite(4173, 1, ATLAS, true)], [], "server");
    const taken = [forward(1, { localPort: 4173, localPortEnd: 4174 })];
    expect(planForwards(atlas, taken, "server")[0].localPort).toBe(4175);
  });

  it("skips rows already forwarded and can limit itself to some rows", () => {
    const reaching = forward(4173);
    const [atlas] = groupPorts(
      [vite(4173, 1, ATLAS, true), vite(4174, 2, SEARCH, false)],
      [reaching],
      "server",
    );
    expect(planForwards(atlas, [reaching], "server")).toHaveLength(1);
    expect(planForwards(atlas, [reaching], "server", [atlas.ports[0]])).toEqual(
      [],
    );
  });
});

describe("names and details", () => {
  const entry = (value: DiscoveredPort) =>
    groupPorts([value], [], "server")[0].ports[0];

  it("name a service by what it is, with where and how it runs below", () => {
    const app = entry(vite(4173, 1, ATLAS, true));
    expect(serviceName(app)).toBe("Vite");
    expect(serviceDetail(app, ATLAS)).toEqual(["app/", "Node"]);
    expect(forwardName(app)).toBe("Vite");
  });

  it("name a Compose port by service and leave its container to the tooltip", () => {
    const ledger = entry(db);
    expect(serviceName(ledger)).toBe("db");
    expect(serviceDetail(ledger, null)).toEqual([
      "Postgres",
      "Docker",
      "all interfaces",
    ]);
  });

  it("keep the process beside a known service's name, unless only case differs", () => {
    expect(serviceDetail(entry(sshd), null)).toEqual(["sshd"]);
    const postgres = entry(
      listener({
        pid: 5,
        processName: "postgres",
        executable: "/usr/bin/postgres",
      }),
    );
    expect(serviceName(postgres)).toBe("Postgres");
    expect(serviceDetail(postgres, null)).toEqual([]);
  });

  it("prefer discovery's application name over a thread name", () => {
    const thread = entry(
      listener({
        pid: 6,
        processName: "MainThread",
        applicationName: "Vite · site",
        executable: "/usr/bin/node",
      }),
    );
    expect(serviceName(thread)).toBe("Vite · site");
  });

  it("lead with a working directory that no longer exists", () => {
    const orphan = entry(
      listener({
        pid: 8,
        processName: "node",
        executable: "/usr/bin/node",
        workingDirectory: "/tmp/build/preview",
        workingDirectoryDeleted: true,
      }),
    );
    // Titled "node" already, so the runtime is not repeated below it.
    expect(serviceName(orphan)).toBe("node");
    expect(serviceDetail(orphan, null)).toEqual([
      "folder deleted",
      "/tmp/build/preview",
    ]);
  });
});

describe("paths", () => {
  it("abbreviate a home directory only when one user owns everything there", () => {
    expect(tildify("/home/ci/work/atlas", ["ci", "ci"])).toBe("~/work/atlas");
    expect(tildify("/root/site", ["root"])).toBe("~/site");
    expect(tildify("/home/ci/work", ["ci", "deploy"])).toBe("/home/ci/work");
  });

  it("shorten long paths to the trailing folders that identify them", () => {
    expect(
      shortPath("/tmp/ci-1000/session-0123456789abcdef/build/jobs/wt/c3", []),
    ).toBe("…/build/jobs/wt/c3");
    expect(shortPath("/home/ci/work/atlas/.worktrees/search", ["ci"])).toBe(
      "~/work/atlas/.worktrees/search",
    );
    expect(shortPath("/data/pg", [])).toBe("/data/pg");
    expect(shortPath("app/", [])).toBe("app/");
  });
});
