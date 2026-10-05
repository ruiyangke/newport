import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { desktop } from "../api/desktop";
import { keys, useServerScope } from "../query/keys";
import { readIPC } from "../query/client";

/**
 * Remote port discovery for one server. It polls while mounted, and a manual
 * discovery is tracked separately so a background refresh never spins the
 * "Discover ports" control or claims the user started a scan.
 */
export function usePorts(id: string) {
  const scope = useServerScope(id);
  const [discovering, setDiscovering] = useState(false);
  const query = useQuery({
    queryKey: keys.ports(scope),
    queryFn: ({ signal }) =>
      readIPC(signal, () => desktop("discover_ports", { id })),
    staleTime: 30_000,
    refetchInterval: 30_000,
    refetchIntervalInBackground: false,
  });
  return {
    data: query.data,
    error: query.error,
    isFetching: query.isFetching,
    discovering,
    updated: query.dataUpdatedAt ? new Date(query.dataUpdatedAt) : null,
    discover: () => {
      setDiscovering(true);
      void query
        .refetch({ cancelRefetch: false })
        .finally(() => setDiscovering(false));
    },
  };
}
