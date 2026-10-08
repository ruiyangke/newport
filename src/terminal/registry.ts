import { registerRetain } from "../state/scoped";
import type { TerminalSession } from "./session";

export type TerminalTab = { id: string; name: string; custom?: string };
export type TerminalLayout = {
  tabs: TerminalTab[];
  active: string;
  /** Monotonic numbering, so closing a tab never renames the remaining ones. */
  next: number;
};

// Keep native resources outside reactive snapshots and out of the initial bundle.
// Sessions live under the ephemeral connection; tab layouts are UI state and
// live under the stable server id, so tabs survive reconnects.
export const terminalResources = new Map<
  string,
  Map<string, TerminalSession>
>();
const terminalLayouts = new Map<string, TerminalLayout>();

export function getTerminalLayout(serverId: string): TerminalLayout {
  let layout = terminalLayouts.get(serverId);
  if (!layout) {
    const id = crypto.randomUUID();
    layout = { tabs: [{ id, name: "Terminal 1" }], active: id, next: 2 };
    terminalLayouts.set(serverId, layout);
  }
  // Copy on the boundary: registry state and React state must never alias.
  // Callers own the returned object and must save it back via
  // `saveTerminalLayout` (which copies again); mutating a returned layout
  // in place updates nothing. Layouts are read on mount and kept in React
  // state afterwards -- there is no subscription.
  return cloneLayout(layout);
}
export function saveTerminalLayout(serverId: string, layout: TerminalLayout) {
  terminalLayouts.set(serverId, cloneLayout(layout));
}
function cloneLayout(layout: TerminalLayout): TerminalLayout {
  return { ...layout, tabs: layout.tabs.map((tab) => ({ ...tab })) };
}
/** After closing a tab, activate its right neighbour, else the left one. */
export function tabAfterClose(
  tabs: TerminalTab[],
  closing: string,
  active: string,
): string {
  if (active !== closing) return active;
  const index = tabs.findIndex((tab) => tab.id === closing);
  const remaining = tabs.filter((tab) => tab.id !== closing);
  return remaining[Math.min(index, remaining.length - 1)]?.id ?? "";
}
export function peekTerminalSession(connection: string, tabId: string) {
  return terminalResources.get(connection)?.get(tabId);
}
export function dropTerminalSession(connection: string, tabId: string) {
  const sessions = terminalResources.get(connection);
  sessions?.get(tabId)?.dispose();
  sessions?.delete(tabId);
  if (sessions?.size === 0) terminalResources.delete(connection);
}
/** Native sessions, keyed by ephemeral connection. */
export function retainTerminalSessions(valid: Set<string>) {
  for (const [connection, sessions] of terminalResources) {
    if (!valid.has(connection)) {
      for (const session of sessions.values()) session.dispose();
      terminalResources.delete(connection);
    }
  }
}
/** Tab layouts, keyed by stable server id. */
export function retainTerminalLayouts(valid: Set<string>) {
  for (const serverId of terminalLayouts.keys()) {
    if (!valid.has(serverId)) terminalLayouts.delete(serverId);
  }
}
/**
 * Legacy single-set entry for tests and full cleanup; production retention
 * runs through `retainAll`, which supplies each tier its own key space.
 */
export function retainTerminals(valid: Set<string>) {
  retainTerminalSessions(valid);
  retainTerminalLayouts(valid);
}
registerRetain(({ servers, connections }) => {
  retainTerminalSessions(connections);
  retainTerminalLayouts(servers);
});
if (import.meta.hot) import.meta.hot.dispose(() => retainTerminals(new Set()));
