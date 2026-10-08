import { useCallback, useLayoutEffect, useRef } from "react";

/**
 * Repairs scroll jumps a state commit causes in engines that adjust the
 * scroller on DOM replacement (WebKit moves `workspace-scroll` when sorted
 * rows are swapped, with no JS scroll call, focus change, or geometry
 * change involved). Capture around the commit trigger; the layout effect
 * restores the exact offset right after the commit, before paint.
 */
export function useScrollGuard<T>(value: T) {
  const saved = useRef<{ scroller: HTMLElement; top: number } | null>(null);
  const capture = useCallback((target: HTMLElement | null) => {
    const scroller = target?.closest<HTMLElement>(
      "[data-slot=workspace-scroll]",
    );
    saved.current =
      scroller && scroller.isConnected
        ? { scroller, top: scroller.scrollTop }
        : null;
  }, []);
  useLayoutEffect(() => {
    const guard = saved.current;
    saved.current = null;
    if (guard?.scroller.isConnected) guard.scroller.scrollTop = guard.top;
  }, [value]);
  return capture;
}
