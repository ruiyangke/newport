import {
  QueryClient,
  type FetchQueryOptions,
  type QueryKey,
} from "@tanstack/react-query";

export function createQueryClient() {
  return new QueryClient({
    defaultOptions: {
      queries: {
        networkMode: "always",
        retry: false,
        staleTime: 10_000,
        gcTime: 5 * 60_000,
        refetchOnWindowFocus: false,
        refetchOnReconnect: false,
      },
      mutations: { networkMode: "always", retry: false, gcTime: 0 },
    },
  });
}

// Consuming the signal makes abandoned query completions inert. Plain IPC is
// not abortable; file operations additionally forward cancellation to Rust.
export async function readIPC<T>(
  signal: AbortSignal,
  read: () => Promise<T>,
): Promise<T> {
  signal.throwIfAborted();
  const result = await read();
  signal.throwIfAborted();
  return result;
}

// Invalidation after a write must also supersede an initial in-flight read
// (which refetch's default cancelRefetch behavior does not always cancel).
export async function refreshQuery<T, K extends QueryKey>(
  client: QueryClient,
  options: FetchQueryOptions<T, Error, T, K>,
) {
  await client.cancelQueries({ queryKey: options.queryKey, exact: true });
  return client.fetchQuery({ ...options, staleTime: 0 });
}
