import { createScopedStore, registerRetain } from "./scoped";

// UI preferences survive navigation, but never enter persistent storage.
// Query owns remote data; terminal resources have a separate lifecycle.
export interface WorkspaceState {
  "overview.auto": boolean;
  "overview.historyMinutes": number;
  "overview.filter": string;
  "overview.processSort": { id: string; desc: boolean }[];
  "overview.processesAll": boolean;
  "overview.metric": "cpu" | "memory" | "load" | "network";
  "overview.virtualOpen": boolean;
  "containers.query": string;
  /** Compose projects folded in the list, by name. */
  "containers.collapsed": ReadonlySet<string>;
  /** `c:<name>`, `p:<project>`, "disk", or null for the default. */
  "containers.selected": string | null;
  "containers.detailTab": "logs" | "inspect";
  "containers.projectTab": "logs" | "activity" | "file";
  "containers.logLevel": "all" | "warning" | "err";
  "containers.live": boolean;
  "containers.logQuery": string;
  "services.query": string;
  /** The chosen unit (`scope:name`), "journal", or null for the default. */
  "services.selected": string | null;
  "services.systemOpen": boolean;
  "services.logLevel": "all" | "warning" | "err";
  "services.live": boolean;
  "services.logQuery": string;
  "services.detailTab": "logs" | "unit";
  "ports.query": string;
  "ports.filter": "all" | "forwarded" | "attention";
  "ports.collapsed": ReadonlySet<string>;
  "ports.view": "projects" | "ports";
  "ports.sort": { id: string; desc: boolean }[];
  /** Other services' disclosure; null follows whether projects exist. */
  "ports.otherOpen": boolean | null;
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
/** Fresh objects every call: one scope's `Set`s must never reach another's. */
export function getWorkspaceDefaults(): WorkspaceState {
  return {
    "overview.auto": true,
    "overview.historyMinutes": 5,
    "overview.filter": "",
    "overview.processSort": [{ id: "cpu", desc: true }],
    "overview.processesAll": false,
    "overview.metric": "cpu",
    "overview.virtualOpen": false,
    "containers.query": "",
    "containers.collapsed": new Set(),
    "containers.selected": null,
    "containers.detailTab": "logs",
    "containers.projectTab": "logs",
    "containers.logLevel": "all",
    "containers.live": true,
    "containers.logQuery": "",
    "services.query": "",
    "services.selected": null,
    "services.systemOpen": false,
    "services.logLevel": "all",
    "services.live": true,
    "services.logQuery": "",
    "services.detailTab": "logs",
    "ports.query": "",
    "ports.filter": "all",
    "ports.collapsed": new Set(),
    "ports.otherOpen": null,
    "ports.view": "projects",
    "ports.sort": [{ id: "port", desc: false }],
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
}
const scoped = createScopedStore<WorkspaceState>({
  getDefaults: getWorkspaceDefaults,
  // UI choices are keyed by stable server id: filters, selections and
  // layouts survive reconnects. Native handles keep the connection key.
  selectScopeKey: (scope) => scope.id,
});
export const workspaceStore = scoped.store;
export const setWorkspaceValue = scoped.set;
/** Several fields that must change together, as one update. */
export const patchWorkspaceValue = scoped.patch;
export const useWorkspaceState = scoped.useScoped;
export const retainWorkspaces = scoped.retain;

// Unified retention participant (wired via `retainAll` in App.tsx).
registerRetain(({ servers }) => scoped.retain(servers));
