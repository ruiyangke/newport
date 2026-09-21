import { useEffect } from "react";
import { queryOptions, useQuery, useQueryClient } from "@tanstack/react-query";
import { desktop } from "../api/desktop";
import {
  addSample,
  restoreHistory,
  type MetricInput,
  type MetricSample,
} from "../metricHistory";
import { keys, useServerScope } from "../query/keys";
import { readIPC, refreshQuery } from "../query/client";
import { useVisible } from "../query/visibility";

export function useMetricHistory(
  id: string,
  live: MetricInput | null,
  at: number | undefined,
  auto: boolean,
) {
  const scope = useServerScope(id);
  const client = useQueryClient();
  const visible = useVisible();
  const key = keys.history(scope);
  const options = queryOptions({
    queryKey: [...key, "saved"],
    queryFn: async ({ signal }) => {
      const saved = await readIPC(signal, () =>
        desktop("cockpit_history", { id }),
      );
      // Merge at completion so a late database response cannot erase a newer
      // live sample. The destination key prevents merging different hosts.
      client.setQueryData<MetricSample[]>(key, (previous = []) =>
        restoreHistory(previous, saved),
      );
      return saved;
    },
  });
  const query = useQuery({
    ...options,
    enabled: visible,
    refetchOnMount: "always",
    refetchInterval: auto ? 10_000 : false,
  });
  // The display cache receives live writes; it never owns saved-read status.
  const display = useQuery<MetricSample[]>({ queryKey: key, enabled: false });
  const destination = scope.destination;
  useEffect(() => {
    if (!live || at === undefined) return;
    client.setQueryData<MetricSample[]>(
      ["history", id, destination],
      (previous = []) => addSample(previous, live, at),
    );
  }, [client, id, destination, live, at]);
  return {
    samples: display.data ?? [],
    error: query.error
      ? `Saved history could not be loaded: ${String(query.error)}`
      : "",
    refresh: () =>
      refreshQuery(client, options).then(
        () => {},
        () => {},
      ),
  };
}
