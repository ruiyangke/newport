import { describe, expect, it } from "vitest";
import {
  availableVerbs,
  duration,
  failureReason,
  groupServices,
  isOwned,
  isProblem,
  listNote,
  priorityTone,
  stateOf,
  unitKey,
  type ServiceUnit,
} from "./services";

const unit = (name: string, extra: Partial<ServiceUnit> = {}): ServiceUnit => ({
  name,
  scope: "system",
  description: "",
  load: "loaded",
  active: "active",
  sub: "running",
  enabled: "enabled",
  mainPid: 100,
  since: 0,
  memory: null,
  cpu: null,
  restarts: 0,
  result: "success",
  exitCode: null,
  exitKind: null,
  fragment: `/lib/systemd/system/${name}`,
  dropIns: [],
  triggeredBy: [],
  user: null,
  ...extra,
});
const failed = (extra: Partial<ServiceUnit> = {}) =>
  unit("atlas-api.service", {
    active: "failed",
    sub: "failed",
    result: "exit-code",
    exitCode: 1,
    exitKind: "exited",
    ...extra,
  });

describe("grouping", () => {
  it("puts problems first, then the person's services, then plumbing", () => {
    const groups = groupServices([
      unit("systemd-journald.service"),
      unit("nginx.service"),
      unit("atlas-worker.service", {
        active: "activating",
        sub: "auto-restart",
      }),
      failed(),
      unit("getty@tty1.service"),
      unit("dev-tunnel.service", { scope: "user", enabled: "disabled" }),
      unit("certbot.service", {
        active: "inactive",
        sub: "dead",
        enabled: "static",
      }),
      unit("backup.service", {
        active: "inactive",
        sub: "dead",
        enabled: "disabled",
        fragment: "/etc/systemd/system/backup.service",
      }),
    ]);
    const names = (list: ServiceUnit[]) => list.map((u) => u.name);
    expect(names(groups.problems)).toEqual([
      "atlas-api.service",
      "atlas-worker.service",
    ]);
    // Administrator-installed and user units count as theirs even when idle.
    expect(names(groups.owned)).toEqual([
      "backup.service",
      "dev-tunnel.service",
      "nginx.service",
    ]);
    expect(names(groups.system)).toEqual([
      "certbot.service",
      "getty@tty1.service",
      "systemd-journald.service",
    ]);
  });

  it("treats a unit that starts after restarting as a problem", () => {
    expect(
      isProblem(unit("a.service", { active: "activating", restarts: 2 })),
    ).toBe(true);
    expect(
      isProblem(unit("a.service", { active: "activating", restarts: 0 })),
    ).toBe(false);
  });

  it("keeps OS plumbing out of the person's list even when running", () => {
    expect(isOwned(unit("ssh.service"))).toBe(false);
    expect(isOwned(unit("systemd-resolved.service"))).toBe(false);
    expect(isOwned(unit("redis-server.service"))).toBe(true);
  });

  it("tells same-named user and system units apart", () => {
    expect(unitKey(unit("a.service"))).not.toBe(
      unitKey(unit("a.service", { scope: "user" })),
    );
  });
});

describe("state in words", () => {
  it("names the state a person reads, and how loud it is", () => {
    expect(stateOf(failed())).toEqual({ label: "Failed", tone: "danger" });
    expect(
      stateOf(unit("a", { active: "activating", sub: "auto-restart" })),
    ).toEqual({
      label: "Restarting",
      tone: "warning",
    });
    expect(stateOf(unit("a", { sub: "exited" })).label).toBe("Completed");
    expect(stateOf(unit("a"))).toEqual({ label: "Running", tone: "success" });
    expect(stateOf(unit("a", { active: "inactive", sub: "dead" })).label).toBe(
      "Stopped",
    );
  });

  it("explains why a service stopped", () => {
    expect(failureReason(failed())).toBe("Exited with exit code 1");
    expect(failureReason(failed({ result: "oom-kill" }))).toBe(
      "Killed for running out of memory",
    );
    expect(
      failureReason(
        failed({ result: "signal", exitKind: "killed", exitCode: 9 }),
      ),
    ).toBe("Stopped: killed by signal 9");
    expect(
      failureReason(
        failed({ result: "core-dump", exitKind: "dumped", exitCode: 11 }),
      ),
    ).toBe("Crashed with a core dump (killed by signal 11)");
    expect(failureReason(failed({ result: "start-limit-hit" }))).toBe(
      "Restarted too often; systemd gave up",
    );
    expect(failureReason(unit("a"))).toBeNull();
  });

  it("summarises each row in a few characters", () => {
    expect(listNote(failed(), 0)).toBe("exit 1");
    expect(
      listNote(
        unit("a", { active: "activating", sub: "auto-restart", restarts: 14 }),
        0,
      ),
    ).toBe("restart ×14");
    expect(
      listNote(unit("a", { since: 0 }), 2 * 86400_000 + 3 * 3600_000),
    ).toBe("2d 3h");
    expect(listNote(unit("a", { active: "inactive", sub: "dead" }), 0)).toBe(
      "stopped",
    );
  });

  it("formats durations at the two largest units", () => {
    expect(duration(42_000)).toBe("42s");
    expect(duration(90 * 60_000)).toBe("1h 30m");
    expect(duration(3 * 3600_000)).toBe("3h");
    expect(duration(-5)).toBe("0s");
  });

  it("reads journal priorities as errors, warnings or information", () => {
    expect(priorityTone(3)).toBe("danger");
    expect(priorityTone(4)).toBe("warning");
    expect(priorityTone(6)).toBe("muted");
    expect(priorityTone(null)).toBe("muted");
  });
});

describe("actions", () => {
  it("offers what makes sense in the current state", () => {
    expect(availableVerbs(unit("a"))).toEqual({
      start: false,
      stop: true,
      restart: true,
      reload: true,
      enable: false,
      disable: true,
    });
    expect(availableVerbs(failed({ enabled: "disabled" }))).toMatchObject({
      start: true,
      stop: false,
      reload: false,
      enable: true,
    });
    // A restart loop can be stopped even though the unit is not running.
    expect(
      availableVerbs(unit("a", { active: "activating", sub: "auto-restart" }))
        .stop,
    ).toBe(true);
  });

  it("never offers boot settings for static or masked units", () => {
    for (const enabled of ["static", "masked", null]) {
      const verbs = availableVerbs(unit("a", { enabled }));
      expect(verbs.enable || verbs.disable).toBe(false);
    }
  });
});
