import type { TerminalSession } from "./session";

// Keep native resources outside reactive snapshots and out of the initial bundle.
export const terminalResources = new Map<string, TerminalSession>();
export function retainTerminals(valid: Set<string>) {
  for (const [key, resource] of terminalResources) {
    if (!valid.has(key)) {
      resource.dispose();
      terminalResources.delete(key);
    }
  }
}
if (import.meta.hot) import.meta.hot.dispose(() => retainTerminals(new Set()));
