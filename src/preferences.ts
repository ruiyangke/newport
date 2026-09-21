import { desktop } from "./api/desktop";
import { isTauri } from "@tauri-apps/api/core";

export function legacySidebarWidth(): number {
  try {
    const value = Number(localStorage.getItem("sidebar-width"));
    return validWidth(value) ? value : 204;
  } catch {
    return 204;
  }
}

function validWidth(value: unknown): value is number {
  return (
    typeof value === "number" &&
    Number.isFinite(value) &&
    value >= 176 &&
    value <= 300
  );
}

// Hydration, migration and resize writes share one queue to preserve user edits.
let operations: Promise<unknown> = Promise.resolve();
function serial<T>(operation: () => Promise<T>): Promise<T> {
  const result = operations.catch(() => {}).then(operation);
  operations = result;
  return result;
}
async function writeWidth(width: number): Promise<void> {
  if (isTauri()) {
    await desktop("set_sidebar_width", { width });
    try {
      localStorage.removeItem("sidebar-width");
    } catch {
      /* Store has the preference. */
    }
  } else {
    localStorage.setItem("sidebar-width", String(width));
  }
}
export function saveSidebarWidth(width: number): Promise<void> {
  return serial(() => writeWidth(width));
}

export function loadSidebarWidth(): Promise<number> {
  return serial(async () => {
    if (!isTauri()) return legacySidebarWidth();
    const width = await desktop("get_sidebar_width");
    if (validWidth(width)) return width;
    const migrated = legacySidebarWidth();
    await writeWidth(migrated);
    return migrated;
  });
}
