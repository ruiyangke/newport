import { expect, it } from "vitest";
import { processDetails } from "./portDetails";
const port = {
  port: 5174,
  address: "0.0.0.0",
  pid: 12,
  processName: "MainThread",
  executable: "/nix/store/node/bin/node",
  workingDirectory: "/home/ruiyang/Projects/demo",
};
it("shows project context for runtimes without classifying by owner", () => {
  for (const executable of [
    "/usr/bin/node",
    "/usr/bin/python3.12",
    "/usr/bin/ruby",
    "/usr/bin/php",
    "/usr/bin/bun",
    "/usr/bin/deno",
  ])
    expect(processDetails({ ...port, executable, user: "root" })).toEqual([
      { label: "Project directory", value: port.workingDirectory },
    ]);
});
it("identifies Python modules and Java/.NET applications with argument boundaries intact", () => {
  expect(
    processDetails({
      ...port,
      executable: "/usr/bin/python3",
      arguments: ["python3", "-m", "uvicorn", "app:app"],
    })[0],
  ).toEqual({ label: "Module", value: "uvicorn" });
  expect(
    processDetails({
      ...port,
      executable: "/usr/bin/java",
      arguments: ["java", "-jar", "/srv/my app.jar"],
    })[0],
  ).toEqual({ label: "Application", value: "/srv/my app.jar" });
  expect(
    processDetails({
      ...port,
      executable: "/usr/bin/dotnet",
      arguments: ["dotnet", "Orders.dll"],
    })[0],
  ).toEqual({ label: "Application", value: "Orders.dll" });
});
it("shows operational context for services rather than hiding all system processes", () => {
  expect(
    processDetails({
      ...port,
      executable: "/usr/bin/postgres",
      arguments: ["postgres", "-D", "/var/lib/postgresql"],
    }),
  ).toEqual([{ label: "Data directory", value: "/var/lib/postgresql" }]);
  expect(
    processDetails({
      ...port,
      executable: "/usr/sbin/nginx",
      arguments: ["nginx", "-c", "/etc/nginx/custom.conf"],
    }),
  ).toEqual([{ label: "Configuration", value: "/etc/nginx/custom.conf" }]);
  expect(
    processDetails({
      ...port,
      executable: "/usr/sbin/mysqld",
      arguments: ["mysqld", "--datadir=/srv/mysql", "--password=secret"],
    }),
  ).toEqual([{ label: "Data directory", value: "/srv/mysql" }]);
});
it("gives custom binaries their working directory without claiming a project", () => {
  expect(
    processDetails({ ...port, executable: "/opt/company/service" }),
  ).toEqual([{ label: "Working directory", value: port.workingDirectory }]);
});
it("omits generic, unavailable, container and default service details", () => {
  for (const workingDirectory of [
    "/",
    "/usr/lib/service",
    "/var/lib/daemon",
    "/home/ruiyang",
    null,
  ])
    expect(processDetails({ ...port, workingDirectory })).toEqual([]);
  expect(processDetails({ ...port, executable: "/usr/sbin/sshd" })).toEqual([]);
  expect(processDetails({ ...port, executable: null })).toEqual([]);
  expect(processDetails({ ...port, containerName: "api" })).toEqual([]);
});
it("does not mine raw commands or inline code for context", () => {
  expect(
    processDetails({
      ...port,
      workingDirectory: "/",
      executable: "/usr/bin/python3",
      command: "python3 -m secret",
      arguments: ["python3", "-c", "print('secret')"],
    }),
  ).toEqual([]);
  expect(
    processDetails({
      ...port,
      executable: "/usr/sbin/nginx",
      arguments: ["nginx", "-c", "-g"],
    }),
  ).toEqual([]);
});
