import type { TerminalSession } from "./session";

export type TerminalTab = { id: string; name: string; custom?: string };
export type TerminalLayout = {
  tabs: TerminalTab[];
  active: string;
  /** Monotonic numbering, so closing a tab never renames the remaining ones. */
  next: number;
};

// Keep native resources outside reactive snapshots and out of the initial bundle.
export const terminalResources = new Map<
  string,
  Map<string, TerminalSession>
>();
const terminalLayouts = new Map<string, TerminalLayout>();

export function getTerminalLayout(connection: string): TerminalLayout {
  let layout = terminalLayouts.get(connection);
  if (!layout) {
    const id = crypto.randomUUID();
    layout = { tabs: [{ id, name: "Terminal 1" }], active: id, next: 2 };
    terminalLayouts.set(connection, layout);
  }
  return layout;
}
export function saveTerminalLayout(connection: string, layout: TerminalLayout) {
  terminalLayouts.set(connection, layout);
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
export function retainTerminals(valid: Set<string>) {
  for (const [connection, sessions] of terminalResources) {
    if (!valid.has(connection)) {
      for (const session of sessions.values()) session.dispose();
      terminalResources.delete(connection);
    }
  }
  for (const connection of terminalLayouts.keys()) {
    if (!valid.has(connection)) terminalLayouts.delete(connection);
  }
}
if (import.meta.hot) import.meta.hot.dispose(() => retainTerminals(new Set()));
