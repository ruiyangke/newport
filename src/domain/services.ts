/** One systemd service as the agent reports it (see services_snapshot). */
export interface ServiceUnit {
  name: string;
  scope: "system" | "user";
  description: string;
  load: string;
  active: string;
  sub: string;
  /** UnitFileState: enabled, disabled, static, masked, indirect… */
  enabled: string | null;
  mainPid: number | null;
  /** Epoch ms of the last state change. */
  since: number | null;
  memory: number | null;
  /** Percent of one core since the agent's previous reading. */
  cpu: number | null;
  restarts: number | null;
  result: string | null;
  exitCode: number | null;
  exitKind: string | null;
  fragment: string | null;
  dropIns: string[];
  triggeredBy: string[];
  user: string | null;
}
export interface TimerUnit {
  name: string;
  scope: "system" | "user";
  description: string;
  active: string;
  next: number | null;
  last: number | null;
  unit: string | null;
}
/** What system actions run as: root, password-less sudo, sudo with a password, or nothing. */
export type Privilege = "root" | "sudo" | "password" | "none";
export interface ServicesSnapshot {
  services: ServiceUnit[];
  timers: TimerUnit[];
  user: { available: boolean; reason: string | null };
  privilege: Privilege;
}
export interface JournalEntry {
  /** Epoch ms; null when the entry carries no timestamp. */
  at: number | null;
  priority: number | null;
  pid: number | null;
  identifier: string | null;
  message: string;
}
export interface JournalPage {
  entries: JournalEntry[];
  cursor: string | null;
}
export type ServiceVerb =
  "start" | "stop" | "restart" | "reload" | "enable" | "disable";
export type ActionResult =
  { ok: true } | { ok: false; needsPassword: true; command: string };

/** A unit's identity across scopes: a user and a system unit may share a name. */
export const unitKey = (unit: { name: string; scope: string }) =>
  `${unit.scope}:${unit.name}`;
export const shortName = (name: string) =>
  name.replace(/\.(service|timer)$/, "");

/** Something is wrong now, or it keeps falling over. */
export function isProblem(unit: ServiceUnit) {
  return (
    unit.active === "failed" ||
    unit.sub === "auto-restart" ||
    (unit.active === "activating" && (unit.restarts ?? 0) > 0)
  );
}

// Operating-system plumbing: it runs on every server and is rarely what a
// person came to look at. Everything else running, enabled, or installed
// by the administrator counts as theirs.
const PLUMBING =
  /^(systemd-|getty@|serial-getty@|user@|user-runtime-dir@|dbus|polkit|udev|snapd|apt-|unattended-upgrades|networkd-dispatcher|ModemManager|NetworkManager|accounts-daemon|cloud-|multipathd|apparmor|ufw|irqbalance|thermald|fwupd|packagekit|plymouth|e2scrub|fstrim|logrotate|man-db|motd-news|rsyslog|cron|atd|kmod|keyboard-setup|console-setup|setvtrgb|rc-local|ifup@|networking|open-iscsi|iscsid|lvm2|mdmonitor|blk-availability|finalrd|grub|secureboot|ua-|ubuntu-|emergency|rescue|sysstat|dmesg|upower|udisks2|avahi|bluetooth|cups|wpa_supplicant|rtkit|chrony|ntp|ssh|sshd|haveged|qemu-guest-agent|open-vm-tools|vgauth|lxd-agent|google-|walinuxagent|hv-|serial-|modprobe@|dpkg-|pollinate|sysstat|smartd|lm-sensors|kerneloops|whoopsie|apport|systemd)/;

