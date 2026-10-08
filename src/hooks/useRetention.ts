import { useEffect } from "react";
import { serverScope } from "../query/keys";
import { retainAll } from "../state/scoped";
import type { Server } from "../types";

/**
 * Prunes per-server state once profiles are loaded. UI choices take stable
 * server ids and survive reconnects; native handles take ephemeral
 * connections and reset with them. One call, no per-store list in App.
 */
export function useRetention({
  loaded,
  snapshotError,
  loadError,
  servers,
  connectionRevisions,
  instanceId,
}: {
  loaded: boolean;
  snapshotError: string;
  loadError: string | null;
  servers: readonly Server[];
  connectionRevisions: Record<string, number> | undefined;
  instanceId: string | undefined;
}) {
  useEffect(() => {
    if (!loaded || snapshotError || loadError) return;
    const ids = new Set(servers.map((server) => server.id));
    const connections = new Set(
      servers.map(
        (server) =>
          serverScope(server, connectionRevisions?.[server.id] ?? 0, instanceId ?? "")
            .connection,
      ),
    );
    retainAll({ servers: ids, connections });
  }, [
    loaded,
    snapshotError,
    loadError,
    servers,
    connectionRevisions,
    instanceId,
  ]);
}
