import { isValidatedPageAppend } from "../git/uniquePages";
import {
  QueryClient,
  replaceEqualDeep,
  QueryObserver,
  type QueryObserverResult,
  type FetchQueryOptions,
  type QueryKey,
} from "@tanstack/react-query";

export function createQueryClient() {
  return new QueryClient({
    defaultOptions: {
      queries: {
        networkMode: "always",
        structuralSharing: (previous, incoming) =>
          isValidatedPageAppend(previous, incoming)
            ? incoming
            : replaceEqualDeep(previous, incoming),
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

/** Keep a shared page read alive only while a consumer still needs it. */
export async function readSharedQuery<T, K extends QueryKey>(
  client: QueryClient,
  options: FetchQueryOptions<T, Error, T, K>,
  signal: AbortSignal,
): Promise<T> {
  signal.throwIfAborted();
  return new Promise<T>((resolve, reject) => {
    const observer = new QueryObserver<T, Error, T, T, K>(client, options);
    let subscribed = false;
    let unsubscribe = () => {};
    let settled = false;
    const cleanup = () => {
      settled = true;
      signal.removeEventListener("abort", abort);
      unsubscribe();
    };
    const abort = () => {
      cleanup();
      reject(signal.reason);
    };
    const finish = (result: QueryObserverResult<T, Error>) => {
      if (!subscribed || settled || result.isFetching) return;
      if (result.isSuccess) {
        cleanup();
        resolve(result.data);
      } else if (result.isError) {
        cleanup();
        reject(result.error);
      }
    };
    signal.addEventListener("abort", abort, { once: true });
    unsubscribe = observer.subscribe(finish);
    subscribed = true;
    // Cached results may need no notification or request at all.
    if (signal.aborted) abort();
    else finish(observer.getCurrentResult());
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
