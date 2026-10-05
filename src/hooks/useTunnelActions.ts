import {
  useMutation,
  useMutationState,
  useQueryClient,
} from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { desktop } from "../api/desktop";
import { refreshQuery } from "../query/client";
import { snapshotOptions } from "./useSnapshot";
import type { Tunnel } from "../types";

type TunnelAction =
  | { kind: "toggle"; id: string; connected: boolean }
  | { kind: "all"; ids: string[]; connected: boolean; target?: string }
  /** Save new forwards, then connect them along with `connect`. */
  | { kind: "forward"; tunnels: Tunnel[]; connect: string[]; target: string }
  | { kind: "open"; id: string };

export interface TunnelActions {
  /** Tunnel ids with a connect/disconnect in flight. */
  pending: ReadonlySet<string>;
  /**
   * Targets (a project key, or a port's `address:port`) with a batch in
   * flight, so the control that started it can show progress.
   */
  pendingTargets: ReadonlySet<string>;
  /** A batch over several tunnels is in flight. */
  allPending: boolean;
  error: string;
  dismissError: () => void;
  toggle: (id: string, connected: boolean) => void;
  connectAll: (tunnels: Tunnel[], connected: boolean, target?: string) => void;
  /** Save `tunnels` and connect them, plus the saved ids in `connect`. */
  forward: (tunnels: Tunnel[], connect: string[], target: string) => void;
  open: (id: string) => void;
}

/**
 * Tunnel actions for one server, shared through the mutation cache: a running
 * action survives page navigation, and a remounted page cannot start a
 * duplicate. One server writes at a time per tunnel; different tunnels may
 * still move independently.
 */
export function useTunnelActions(serverId: string): TunnelActions {
  const client = useQueryClient();
  const mutationKey = useMemo(
    () => ["tunnel-actions", serverId] as const,
    [serverId],
  );
  const [dismissed, setDismissed] = useState(0);
  const mutation = useMutation({
    mutationKey,
    // Keep the last outcome observable across navigation, as actions do.
    gcTime: 5 * 60_000,
    mutationFn: async (action: TunnelAction) => {
      if (action.kind === "open")
        return desktop("open_tunnel", { id: action.id });
      if (action.kind === "toggle")
        return desktop("set_tunnel_connected", {
          id: action.id,
          connected: action.connected,
        });
      if (action.kind === "forward") {
        // Save everything first: a rejected forward leaves nothing half-up.
        for (const tunnel of action.tunnels)
          await desktop("save_tunnel", { tunnel });
        void refreshQuery(client, snapshotOptions).catch(() => {});
        for (const id of [
          ...action.connect,
          ...action.tunnels.map((t) => t.id),
        ])
          await desktop("set_tunnel_connected", { id, connected: true });
        return;
      }
      for (const id of action.ids)
        await desktop("set_tunnel_connected", {
          id,
          connected: action.connected,
        });
    },
    // A successful action changes the snapshot; a failed one may have too.
    onSettled: () => {
      void refreshQuery(client, snapshotOptions).catch(() => {});
    },
  });
  const entries = useMutationState({
    filters: { mutationKey, exact: false },
    select: (entry) => entry.state,
  });
  const pending = new Set<string>();
  const pendingTargets = new Set<string>();
  let allPending = false;
  // Only the most recent settled action may report; a later success clears an
  // earlier failure the same way the previous global banner did.
  let settled: (typeof entries)[number] | null = null;
  for (const entry of entries) {
    const action = entry.variables as TunnelAction | undefined;
    if (entry.status === "pending") {
      if (action?.kind === "toggle") pending.add(action.id);
      else if (action?.kind === "all" || action?.kind === "forward") {
        const ids =
          action.kind === "all"
            ? action.ids
            : [...action.connect, ...action.tunnels.map((t) => t.id)];
        for (const id of ids) pending.add(id);
        if (action.target) pendingTargets.add(action.target);
        else allPending = true;
      }
    } else if (!settled || entry.submittedAt > settled.submittedAt) {
      settled = entry;
    }
  }
  const failure =
    settled?.status === "error" && settled.error
      ? { at: settled.submittedAt, message: String(settled.error) }
      : null;
  const busy = pending.size > 0 || allPending || pendingTargets.size > 0;
  const running = (match: (action: TunnelAction) => boolean) =>
    client
      .getMutationCache()
      .findAll({ mutationKey, exact: false })
      .some((entry) => {
        const action = entry.state.variables as TunnelAction | undefined;
        return entry.state.status === "pending" && !!action && match(action);
      });
  const batching = (target?: string) => (action: TunnelAction) =>
    (action.kind === "all" || action.kind === "forward") &&
    action.target === target;
  return {
    pending,
    pendingTargets,
    allPending,
    error: busy || !failure || failure.at <= dismissed ? "" : failure.message,
    dismissError: () => setDismissed(failure?.at ?? 0),
    toggle: (id, connected) => {
      if (running((action) => action.kind === "toggle" && action.id === id))
        return;
      mutation.mutate({ kind: "toggle", id, connected });
    },
    connectAll: (tunnels, connected, target) => {
      if (running(batching(target))) return;
      mutation.mutate({
        kind: "all",
        ids: tunnels.map((tunnel) => tunnel.id),
        connected,
        target,
      });
    },
    forward: (tunnels, connect, target) => {
      if (running(batching(target))) return;
      mutation.mutate({ kind: "forward", tunnels, connect, target });
    },
    open: (id) => {
      if (running((action) => action.kind === "open" && action.id === id))
        return;
      mutation.mutate({ kind: "open", id });
    },
  };
}
