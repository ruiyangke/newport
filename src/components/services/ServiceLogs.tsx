import { LogToolbar as Toolbar, LogViewer } from "../logs/LogViewer";
import { useWorkspaceState } from "../../state/workspace";
import { useServiceLogs, type LogTarget } from "../../hooks/useServices";
import { priorityTone } from "../../domain/services";

/** The journal's controls; workspace state, so they can sit in a tab bar. */
export function LogToolbar({ id }: { id: string }) {
  const [level, setLevel] = useWorkspaceState(id, "services.logLevel");
  const [live, setLive] = useWorkspaceState(id, "services.live");
  const [find, setFind] = useWorkspaceState(id, "services.logQuery");
  return (
    <Toolbar
      live={live}
      onLive={setLive}
      level={level}
      onLevel={setLevel}
      find={find}
      onFind={setFind}
    />
  );
}

/** A unit's journal, or the whole journal; levels filter on the server. */
export function LogView({
  id,
  target,
  label = "Logs",
}: {
  id: string;
  target: LogTarget;
  label?: string;
}) {
  const [level] = useWorkspaceState(id, "services.logLevel");
  const [live] = useWorkspaceState(id, "services.live");
  const [find] = useWorkspaceState(id, "services.logQuery");
  const priority = level === "all" ? null : level;
  const logs = useServiceLogs(id, target, priority, live);
  const lines = (logs.data?.entries ?? []).map((entry) => {
    const tone = priorityTone(entry.priority);
    return {
      at: entry.at,
      tone: tone === "success" ? "muted" : tone,
      source: entry.identifier
        ? `${entry.identifier}${entry.pid ? `[${entry.pid}]` : ""}`
        : null,
      message: entry.message,
    } as const;
  });
  return (
    <LogViewer
      lines={lines}
      dataset={`${target.scope}:${target.unit ?? "journal"}:${priority ?? "all"}`}
      pending={logs.isPending}
      placeholder={logs.isPlaceholderData}
      error={logs.error}
      live={live}
      find={find}
      label={label}
      reading="Reading the journal"
      empty={
        priority
          ? `No ${priority === "err" ? "errors" : "warnings or errors"} in the latest lines.`
          : "No accessible journal entries."
      }
    />
  );
}
