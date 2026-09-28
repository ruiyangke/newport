import { createStore, useSelector } from "@tanstack/react-store";
import { useCallback, type SetStateAction } from "react";
import { useServerScope } from "../query/keys";

// UI preferences survive navigation, but never enter persistent storage.
// Query owns remote data; terminal resources have a separate lifecycle.
export interface WorkspaceState {
  "overview.auto": boolean;
  "overview.historyMinutes": number;
  "overview.filter": string;
  "overview.sort": string;
  "containers.query": string;
  "containers.collapsed": ReadonlySet<string>;
  "services.query": string;
  "services.filter": "all" | "failed" | "active" | "inactive";
  "metrics.interface": string;
  "metrics.samples": boolean;
  "files.path": string;
  "files.input": string;
  "files.home": string;
  "files.query": string;
  "files.hidden": boolean;
  "files.page": number;
  "files.tree": boolean;
}
const defaults: WorkspaceState = {
  "overview.auto": true,
  "overview.historyMinutes": 5,
  "overview.filter": "",
  "overview.sort": "cpu",
  "containers.query": "",
  "containers.collapsed": new Set(),
  "services.query": "",
  "services.filter": "all",
  "metrics.interface": "",
  "metrics.samples": false,
  "files.path": ".",
  "files.input": "",
  "files.home": "",
  "files.query": "",
  "files.hidden": false,
  "files.page": 0,
  "files.tree": true,
};
export const workspaceStore = createStore<Record<string, WorkspaceState>>({});
export function setWorkspaceValue<K extends keyof WorkspaceState>(
  scope: string,
  key: K,
  next: SetStateAction<WorkspaceState[K]>,
) {
  workspaceStore.setState((state) => {
    const page = state[scope] ?? defaults;
    const previous = page[key];
    const value = typeof next === "function" ? next(previous) : next;
    if (Object.is(previous, value)) return state;
    return { ...state, [scope]: { ...page, [key]: value } };
  });
}
export function useWorkspaceState<K extends keyof WorkspaceState>(
  id: string,
  key: K,
) {
  const { connection } = useServerScope(id);
  const value = useSelector(
    workspaceStore,
    (state) => (state[connection] ?? defaults)[key],
  );
  const set = useCallback(
    (next: SetStateAction<WorkspaceState[K]>) => {
      setWorkspaceValue(connection, key, next);
    },
    [connection, key],
  );
  return [value, set] as const;
}
export function retainWorkspaces(valid: Set<string>) {
  workspaceStore.setState((state) => {
    if (Object.keys(state).every((key) => valid.has(key))) return state;
    return Object.fromEntries(
      Object.entries(state).filter(([key]) => valid.has(key)),
    );
  });
}
