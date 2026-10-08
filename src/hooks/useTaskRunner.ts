import { isCancelledError } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { toast } from "sonner";

/**
 * Serial per-key task runner for App-level desktop actions.
 *
 * One key runs at a time (`inFlight`); the `pending` set mirrors it for
 * rendering. Errors stay in this channel only: snapshot failures surface
 * through `useSnapshot`, never here.
 */
export function useTaskRunner(refresh: () => Promise<void>) {
  const inFlight = useRef(new Set<string>());
  const [pending, setPending] = useState<Set<string>>(new Set());
  const [actionError, setActionError] = useState("");
  const act = async (
    key: string,
    task: () => Promise<unknown>,
    message?: string,
  ) => {
    if (inFlight.current.has(key)) return;
    inFlight.current.add(key);
    setPending((p) => new Set(p).add(key));
    setActionError("");
    try {
      try {
        await task();
      } finally {
        await refresh();
      }
      if (message)
        toast.success(message, { id: "ssh-verified", duration: 3500 });
    } catch (e) {
      if (!isCancelledError(e)) setActionError(String(e));
    } finally {
      inFlight.current.delete(key);
      setPending((p) => {
        const next = new Set(p);
        next.delete(key);
        return next;
      });
    }
  };
  return {
    act,
    pending,
    actionError,
    clearActionError: () => setActionError(""),
  };
}
