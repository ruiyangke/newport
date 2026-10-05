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

// What to call a listening port on screen and prefill as a forward name.
// Containers are named first because their process name is often just the
// runtime, but a container's application name is unknown to discovery.
export function portOwner(port: DiscoveredPort): string | null {
  return port.containerName || port.applicationName || port.processName || null;
}
/** A port's name within its project: a Compose service reads best. */
export function portName(port: DiscoveredPort): string | null {
  return port.composeService || portOwner(port);
}
export function portLabel(port: DiscoveredPort): string {
  return port.containerName
    ? `${port.containerName} · Docker`
    : (portOwner(port) ?? "Owner not reported");
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

const services: Record<string, string> = {
  postgres: "Postgres",
  postgresql: "Postgres",
  postgis: "Postgres",
  timescaledb: "Postgres",
  mysqld: "MySQL",
  mysql: "MySQL",
  mariadbd: "MariaDB",
  mariadb: "MariaDB",
  "redis-server": "Redis",
  redis: "Redis",
  "redis-stack": "Redis",
  "valkey-server": "Valkey",
  valkey: "Valkey",
  mongod: "MongoDB",
  mongo: "MongoDB",
  memcached: "Memcached",
  nginx: "nginx",
  caddy: "Caddy",
  httpd: "Apache",
  apache2: "Apache",
  traefik: "Traefik",
  haproxy: "HAProxy",
  envoy: "Envoy",
  sshd: "SSH",
  "sshd-session": "SSH",
  elasticsearch: "Elasticsearch",
  opensearch: "OpenSearch",
  "clickhouse-server": "ClickHouse",
  clickhouse: "ClickHouse",
  rabbitmq: "RabbitMQ",
  "nats-server": "NATS",
  nats: "NATS",
  kafka: "Kafka",
  minio: "MinIO",
  etcd: "etcd",
  consul: "Consul",
  vault: "Vault",
  grafana: "Grafana",
  "grafana-server": "Grafana",
  prometheus: "Prometheus",
  ollama: "Ollama",
  "code-server": "code-server",
  cloudflared: "Cloudflare Tunnel",
  tailscaled: "Tailscale",
  dnsmasq: "DNS",
  "systemd-resolve": "DNS",
  "systemd-resolved": "DNS",
  "docker-proxy": "Docker",
};
const runtimes: Record<string, string> = {
  node: "Node",
  nodejs: "Node",
  bun: "Bun",
  deno: "Deno",
  ruby: "Ruby",
  php: "PHP",
  "php-fpm": "PHP",
  java: "Java",
  dotnet: ".NET",
  "beam.smp": "Erlang",
};
// Matched against an argv entry's basename, or a `python -m` module.
const frameworks: Record<string, string> = {
  vite: "Vite",
  "vite.js": "Vite",
  next: "Next.js",
  "next-server": "Next.js",
  nuxt: "Nuxt",
  nuxi: "Nuxt",
  astro: "Astro",
  remix: "Remix",
  "react-router": "React Router",
  "webpack-dev-server": "webpack",
  webpack: "webpack",
  "react-scripts": "Create React App",
  storybook: "Storybook",
  ng: "Angular",
  uvicorn: "Uvicorn",
  gunicorn: "Gunicorn",
  hypercorn: "Hypercorn",
  flask: "Flask",
  fastapi: "FastAPI",
  streamlit: "Streamlit",
  jupyter: "Jupyter",
  "jupyter-lab": "Jupyter",
  "jupyter-notebook": "Jupyter",
  "http.server": "HTTP server",
  rails: "Rails",
  puma: "Puma",
  unicorn: "Unicorn",
  artisan: "Laravel",
};

/** Tags that say how a service runs rather than what it is. */
export const RUNTIME_KINDS = new Set([
  ...Object.values(runtimes),
  "Python",
  "Docker",
]);

/**
 * Short, observed kind tags for a listening port: the service or framework it
 * runs, then its runtime. Tags come only from the executable, argv, or a
 * container image -- never from the port number, which proves nothing.
 */
export function portKinds(port: DiscoveredPort): string[] {
  const tags: string[] = [];
  const add = (tag?: string) => {
    if (tag && !tags.includes(tag)) tags.push(tag);
  };
  if (port.containerName) {
    const image = port.image?.split("@")[0].split(":")[0].split("/").pop();
    if (image) add(services[image] ?? services[image.replace(/-[^-]*$/, "")]);
    add("Docker");
    return tags.slice(0, 3);
  }
  const exe = (
    port.executable
      ?.replace(/ \(deleted\)$/, "")
      .split("/")
      .pop() ??
    port.processName ??
    ""
  ).toLowerCase();
  const argv = (port.arguments ?? []).slice(1);
  add(services[exe]);
  const python = /^python(?:\d+(?:\.\d+)*)?$/.test(exe);
  if (python) {
    const module = argv[argv.indexOf("-m") + 1];
    if (argv.includes("-m") && module) add(frameworks[module]);
  }
  for (const arg of argv) {
    if (arg.startsWith("-")) continue;
    const base = arg.split("/").pop() ?? "";
    if (base === "manage.py" && argv.includes("runserver")) add("Django");
    add(frameworks[base] ?? frameworks[base.replace(/\.(?:c|m)?js$/, "")]);
  }
  add(frameworks[exe]);
  add(python ? "Python" : runtimes[exe]);
  return tags.slice(0, 3);
}
