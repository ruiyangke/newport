import { useEffect, useState } from "react";

/**
 * A shared clock for staleness labels and age displays. One interval plus a
 * focus refresh per caller; components derive "stale" at render time instead
 * of each owning an interval for it.
 */
export function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const tick = () => setNow(Date.now());
    const timer = window.setInterval(tick, intervalMs);
    window.addEventListener("focus", tick);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", tick);
    };
  }, [intervalMs]);
  return now;
}
