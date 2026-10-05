import { createContext, useContext, useMemo, type ReactNode } from "react";
import { serverConnectionKey, type Server } from "../types";

export function serverScope(server: Server, revision = 0, instanceId = "") {
  return {
    id: server.id,
    connection: `${serverConnectionKey(server)}:${revision}:${instanceId}`,
    destination: JSON.stringify([
      server.id,
      server.sshHost,
      server.sshPort,
      server.sshUser,
    ]),
  };
}
export type ServerScope = ReturnType<typeof serverScope>;
const Context = createContext<ServerScope | null>(null);
export function ServerScopeProvider({
  server,
  revision = 0,
  instanceId = "",
  children,
}: {
  server: Server;
  revision?: number;
  instanceId?: string;
  children: ReactNode;
}) {
  const { id, connection, destination } = serverScope(
    server,
    revision,
    instanceId,
  );
  const value = useMemo(
    () => ({ id, connection, destination }),
    [id, connection, destination],
  );
  return <Context.Provider value={value}>{children}</Context.Provider>;
}
export function useServerScope(id: string) {
  const scope = useContext(Context);
  if (!scope || scope.id !== id)
    throw new Error("Server data requires its workspace scope");
  return scope;
}
/**
 * The enclosing server's scope, for components that sit inside one server's
 * page but are not told which server it is -- the Git panels, which read the
 * repository the page opened. The nearest provider is that page's own.
 */
export function useCurrentServerScope() {
  const scope = useContext(Context);
  if (!scope) throw new Error("Server data requires its workspace scope");
  return scope;
}
export const keys = {
  snapshot: ["snapshot"] as const,
  startup: ["preferences", "startup"] as const,
  metricsCache: ["cache", "metrics"] as const,
  server: (scope: ServerScope) =>
    ["server", scope.id, scope.connection] as const,
  collection: (scope: ServerScope, section: string) =>
    [...keys.server(scope), "collection", section] as const,
  history: (scope: ServerScope) =>
    ["history", scope.id, scope.destination] as const,
  ports: (scope: ServerScope) => [...keys.server(scope), "ports"] as const,
  services: (scope: ServerScope) =>
    [...keys.server(scope), "services"] as const,
  serviceLogs: (scope: ServerScope, unit: string, priority: string) =>
    [...keys.server(scope), "service-logs", unit, priority] as const,
  unitFile: (scope: ServerScope, unit: string) =>
    [...keys.server(scope), "unit-file", unit] as const,
  containers: (scope: ServerScope) =>
    [...keys.server(scope), "containers"] as const,
  containerInspect: (scope: ServerScope, container: string) =>
    [...keys.server(scope), "container-inspect", container] as const,
  /** `target` is `c:<id>` for a container or `p:<project>` for a project. */
  containerLogs: (scope: ServerScope, target: string) =>
    [...keys.server(scope), "container-logs", target] as const,
  dockerDisk: (scope: ServerScope) =>
    [...keys.server(scope), "docker-disk"] as const,
  composeProject: (scope: ServerScope, project: string) =>
    [...keys.server(scope), "compose-project", project] as const,
  composeJobs: (scope: ServerScope) =>
    [...keys.server(scope), "compose-jobs"] as const,
  composeJob: (scope: ServerScope, job: string) =>
    [...keys.server(scope), "compose-job", job] as const,
  files: (scope: ServerScope, path: string) =>
    [...keys.server(scope), "files", path] as const,
};
