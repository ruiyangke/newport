import { useQuery } from "@tanstack/react-query";
import { desktop } from "../api/desktop";
import { keys, useServerScope } from "../query/keys";
import { readIPC } from "../query/client";

export function usePorts(id: string) {
  const scope = useServerScope(id);
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
    discover: () => query.refetch({ cancelRefetch: false, throwOnError: true }),
  };
}
