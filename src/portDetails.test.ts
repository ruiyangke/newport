import { describe, expect, it } from "vitest";
import {
  RUNTIME_KINDS,
  portKinds,
  portLabel,
  portName,
  portOwner,
  processDetails,
} from "./portDetails";
import type { DiscoveredPort } from "./types";

// An invented listener; each case overrides only what it is about.
const listener = (value: Partial<DiscoveredPort> = {}): DiscoveredPort => ({
  port: 7000,
  address: "127.0.0.1",
  pid: 40,
  processName: "worker",
  executable: "/opt/acme/bin/worker",
  workingDirectory: "/srv/acme/worker",
  ...value,
});

describe("owner names", () => {
  it("prefers a container, then discovery's application name, then the process", () => {
    const container = listener({ containerName: "queue" });
    expect(portOwner(container)).toBe("queue");
    expect(portLabel(container)).toBe("queue · Docker");
    expect(portLabel(listener({ applicationName: "Vite · site" }))).toBe(
      "Vite · site",
    );
    expect(portLabel(listener())).toBe("worker");
  });

  it("admits when the server did not say who owns a socket", () => {
    const unknown = listener({ processName: null, applicationName: null });
    expect(portOwner(unknown)).toBeNull();
    expect(portLabel(unknown)).toBe("Owner not reported");
  });

  it("names a Compose port by its service", () => {
    expect(
      portName(listener({ containerName: "shop-db-1", composeService: "db" })),
    ).toBe("db");
  });
});

describe("process details", () => {
  it("gives a runtime's project directory, whichever runtime it is", () => {
    for (const executable of [
      "/usr/bin/node",
      "/usr/local/bin/bun",
      "/usr/bin/deno",
      "/usr/bin/python3.11",
      "/usr/bin/ruby",
      "/usr/bin/php",
    ])
      expect(processDetails(listener({ executable }))).toEqual([
        { label: "Project directory", value: "/srv/acme/worker" },
      ]);
  });

  it("gives a custom program its working directory, not a project claim", () => {
    expect(processDetails(listener())).toEqual([
      { label: "Working directory", value: "/srv/acme/worker" },
    ]);
  });

  it("reads modules and applications without merging arguments", () => {
    const [module] = processDetails(
      listener({
        executable: "/usr/bin/python3",
        arguments: ["python3", "-m", "gunicorn", "site:app"],
      }),
    );
    expect(module).toEqual({ label: "Module", value: "gunicorn" });
    const [jar] = processDetails(
      listener({
        executable: "/usr/bin/java",
        arguments: ["java", "-jar", "/opt/acme/report server.jar"],
      }),
    );
    expect(jar).toEqual({
      label: "Application",
      value: "/opt/acme/report server.jar",
    });
    const [dll] = processDetails(
      listener({
        executable: "/usr/bin/dotnet",
        arguments: ["dotnet", "Billing.dll"],
      }),
    );
    expect(dll).toEqual({ label: "Application", value: "Billing.dll" });
  });

  it("names the data and configuration a known service runs with", () => {
    const service = (executable: string, ...args: string[]) =>
      processDetails(
        listener({ executable, arguments: [executable, ...args] }),
      );
    expect(
      service("/usr/lib/postgresql/16/bin/postgres", "-D", "/data/pg"),
    ).toEqual([{ label: "Data directory", value: "/data/pg" }]);
    expect(service("/usr/sbin/nginx", "-c", "/etc/nginx/site.conf")).toEqual([
      { label: "Configuration", value: "/etc/nginx/site.conf" },
    ]);
    // Only the option asked for is read; a password beside it never is.
    expect(
      service("/usr/sbin/mariadbd", "--datadir=/data/maria", "--password=x"),
    ).toEqual([{ label: "Data directory", value: "/data/maria" }]);
  });

  it("stays quiet about homes, system paths, containers and unknowns", () => {
    for (const workingDirectory of [
      "/",
      "/usr/share/x",
      "/var/lib/x",
      "/home/ci",
      null,
    ])
      expect(processDetails(listener({ workingDirectory }))).toEqual([]);
    expect(processDetails(listener({ executable: "/usr/sbin/sshd" }))).toEqual(
      [],
    );
    expect(processDetails(listener({ containerName: "queue" }))).toEqual([]);
  });

  it("never mines inline code, flags or raw commands", () => {
    expect(
      processDetails(
        listener({
          workingDirectory: "/",
          executable: "/usr/bin/python3",
          command: "python3 -c 'print(1)'",
          arguments: ["python3", "-c", "print(1)"],
        }),
      ),
    ).toEqual([]);
    expect(
      processDetails(
        listener({
          executable: "/usr/sbin/nginx",
          workingDirectory: "/",
          arguments: ["nginx", "-c", "-g"],
        }),
      ),
    ).toEqual([]);
  });
});

describe("kind tags", () => {
  const kinds = (value: Partial<DiscoveredPort>) => portKinds(listener(value));

  it("name the framework or service first, then its runtime", () => {
    expect(
      kinds({
        executable: "/usr/bin/node",
        arguments: ["node", "/srv/site/node_modules/.bin/astro", "dev"],
      }),
    ).toEqual(["Astro", "Node"]);
    expect(
      kinds({
        executable: "/usr/bin/python3",
        arguments: ["python3", "-m", "streamlit", "run", "app.py"],
      }),
    ).toEqual(["Streamlit", "Python"]);
    expect(
      kinds({
        executable: "/usr/bin/python3",
        arguments: ["python3", "manage.py", "runserver"],
      }),
    ).toEqual(["Django", "Python"]);
    expect(kinds({ executable: "/usr/bin/redis-server" })).toEqual(["Redis"]);
  });

  it("read a container's image, registry and tag aside", () => {
    expect(
      kinds({ containerName: "q", image: "registry.example/mq/rabbitmq:3" }),
    ).toEqual(["RabbitMQ", "Docker"]);
    expect(
      kinds({ containerName: "c", image: "valkey/valkey-bundle" }),
    ).toEqual(["Valkey", "Docker"]);
  });

  it("never guess from a port number", () => {
    expect(kinds({ port: 5432 })).toEqual([]);
    expect(kinds({ port: 6379, processName: null, executable: null })).toEqual(
      [],
    );
  });

  it("separate how a service runs from what it is", () => {
    expect(RUNTIME_KINDS.has("Node")).toBe(true);
    expect(RUNTIME_KINDS.has("Docker")).toBe(true);
    expect(RUNTIME_KINDS.has("Redis")).toBe(false);
  });
});
