import {
  queryOptions,
  useMutation,
  useMutationState,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { desktop } from "../api/desktop";
import { keys, useServerScope } from "../query/keys";
import { readIPC } from "../query/client";
import type {
  ComposeAction,
  ComposeJob,
  ContainerLogEntry,
  ContainerLogPage,
  ContainerVerb,
  ProjectVerb,
  PruneTarget,
} from "../domain/containers";

/** How often the open page refreshes containers and their stats. */
export const CONTAINERS_INTERVAL = 5_000;
const LOG_INTERVAL = 2_000;
const MAX_ENTRIES = 2_000;

export function useContainersSnapshot(id: string) {
  const scope = useServerScope(id);
  return useQuery({
    queryKey: keys.containers(scope),
    queryFn: ({ signal }) =>
      readIPC(signal, () => desktop("containers_snapshot", { id })),
    refetchInterval: CONTAINERS_INTERVAL,
    refetchIntervalInBackground: false,
    staleTime: 0,
  });
}

export function useContainerInspect(
  id: string,
  container: string,
  enabled = true,
) {
  const scope = useServerScope(id);
  return useQuery({
    queryKey: keys.containerInspect(scope, container),
    queryFn: ({ signal }) =>
      readIPC(signal, () => desktop("container_inspect", { id, container })),
    staleTime: 15_000,
    enabled,
  });
}

export type ContainerLogTarget =
  | { container: string; project?: undefined }
  | { project: string; container?: undefined };

/**
 * A container's or a project's logs: the last lines first, then, while
 * `live`, only what arrived after the previous cursor, appended.
 */
export function useContainerLogs(
  id: string,
  target: ContainerLogTarget,
  live: boolean,
) {
  const scope = useServerScope(id);
  const client = useQueryClient();
  const options = queryOptions({
    queryKey: keys.containerLogs(
      scope,
      target.container ? `c:${target.container}` : `p:${target.project}`,
    ),
    queryFn: async ({ signal, queryKey }) => {
      const previous = client.getQueryData<ContainerLogPage>(queryKey);
      const page = await readIPC(signal, () =>
        desktop("container_logs", {
          id,
          container: target.container ?? null,
          project: target.project ?? null,
          cursor: previous?.cursor ?? null,
          lines: previous ? null : 300,
        }),
      );
      if (!previous) return page;
      const entries: ContainerLogEntry[] = [
        ...previous.entries,
        ...page.entries,
      ];
      return {
        entries: entries.slice(-MAX_ENTRIES),
        cursor: page.cursor ?? previous.cursor,
      };
    },
    structuralSharing: false,
    staleTime: Infinity,
  });
  return useQuery({
    ...options,
    refetchInterval: live ? LOG_INTERVAL : false,
    refetchIntervalInBackground: false,
  });
}

export function useDockerDisk(id: string) {
  const scope = useServerScope(id);
  return useQuery({
    queryKey: keys.dockerDisk(scope),
    queryFn: ({ signal }) =>
      readIPC(signal, () => desktop("docker_disk", { id })),
    staleTime: 30_000,
  });
}

export type ContainerAction =
  | { kind: "container"; container: string; name: string; verb: ContainerVerb }
  | {
      kind: "project";
      project: string;
      verb: ProjectVerb;
      expectedIds: string[];
    }
  | { kind: "remove"; containers: string[]; names: string[] }
  | { kind: "prune"; what: PruneTarget };

/**
 * Every Docker change goes through one shared mutation: it survives
 * navigation, and the page can show which target is busy. Containers and
 * disk usage refresh afterwards either way; a failure may have changed
 * something too.
 */
export function useContainerActions(id: string) {
  const scope = useServerScope(id);
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["container-action", id],
    gcTime: 60_000,
    mutationFn: async (action: ContainerAction) => {
      switch (action.kind) {
        case "container":
          await desktop("container_action", {
            id,
            container: action.container,
            verb: action.verb,
          });
          return null;
        case "project":
          await desktop("project_action", {
            id,
            project: action.project,
            verb: action.verb,
            expectedIds: action.expectedIds,
          });
          return null;
        case "remove":
          return desktop("containers_remove", {
            id,
            containers: action.containers,
          });
        case "prune":
          return desktop("docker_prune", { id, what: action.what });
      }
    },
    onSettled: () => {
      void client.invalidateQueries({ queryKey: keys.containers(scope) });
      void client.invalidateQueries({ queryKey: keys.dockerDisk(scope) });
    },
  });
}

/** Actions in flight, so any view of a target can say it is busy. */
export function usePendingActions(id: string) {
  return useMutationState({
    filters: { mutationKey: ["container-action", id], status: "pending" },
    select: (mutation) => mutation.state.variables as ContainerAction,
  });
}

/** The project's compose files and services, as the Compose CLI sees them. */
export function useComposeProject(id: string, project: string, enabled = true) {
  const scope = useServerScope(id);
  return useQuery({
    queryKey: keys.composeProject(scope, project),
    queryFn: ({ signal }) =>
      readIPC(signal, () => desktop("compose_project", { id, project })),
    staleTime: 30_000,
    enabled,
  });
}

/**
 * Compose commands running or recently finished on the server. They outlive
 * the page (the agent keeps them), so a reopened page re-attaches; the list
 * polls quickly only while one runs.
 */
export function useComposeJobs(id: string) {
  const scope = useServerScope(id);
  return useQuery({
    queryKey: keys.composeJobs(scope),
    queryFn: ({ signal }) =>
      readIPC(signal, () => desktop("compose_jobs", { id })),
    refetchInterval: (query) =>
      query.state.data?.jobs.some((j) => j.state === "running")
        ? 1_500
        : 15_000,
    refetchIntervalInBackground: false,
  });
}

/** One job with its output so far, appended a page at a time while it runs. */
export function useComposeJob(id: string, job: string | null) {
  const scope = useServerScope(id);
  const client = useQueryClient();
  const key = keys.composeJob(scope, job ?? "");
  return useQuery({
    queryKey: key,
    queryFn: async ({ signal }) => {
      const previous = client.getQueryData<ComposeJob>(key);
      const next = await readIPC(signal, () =>
        desktop("compose_job", {
          id,
          job: job!,
          offset: previous?.offset ?? 0,
        }),
      );
      return previous
        ? {
            ...next,
            // Keep the last 1 MiB, as the agent does.
            output: (previous.output + next.output).slice(-1_048_576),
          }
        : next;
    },
    enabled: !!job,
    structuralSharing: false,
    staleTime: Infinity,
    refetchInterval: (query) =>
      query.state.data && query.state.data.state !== "running" ? false : 1_000,
    refetchIntervalInBackground: false,
  });
}

/** Starts a Compose command; the job list picks it up straight away. */
export function useComposeRun(id: string) {
  const scope = useServerScope(id);
  const client = useQueryClient();
  return useMutation({
    mutationKey: ["compose-run", id],
    mutationFn: (variables: { project: string; action: ComposeAction }) =>
      desktop("compose_run", { id, ...variables }),
    onSettled: () =>
      void client.invalidateQueries({ queryKey: keys.composeJobs(scope) }),
  });
}

export function useComposeCancel(id: string) {
  const scope = useServerScope(id);
  const client = useQueryClient();
  return useMutation({
    mutationFn: (job: string) => desktop("compose_cancel", { id, job }),
    onSettled: () =>
      void client.invalidateQueries({ queryKey: keys.composeJobs(scope) }),
  });
}
