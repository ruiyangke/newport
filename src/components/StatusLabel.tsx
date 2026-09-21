import type { ConnectionState } from "../types";
export function StatusLabel({ state }: { state: ConnectionState }) {
  return (
    <span className={`status ${state.status}`}>
      <span className="status-dot" />
      {state.status === "reconnecting" && state.reconnectAttempt > 0
        ? `Retrying · ${state.reconnectAttempt}/10`
        : state.status[0].toUpperCase() + state.status.slice(1)}
    </span>
  );
}
