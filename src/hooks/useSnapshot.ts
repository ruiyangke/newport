import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient, queryOptions } from "@tanstack/react-query";
import { desktop } from "../api/desktop";
import type { Snapshot } from "../types";
import { keys } from "../query/keys";
import { readIPC, refreshQuery } from "../query/client";
import { useVisible } from "../query/visibility";

const empty: Snapshot = {
  config: { servers: [], tunnels: [] },
  runtime: { tunnels: {}, clipboard: {}, health: {} },
  loadError: null,
};
export const snapshotOptions = queryOptions({
  queryKey: keys.snapshot,
  queryFn: ({ signal }) => readIPC(signal, () => desktop("snapshot")),
  staleTime: 0,
});
export function useSnapshot(live: boolean) {
  const client = useQueryClient();
  const visible = useVisible();
  const instance = useRef<string | undefined>(undefined);
  const [dismissed, setDismissed] = useState(0);
  const query = useQuery({
    ...snapshotOptions,
    enabled: live && visible,
    refetchInterval: 30_000,
    refetchOnWindowFocus: "always",
    refetchOnReconnect: "always",
    // Background fetch status/timestamps must not redraw the whole application.
    notifyOnChangeProps: ["data", "error", "errorUpdatedAt", "status"],
  });
  const refresh = useCallback(async () => {
    if (!live) return;
    // Surface a read failure in the snapshot banner, not as a failed mutation.
    await refreshQuery(client, snapshotOptions).catch(() => {});
  }, [client, live]);
  useEffect(() => {
    const next = query.data?.instanceId;
    if (next && instance.current && instance.current !== next) {
      void client.invalidateQueries({
        predicate: (entry) => entry.queryKey[0] !== "snapshot",
      });
    }
    if (next) instance.current = next;
  }, [query.data?.instanceId, client]);
  useEffect(() => {
    if (!live || !visible) return;
    const onFocus = () => {
      void refresh();
    };
    window.addEventListener("focus", onFocus);
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen<{ instanceId: string; revision: number }>(
      "state-changed",
      ({ payload }) => {
        const cached = client.getQueryData<Snapshot>(keys.snapshot);
        if (
          cached?.instanceId !== payload.instanceId ||
          (cached?.revision ?? 0) < payload.revision
        ) {
          void refresh();
        }
      },
    )
      .then((unlisten) => {
        if (disposed) unlisten();
        else {
          stop = unlisten;
          // Subscribe before reading: changes during bootstrap cannot be lost.
          void refresh();
        }
      })
      .catch(() => {
        /* Periodic reads remain available if events fail. */
      });
    return () => {
      window.removeEventListener("focus", onFocus);
      disposed = true;
      stop?.();
    };
  }, [live, visible, client, refresh]);
  return {
    data: query.data ?? empty,
    loaded: !live || !query.isPending,
    error:
      query.error && query.errorUpdatedAt !== dismissed
        ? String(query.error)
        : "",
    refresh,
    dismissError: () => setDismissed(query.errorUpdatedAt),
  };
}
