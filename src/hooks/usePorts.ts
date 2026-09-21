import { queryOptions, useQuery, useQueryClient } from "@tanstack/react-query";
import { desktop } from "../api/desktop";
import { keys, useServerScope } from "../query/keys";
import { readIPC, refreshQuery } from "../query/client";
export function usePorts(id: string) {
  const scope = useServerScope(id);
  const client = useQueryClient();
  const options = queryOptions({
    queryKey: keys.ports(scope),
    queryFn: ({ signal }) =>
      readIPC(signal, () => desktop("discover_ports", { id })),
    staleTime: Infinity,
  });
  const query = useQuery({ ...options, enabled: false });
  return { data: query.data, discover: () => refreshQuery(client, options) };
}
