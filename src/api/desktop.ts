import type { FileListing, FilePreview, FileProgress } from "../domain/files";
import { invoke } from "@tauri-apps/api/core";
import type { Snapshot, Server, Tunnel, DiscoveredPort } from "../types";
import type { Overview, Service, Log } from "../domain/cockpit";
import type { Container } from "../docker";
import type { SavedSample } from "../metricHistory";

export interface CollectionData {
  overview: Overview;
  services: Service[];
  containers: Container[];
}
export type CollectionSection = keyof CollectionData;
export type TerminalEvent =
  | { type: "ready" }
  | { type: "data"; data: number[] }
  | { type: "exit"; data: number | null }
  | { type: "error"; data: string };
export interface AgentKey {
  source: "system" | "onePassword";
  comment: string;
  fingerprint: string;
  algorithm: string;
}
type Command<Args, Result> = { args: Args; result: Result };
export interface StartupSettings {
  enabled: boolean;
  available: boolean;
}
export interface MetricsCache {
  bytes: number;
  samples: number;
}
interface Commands {
  get_metrics_cache: Command<undefined, MetricsCache>;
  clear_metrics_cache: Command<undefined, MetricsCache>;
  get_startup_settings: Command<undefined, StartupSettings>;
  set_launch_at_login: Command<{ enabled: boolean }, StartupSettings>;
  files_list: Command<
    { id: string; operation: string; path: string },
    FileListing
  >;
  files_preview: Command<
    { id: string; operation: string; path: string },
    FilePreview
  >;
  files_upload: Command<
    { id: string; operation: string; path: string },
    string | null
  >;
  files_download: Command<
    { id: string; operation: string; path: string },
    string | null
  >;
  files_cancel: Command<{ operation: string }, void>;
  files_progress: Command<{ operation: string }, FileProgress | null>;
  snapshot: Command<undefined, Snapshot>;
  test_connection: Command<{ id: string }, void>;
  save_server: Command<{ server: Server; password: string | null }, void>;
  save_tunnel: Command<{ tunnel: Tunnel }, void>;
  delete_server: Command<{ id: string }, void>;
  delete_tunnel: Command<{ id: string }, void>;
  set_tunnel_connected: Command<{ id: string; connected: boolean }, void>;
  open_tunnel: Command<{ id: string }, void>;
  discover_ports: Command<{ id: string }, DiscoveredPort[]>;
  set_clipboard_enabled: Command<{ id: string; enabled: boolean }, void>;
  cockpit_history: Command<{ id: string }, SavedSample[]>;
  cockpit_logs: Command<
    { id: string; source: Log["source"]; target: string },
    string
  >;
  cockpit_project_action: Command<
    {
      id: string;
      project: string;
      action: "start" | "stop" | "restart";
      expectedIds: string[];
    },
    void
  >;
  ssh_agent_keys: Command<undefined, { keys: AgentKey[]; warnings: string[] }>;
  terminal_open: Command<
    { id: string; session: string; cols: number; rows: number },
    void
  >;
  terminal_read: Command<{ session: string }, TerminalEvent | null>;
  terminal_write: Command<{ session: string; data: number[] }, void>;
  terminal_resize: Command<
    { session: string; cols: number; rows: number },
    void
  >;
  terminal_close: Command<{ session: string }, void>;
  get_sidebar_width: Command<undefined, number | null>;
  set_sidebar_width: Command<{ width: number }, void>;
}
/** One boundary for frontend command names, arguments and results. */
export function desktop<K extends keyof Commands>(
  command: K,
  ...args: Commands[K]["args"] extends undefined ? [] : [Commands[K]["args"]]
): Promise<Commands[K]["result"]> {
  return invoke<Commands[K]["result"]>(command, args[0]);
}
export function collect<S extends CollectionSection>(
  id: string,
  section: S,
): Promise<CollectionData[S]> {
  return invoke<CollectionData[S]>("cockpit_collect", { id, section });
}
