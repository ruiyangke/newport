import { StatusIndicator } from "./StatusIndicator";
import type { ConnectionState } from "../types";
export function StatusLabel({ state }: { state: ConnectionState }) {
  return (
    <StatusIndicator
      className="whitespace-nowrap"
      tone={
        state.status === "connected"
          ? "success"
          : state.status === "error"
            ? "danger"
            : state.status === "connecting" || state.status === "reconnecting"
              ? "warning"
              : "muted"
      }
    >
      {state.status === "reconnecting" && state.reconnectAttempt > 0
        ? `Retrying · ${state.reconnectAttempt}/10`
        : state.status[0].toUpperCase() + state.status.slice(1)}
    </StatusIndicator>
  );
}
