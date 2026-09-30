import { createUniquePageAppender } from "../git/uniquePages";
import { gitErrorMessage } from "../git/errors";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient, type QueryKey } from "@tanstack/react-query";
import { type GitPage } from "../domain/gitResponses";

/** A page read never blocks writes, or appends into a refreshed/switched view. */
export function useGitPageLoader<T, M>({
  queryKey,
  page,
  enabled,
  read,
  entryKey,
  prefetch = false,
  merge,
}: {
  queryKey: QueryKey;
  page: GitPage<T, M> | null;
  enabled: boolean;
  read: (cursor: string, signal: AbortSignal) => Promise<GitPage<T, M>>;
  entryKey: (entry: T) => string;
  prefetch?: boolean;
  merge?: (
    previous: GitPage<T, M>,
    incoming: GitPage<T, M>,
    cursor: string,
  ) => GitPage<T, M>;
}) {
  const client = useQueryClient();
  const serializedKey = JSON.stringify(queryKey);
  const key = useMemo<QueryKey>(
    () => JSON.parse(serializedKey),
    [serializedKey],
  );
  const identity = `${serializedKey}:${page?.snapshot ?? ""}`;
  const entryKeyRef = useRef(entryKey);
  entryKeyRef.current = entryKey;
  const appendUnique = useMemo(
    () => createUniquePageAppender<T, M>((entry) => entryKeyRef.current(entry)),
    [],
  );
  useEffect(() => appendUnique.reset(), [identity, appendUnique]);
  const pending = useRef<AbortController | null>(null);
  const readRef = useRef(read);
  readRef.current = read;
  const ahead = useRef<{
    source: GitPage<T, M>;
    version: number;
    cursor: string;
    controller: AbortController;
    promise: Promise<GitPage<T, M> | null>;
  } | null>(null);
  useEffect(() => {
    if (!prefetch || !enabled || !page?.nextCursor) return;
    const cursor = page.nextCursor;
    const controller = new AbortController();
    let unsubscribe = () => {};
    const timer = setTimeout(() => {
      const state = client.getQueryState(key);
      if (
        client.getQueryData(key) !== page ||
        state?.isInvalidated ||
        state?.fetchStatus === "fetching" ||
        pending.current
      )
        return;
      // One page ahead, never automatically append or drain the repository.
      // A speculative failure stays quiet; a user load retries normally.
      const cache = client.getQueryCache();
      const query = cache.find({ queryKey: key, exact: true });
      unsubscribe = cache.subscribe((event) => {
        if (event.query !== query) return;
        const fresh = client.getQueryState(key);
        if (
          fresh?.dataUpdateCount !== state?.dataUpdateCount ||
          fresh?.isInvalidated ||
          fresh?.fetchStatus === "fetching"
        ) {
          controller.abort();
          unsubscribe();
        }
      });
      ahead.current = {
        source: page,
        version: state?.dataUpdateCount ?? 0,
        cursor,
        controller,
        promise: readRef
          .current(cursor, controller.signal)
          .catch(() => null)
          .finally(() => unsubscribe()),
      };
    }, 150);
    return () => {
      clearTimeout(timer);
      controller.abort();
      unsubscribe();
      ahead.current = null;
    };
  }, [client, key, page, enabled, prefetch]);
  const [state, setState] = useState({ identity, loading: false, error: "" });
  useEffect(() => {
    return () => {
      pending.current?.abort();
      pending.current = null;
      setState((state) => ({ ...state, loading: false, error: "" }));
    };
  }, [identity, enabled, page]);

  // A failed read no longer has a pending-request observer. Keep its error
  // tied to this listing until a refresh supersedes it, even when structural
  // sharing preserves the page object and its timestamp.
  useEffect(() => {
    if (!state.error || state.identity !== identity || !enabled) return;
    const cache = client.getQueryCache();
    const query = cache.find({ queryKey: key, exact: true });
    return cache.subscribe((event) => {
      if (event.query !== query || event.type !== "updated") return;
      if (
        event.action.type === "success" ||
        event.action.type === "fetch" ||
        event.action.type === "invalidate"
      ) {
        setState((state) =>
          state.identity === identity ? { ...state, error: "" } : state,
        );
      }
    });
  }, [client, key, identity, enabled, state.error, state.identity]);

  const load = useCallback(async () => {
    const current = client.getQueryData<GitPage<T, M>>(key);
    const before = client.getQueryState(key);
    if (
      !enabled ||
      pending.current ||
      !current?.nextCursor ||
      before?.fetchStatus === "fetching" ||
      before?.isInvalidated
    )
      return;
    const controller = new AbortController();
    pending.current = controller;
    setState({ identity, loading: true, error: "" });
    const isCurrent = () => {
      const after = client.getQueryState(key);
      return (
        !controller.signal.aborted &&
        client.getQueryData(key) === current &&
        before?.dataUpdatedAt === after?.dataUpdatedAt &&
        before?.dataUpdateCount === after?.dataUpdateCount &&
        !after?.isInvalidated &&
        after?.fetchStatus !== "fetching"
      );
    };
    // Refreshes can preserve both object identity and the millisecond timestamp.
    // Observe cache generations so an obsolete read cannot hold up a fresh one.
    const cache = client.getQueryCache();
    const query = cache.find({ queryKey: key, exact: true });
    const unsubscribe = cache.subscribe((event) => {
      if (event.query !== query || isCurrent()) return;
      controller.abort();
      if (pending.current === controller) {
        pending.current = null;
        setState((state) =>
          state.identity === identity
            ? { ...state, loading: false, error: "" }
            : state,
        );
      }
    });
    controller.signal.addEventListener("abort", unsubscribe, { once: true });
    let releasePrepared = () => {};
    try {
      // Forward cancellation to the read transport and also reject late
      // completions from transports that cannot stop an in-flight request.
      const prepared = ahead.current;
      ahead.current = null;
      const usable =
        prepared?.source === current &&
        prepared.cursor === current.nextCursor &&
        prepared.version === before?.dataUpdateCount &&
        !prepared.controller.signal.aborted;
      if (usable) {
        const abort = () => prepared.controller.abort();
        controller.signal.addEventListener("abort", abort, { once: true });
        releasePrepared = () =>
          controller.signal.removeEventListener("abort", abort);
      } else {
        prepared?.controller.abort();
      }
      const prefetched = usable ? await prepared.promise : null;
      if (controller.signal.aborted) return;
      const incoming =
        prefetched ?? (await read(current.nextCursor, controller.signal));
      if (!isCurrent()) return;
      const merged = (merge ?? appendUnique)(
        current,
        incoming,
        current.nextCursor,
      );
      unsubscribe();
      const stored = client.setQueryData<GitPage<T, M>>(key, merged, {
        updatedAt: before?.dataUpdatedAt,
      });
      if (!merge) appendUnique.adopt(merged, stored);
    } catch (error) {
      if (isCurrent())
        setState({
          identity,
          loading: false,
          error: gitErrorMessage(error, "Could not load the next page."),
        });
    } finally {
      releasePrepared();
      unsubscribe();
      controller.signal.removeEventListener("abort", unsubscribe);
      if (pending.current === controller) {
        pending.current = null;
        setState((state) =>
          state.identity === identity ? { ...state, loading: false } : state,
        );
      }
    }
  }, [client, key, enabled, identity, read, merge, appendUnique]);
  return {
    load,
    loading: state.identity === identity && state.loading,
    error: state.identity === identity ? state.error : "",
  };
}
