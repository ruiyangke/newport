import { useQuery, useQueryClient, queryOptions } from "@tanstack/react-query";
import { keys, useServerScope } from "../query/keys";
import { readIPC, refreshQuery } from "../query/client";
import { desktop } from "../api/desktop";
import { X } from "lucide-react";
import { Button, RefreshButton } from "./controls";
import { type Log } from "../domain/cockpit";

export function LogsPanel({
  id,
  log,
  close,
}: {
  id: string;
  log: Log;
  close: () => void;
}) {
  const scope = useServerScope(id);
  const client = useQueryClient();
  const options = queryOptions({
    queryKey: keys.logs(scope, log.source, log.target),
    queryFn: ({ signal }) =>
      readIPC(signal, () =>
        desktop("cockpit_logs", { id, source: log.source, target: log.target }),
      ),
  });
  const query = useQuery({ ...options, refetchOnMount: "always" });
  const text = query.data ?? "";
  const error = query.error ? String(query.error) : "";
  const busy = query.isFetching;
  return (
    <section className="log-view">
      <div className="section-heading">
        <h2>{log.title}</h2>
        <div className="section-actions">
          <RefreshButton
            busy={busy}
            onClick={() => void refreshQuery(client, options).catch(() => {})}
            logs
          />
          <Button
            variant="ghost"
            size="icon"
            className="icon"
            aria-label="Close logs"
            onClick={close}
          >
            <X size={16} />
          </Button>
        </div>
      </div>
      <p className="muted">
        {log.source === "compose"
          ? "Last 100 lines per container · up to 20 containers"
          : "Last 200 lines"}
      </p>
      {error && (
        <p className="cockpit-error" role="alert">
          {error}
        </p>
      )}
      <pre className="command-output" tabIndex={0} aria-label="Log output">
        {text || (busy ? "Reading logs…" : "No accessible log entries.")}
      </pre>
    </section>
  );
}
