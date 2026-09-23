import type { DiscoveredPort } from "./types";
type Detail = { label: string; value: string };
const systemRoots = [
  "/usr",
  "/bin",
  "/sbin",
  "/etc",
  "/proc",
  "/sys",
  "/dev",
  "/run",
  "/nix/store",
  "/var/lib",
  "/var/log",
];
function workspace(directory?: string | null): string | null {
  const value = directory?.replace(/\/+$/, "");
  if (!value?.startsWith("/") || /^\/(?:root|home(?:\/[^/]+)?)$/.test(value))
    return null;
  if (
    systemRoots.some((root) => value === root || value.startsWith(`${root}/`))
  )
    return null;
  return value;
}

// Curated context, not a command-line dump. Never infer identity from MainThread,
// the owner, or port number. Preserve argv boundaries supplied by /proc.
export function processDetails(port: DiscoveredPort): Detail[] {
  if (port.containerName) return [];
  const exe =
    port.executable
      ?.replace(/ \(deleted\)$/, "")
      .split("/")
      .pop() ??
    port.processName ??
    "";
  const argv = (port.arguments ?? []).slice(1);
  const details: Detail[] = [];
  const add = (label: string, value?: string | null) => {
    if (
      value &&
      !value.startsWith("-") &&
      !value.includes("\n") &&
      !value.includes("\r")
    )
      details.push({ label, value });
  };
  const option = (flag: string) => {
    const end = argv.indexOf("--");
    const options = end < 0 ? argv : argv.slice(0, end);
    const index = options.indexOf(flag);
    return index >= 0
      ? options[index + 1]
      : options
          .find((arg) => arg.startsWith(`${flag}=`))
          ?.slice(flag.length + 1);
  };
  const project = workspace(port.workingDirectory);
  const isPython = /^python(?:\d+(?:\.\d+)*)?$/.test(exe);
  const isRuntime =
    isPython ||
    [
      "node",
      "nodejs",
      "bun",
      "deno",
      "ruby",
      "php",
      "java",
      "dotnet",
      "uvicorn",
      "gunicorn",
    ].includes(exe);
  // Known services get their operational context, even when paths are system-owned.
  switch (exe) {
    case "postgres":
      add("Data directory", option("-D"));
      break;
    case "mysqld":
    case "mariadbd":
      add("Data directory", option("--datadir"));
      add("Configuration", option("--defaults-file"));
      break;
    case "nginx":
      add("Configuration", option("-c"));
      break;
    case "sshd":
      add("Configuration", option("-f"));
      break;
    case "redis-server":
      if (argv[0]?.endsWith(".conf")) add("Configuration", argv[0]);
      break;
    case "caddy":
      add("Configuration", option("--config"));
      break;
    default:
      if (isPython) {
        const module = option("-m");
        if (module && /^[\w.-]+$/.test(module)) add("Module", module);
        else if (argv[0]?.endsWith(".py")) add("Script", argv[0]);
      } else if (exe === "java") add("Application", option("-jar"));
      else if (exe === "dotnet" && argv[0]?.endsWith(".dll"))
        add("Application", argv[0]);
      else if (
        ["ruby", "php"].includes(exe) &&
        /\.(rb|php)$/.test(argv[0] ?? "")
      )
        add("Script", argv[0]);
      if (project && exe && exe !== "MainThread")
        add(isRuntime ? "Project directory" : "Working directory", project);
  }
  return details.slice(0, 2);
}
