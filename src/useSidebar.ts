import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import {
  legacySidebarWidth,
  loadSidebarWidth,
  saveSidebarWidth,
} from "./preferences";

export function useSidebar() {
  const [width, setWidth] = useState(legacySidebarWidth);
  const [hidden, setHidden] = useState(false);
  const [viewport, setViewport] = useState(window.innerWidth);
  const [changed, setChanged] = useState(false);
  const edited = useRef(false);
  const maximum = Math.max(176, Math.min(300, viewport - 440));
  const actual = Math.max(176, Math.min(maximum, width));
  useEffect(() => {
    let active = true;
    void loadSidebarWidth()
      .then((saved) => {
        if (active && !edited.current) setWidth(saved);
      })
      .catch(() => {
        if (active) toast.error("Could not load sidebar preferences.");
      });
    return () => {
      active = false;
    };
  }, []);
  useEffect(() => {
    const resize = () => setViewport(window.innerWidth);
    window.addEventListener("resize", resize);
    return () => window.removeEventListener("resize", resize);
  }, []);
  useEffect(() => {
    if (!changed) return;
    const timer = window.setTimeout(() => {
      void saveSidebarWidth(width).catch(() =>
        toast.error("Could not save sidebar width."),
      );
    }, 200);
    return () => window.clearTimeout(timer);
  }, [width, changed]);
  return {
    width: actual,
    hidden,
    toggle: () => setHidden((v) => !v),
    resize: (next: number) => {
      edited.current = true;
      setChanged(true);
      setWidth(Math.max(176, Math.min(maximum, next)));
    },
    maximum,
  };
}
