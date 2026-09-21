import { createContext, useContext, useMemo, type ReactNode } from "react";
import { serverConnectionKey, type Server } from "../types";

export function serverScope(server: Server, revision = 0) {
  return {
    id: server.id,
    connection: `${serverConnectionKey(server)}:${revision}`,
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
  children,
}: {
  server: Server;
  revision?: number;
  children: ReactNode;
}) {
  const { id, connection, destination } = serverScope(server, revision);
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
  logs: (scope: ServerScope, source: string, target: string) =>
    [...keys.server(scope), "logs", source, target] as const,
  files: (scope: ServerScope, path: string) =>
    [...keys.server(scope), "files", path] as const,
};
