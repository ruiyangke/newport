import {
  queryOptions,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { desktop } from "../api/desktop";
import { keys, useServerScope } from "../query/keys";
import { readIPC } from "../query/client";
import type {
  JournalEntry,
  JournalPage,
  ServiceVerb,
} from "../domain/services";

/** How often the open page refreshes services; the agent keeps it cheap. */
export const SERVICES_INTERVAL = 5_000;
const LOG_INTERVAL = 2_000;
/** Entries kept in a live tail before the oldest drop off. */
const MAX_ENTRIES = 2_000;

export function useServicesSnapshot(id: string) {
  const scope = useServerScope(id);
  return useQuery({
    queryKey: keys.services(scope),
    queryFn: ({ signal }) =>
      readIPC(signal, () => desktop("services_snapshot", { id })),
    refetchInterval: SERVICES_INTERVAL,
    refetchIntervalInBackground: false,
    staleTime: 0,
  });
}

export interface LogTarget {
  /** null reads the whole journal for the scope. */
  unit: string | null;
  scope: "system" | "user";
}

/**
 * A unit's journal: the last lines first, then, while `live`, each poll asks
 * only for entries after the previous cursor and appends them, so a tail
 * costs one small request every two seconds.
 */
export function useServiceLogs(
  id: string,
  target: LogTarget,
  priority: "err" | "warning" | null,
  live: boolean,
) {
  const scope = useServerScope(id);
  const client = useQueryClient();
  const options = queryOptions({
    queryKey: keys.serviceLogs(
      scope,
      `${target.scope}:${target.unit ?? "journal"}`,
      priority ?? "all",
    ),
    queryFn: async ({ signal, queryKey }) => {
      const previous = client.getQueryData<JournalPage>(queryKey);
      const page = await readIPC(signal, () =>
        desktop("service_logs", {
          id,
          unit: target.unit,
          scope: target.scope,
          priority,
          cursor: previous?.cursor ?? null,
          lines: previous ? null : 200,
        }),
      );
      if (!previous) return page;
      const entries: JournalEntry[] = [...previous.entries, ...page.entries];
      return {
        entries: entries.slice(-MAX_ENTRIES),
        cursor: page.cursor ?? previous.cursor,
      };
    },
    // Appending is the point; a cached page is never replaced wholesale.
    structuralSharing: false,
    staleTime: Infinity,
  });
  const query = useQuery({
    ...options,
    refetchInterval: live ? LOG_INTERVAL : false,
    refetchIntervalInBackground: false,
    // A new level keeps the same unit's lines on screen until the filtered
    // ones arrive; another unit starts empty rather than borrowing them.
    placeholderData: (previous, previousQuery) =>
      previousQuery?.queryKey.at(-2) === options.queryKey.at(-2)
        ? previous
        : undefined,
  });
  return {
    ...query,
    /** Start again from the latest lines, as after changing the filter. */
    reset: () => client.resetQueries({ queryKey: options.queryKey }),
  };
}

export function useUnitFile(
  id: string,
  unit: string,
  scope: "system" | "user",
  enabled: boolean,
) {
  const serverScope = useServerScope(id);
  return useQuery({
    queryKey: keys.unitFile(serverScope, `${scope}:${unit}`),
    queryFn: ({ signal }) =>
      readIPC(signal, () => desktop("service_unit_file", { id, unit, scope })),
    enabled,
  });
}

export interface ServiceAction {
  unit: string;
  scope: "system" | "user";
  verb: ServiceVerb;
}

/**
 * Start/stop/restart/reload/enable/disable, shared through the mutation
 * cache so a running action survives navigation. The services list refreshes
 * afterwards either way: a failed action may still have changed state.
 */
export function useServiceAction(id: string) {
  const scope = useServerScope(id);
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["service-action", id],
    gcTime: 60_000,
    mutationFn: (action: ServiceAction) =>
      desktop("service_action", { id, ...action }),
    onSettled: () =>
      void client.invalidateQueries({ queryKey: keys.services(scope) }),
  });
}
