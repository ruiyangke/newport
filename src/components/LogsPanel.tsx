import { SectionHeader, SectionActions } from "./SectionHeader";
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
    <section
      data-slot="log-view"
      className="mt-[27px] [&_h2]:min-w-0 [&_h2]:wrap-anywhere"
    >
      <SectionHeader className="flex-wrap items-start">
        <h2>{log.title}</h2>
        <SectionActions>
          <RefreshButton
            busy={busy}
            onClick={() => void refreshQuery(client, options).catch(() => {})}
            logs
          />
          <Button
            variant="ghost"
            size="icon"

            aria-label="Close logs"
            onClick={close}
          >
            <X size={16} />
          </Button>
        </SectionActions>
      </SectionHeader>
      <p className="text-[12px] leading-[1.65] text-muted-foreground">
        {log.source === "compose"
          ? "Last 100 lines per container · up to 20 containers"
          : "Last 200 lines"}
      </p>
      {error && (
        <p
          data-slot="cockpit-error"
          className="text-[12px] leading-[1.6] text-(--red) wrap-anywhere"
          role="alert"
        >
          {error}
        </p>
      )}
      <pre
        data-slot="command-output"
        className="my-[16px] max-h-[480px] overflow-auto rounded-[5px] border border-border bg-(--native-toolbar) p-[12px] font-[ui-monospace,SFMono-Regular,Menlo,monospace] text-[12px] leading-[1.8] whitespace-pre-wrap wrap-anywhere tab-4"
        tabIndex={0}
        aria-label="Log output"
      >
        {text || (busy ? "Reading logs…" : "No accessible log entries.")}
      </pre>
    </section>
  );
}