export function isOwned(unit: ServiceUnit) {
  if (unit.scope === "user") return true;
  if (PLUMBING.test(unit.name)) return false;
  // Installed by the administrator rather than shipped with the distro.
  if (unit.fragment && !/^\/(usr\/)?lib\/systemd\//.test(unit.fragment))
    return true;
  return (
    unit.active === "active" ||
    unit.active === "reloading" ||
    unit.enabled === "enabled"
  );
}

export interface ServiceGroups {
  problems: ServiceUnit[];
  owned: ServiceUnit[];
  system: ServiceUnit[];
}
/** Problems first, then the person's own services, then the OS. */
export function groupServices(services: ServiceUnit[]): ServiceGroups {
  const byName = (a: ServiceUnit, b: ServiceUnit) =>
    shortName(a.name).localeCompare(shortName(b.name)) ||
    a.scope.localeCompare(b.scope);
  const sorted = [...services].sort(byName);
  return {
    problems: sorted.filter(isProblem),
    owned: sorted.filter((u) => !isProblem(u) && isOwned(u)),
    system: sorted.filter((u) => !isProblem(u) && !isOwned(u)),
  };
}

export function duration(ms: number) {
  const s = Math.max(0, Math.round(ms / 1000));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d) return h ? `${d}d ${h}h` : `${d}d`;
  if (h) return m ? `${h}h ${m}m` : `${h}h`;
  if (m) return `${m}m`;
  return `${s}s`;
}

export type Tone = "success" | "warning" | "danger" | "muted";
/** The one-word state a person reads, and how loud it is. */
export function stateOf(unit: ServiceUnit): { label: string; tone: Tone } {
  if (unit.active === "failed") return { label: "Failed", tone: "danger" };
  if (unit.sub === "auto-restart")
    return { label: "Restarting", tone: "warning" };
  if (unit.active === "activating")
    return { label: "Starting", tone: "warning" };
  if (unit.active === "deactivating")
    return { label: "Stopping", tone: "warning" };
  if (unit.active === "reloading")
    return { label: "Reloading", tone: "warning" };
  if (unit.active === "active")
    return unit.sub === "exited" || unit.sub === "waiting"
      ? {
          label: unit.sub === "exited" ? "Completed" : "Waiting",
          tone: "muted",
        }
      : { label: "Running", tone: "success" };
  return { label: "Stopped", tone: "muted" };
}

/** Why a failed service stopped, in words, from systemd's result fields. */
export function failureReason(unit: ServiceUnit): string | null {
  if (unit.active !== "failed" && unit.sub !== "auto-restart") return null;
  const code =
    unit.exitKind === "killed" || unit.exitKind === "dumped"
      ? `killed by signal ${unit.exitCode ?? "?"}`
      : unit.exitCode !== null && unit.exitCode !== 0
        ? `exit code ${unit.exitCode}`
        : null;
  switch (unit.result) {
    case "timeout":
      return "Timed out while starting or stopping";
    case "start-limit-hit":
      return "Restarted too often; systemd gave up";
    case "core-dump":
      return `Crashed with a core dump${code ? ` (${code})` : ""}`;
    case "oom-kill":
      return "Killed for running out of memory";
    case "watchdog":
      return "Stopped answering its watchdog";
    case "signal":
      return code ? `Stopped: ${code}` : "Stopped by a signal";
    case "exit-code":
      return code ? `Exited with ${code}` : "Exited with an error";
    default:
      return code ? `Stopped: ${code}` : null;
  }
}

/** The short right-hand note in the list: how long, or what went wrong. */
export function listNote(unit: ServiceUnit, now: number): string {
  if (unit.active === "failed")
    return unit.exitCode ? `exit ${unit.exitCode}` : (unit.result ?? "failed");
  if (unit.sub === "auto-restart")
    return unit.restarts ? `restart ×${unit.restarts}` : "restarting";
  if (unit.active === "active" && unit.since !== null && unit.sub === "running")
    return duration(now - unit.since);
  return stateOf(unit).label.toLowerCase();
}

/** Whether each action makes sense for a service in its current state. */
export function availableVerbs(
  unit: ServiceUnit,
): Record<ServiceVerb, boolean> {
  const running = ["active", "reloading", "activating"].includes(unit.active);
  const bootable = ![
    "static",
    "masked",
    "generated",
    "transient",
    "alias",
  ].includes(unit.enabled ?? "static");
  return {
    start: !running,
    stop: running || unit.sub === "auto-restart",
    restart: true,
    reload: unit.active === "active",
    enable: bootable && unit.enabled !== "enabled",
    disable: bootable && unit.enabled === "enabled",
  };
}

/** Priorities 0-3 read as errors, 4 as warnings; the rest is information. */
export function priorityTone(priority: number | null): Tone {
  if (priority === null) return "muted";
  if (priority <= 3) return "danger";
  if (priority === 4) return "warning";
  return "muted";
}
