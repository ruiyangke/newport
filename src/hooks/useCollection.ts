import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type RefObject,
} from "react";
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
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const tick = () => setNow(Date.now());
    const timer = window.setInterval(tick, 5_000);
    window.addEventListener("focus", tick);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", tick);
    };
  }, []);
  const client = useQueryClient();
  const visible = useVisible();
  const scroll = useRef<ReturnType<typeof captureCollectionScroll>>([]);
  const read = async (signal: AbortSignal, refresh = false) => {
    try {
      return await readIPC(signal, () => collect(id, section, refresh));
    } finally {
      if (!signal.aborted)
        scroll.current = captureCollectionScroll(scope?.current ?? null);
    }
  };
  const options = queryOptions({
    queryKey: keys.collection(server, section),
    queryFn: ({ signal }) => read(signal),
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
  const collectionError =
    query.data &&
    "collectionError" in query.data &&
    typeof query.data.collectionError === "string"
      ? query.data.collectionError
      : "";
  return {
    data: query.data ?? null,
    stale:
      !!query.error ||
      !!collectionError ||
      (typeof sampledAt === "number" && now - sampledAt > 30_000),
    error: query.error ? String(query.error) : collectionError,
    busy: query.isFetching,
    updated: query.dataUpdatedAt
      ? new Date(
          typeof sampledAt === "number" ? sampledAt : query.dataUpdatedAt,
        )
      : null,
    refresh: () =>
      refreshQuery(client, {
        ...options,
        queryFn: ({ signal }) => read(signal, true),
      }).then(
        () => {},
        () => {},
      ),
  };
}
