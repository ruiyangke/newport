import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@tauri-apps/api/core";
import { useEffect, useLayoutEffect, useRef } from "react";
import { workspaceViews } from "./useWorkspaceNavigation";

/**
 * Desktop menu actions and their keyboard equivalents, dispatched through a
 * ref so the Tauri event listener never goes stale. The dispatch closure
 * runs on every render (layout effect without deps), exactly as before.
 */
export function useDesktopShortcuts({
  packagedDesktop,
  dispatch,
}: {
  packagedDesktop: boolean;
  dispatch: (action: string) => void;
}) {
  const desktopAction = useRef<(action: string) => void>(() => {});
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    if (isTauri())
      void listen<string>("workspace-action", (event) =>
        desktopAction.current(event.payload),
      )
        .then((stop) => {
          if (disposed) stop();
          else unlisten = stop;
        })
        .catch(() => {});
    const keydown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || packagedDesktop) return;
      const action =
        event.key === ","
          ? "app-settings"
          : event.key === "n"
            ? "server-new"
            : event.shiftKey && event.key.toLowerCase() === "e"
              ? "server-edit"
              : event.shiftKey && event.key.toLowerCase() === "t"
                ? "server-test"
                : event.shiftKey && event.key.toLowerCase() === "l"
                  ? "sidebar-toggle"
                  : /^[1-8]$/.test(event.key)
                    ? `view-${workspaceViews[Number(event.key) - 1]}`
                    : "";
      if (action) {
        event.preventDefault();
        desktopAction.current(action);
      }
    };
    window.addEventListener("keydown", keydown);
    return () => {
      disposed = true;
      unlisten?.();
      window.removeEventListener("keydown", keydown);
    };
  }, [packagedDesktop]);
  useLayoutEffect(() => {
    desktopAction.current = dispatch;
  });
  return desktopAction;
}
