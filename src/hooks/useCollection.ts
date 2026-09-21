import { useLayoutEffect, useRef, type RefObject } from "react";
import { queryOptions, useQuery, useQueryClient } from "@tanstack/react-query";
import { collect, type CollectionSection } from "../api/desktop";
import {
  captureCollectionScroll,
  restoreCollectionScroll,
} from "../collectionScroll";
import { keys, useServerScope } from "../query/keys";
import { readIPC, refreshQuery } from "../query/client";
import { useVisible } from "../query/visibility";

export function useCollection<S extends CollectionSection>(
  id: string,
  section: S,
  auto = false,
  scope?: RefObject<HTMLElement | null>,
) {
  const server = useServerScope(id);
  const client = useQueryClient();
  const visible = useVisible();
  const scroll = useRef<ReturnType<typeof captureCollectionScroll>>([]);
  const options = queryOptions({
    queryKey: keys.collection(server, section),
    queryFn: async ({ signal }) => {
      try {
        return await readIPC(signal, () => collect(id, section));
      } finally {
        if (!signal.aborted)
          scroll.current = captureCollectionScroll(scope?.current ?? null);
      }
    },
    staleTime: auto ? 10_000 : Infinity,
  });
  const query = useQuery({
    ...options,
    enabled: visible,
    refetchOnMount: "always",
    refetchInterval: auto ? 10_000 : false,
  });
  useLayoutEffect(() => {
    restoreCollectionScroll(scroll.current);
    scroll.current = [];
  }, [query.data, query.error, query.dataUpdatedAt]);
  const sampledAt =
    query.data && "sampledAt" in query.data ? query.data.sampledAt : undefined;
  return {
    data: query.data ?? null,
    error: query.error ? String(query.error) : "",
    busy: query.isFetching,
    updated: query.dataUpdatedAt
      ? new Date(
          typeof sampledAt === "number" ? sampledAt : query.dataUpdatedAt,
        )
      : null,
    refresh: () =>
      refreshQuery(client, options).then(
        () => {},
        () => {},
      ),
  };
}
