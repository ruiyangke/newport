import { useCallback, useState } from "react";
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
  const [dismissed, setDismissed] = useState(0);
  const query = useQuery({
    ...snapshotOptions,
    enabled: live && visible,
    refetchInterval: 1000,
    // Background fetch status/timestamps must not redraw the whole application.
    notifyOnChangeProps: ["data", "error", "errorUpdatedAt", "status"],
  });
  const refresh = useCallback(async () => {
    if (!live) return;
    // Surface a read failure in the snapshot banner, not as a failed mutation.
    await refreshQuery(client, snapshotOptions).catch(() => {});
  }, [client, live]);
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
