import { cn } from "../lib/utils";
import type { Container } from "../docker";
import type { Log } from "../domain/cockpit";
import { Button } from "./controls";
import { StatusIndicator } from "./StatusIndicator";

const containerColumns =
  "grid grid-cols-[minmax(120px,1.2fr)_minmax(130px,1.1fr)_minmax(100px,0.9fr)_45px] items-center gap-[16px] max-[800px]:grid-cols-[minmax(100px,1fr)_minmax(90px,1fr)_45px] max-[800px]:gap-x-[12px] max-[800px]:gap-y-[8px]";

export function ContainerColumns() {
  return (
    <div
      aria-hidden="true"
      className={cn(
        containerColumns,
        "border-y border-border bg-(--native-toolbar) px-[16px] py-[9px] text-[10px] text-muted-foreground",
      )}
    >
      <span>Container</span>
      <span className="max-[800px]:hidden">Image / ports</span>
      <span>Status</span>
      <span />
    </div>
  );
}
export function ContainerRow({
  container: c,
  onLogs,
}: {
  container: Container;
  onLogs: (log: Log, origin: HTMLElement) => void;
}) {
  return (
    <li
      data-scroll-anchor
      data-slot="container-row"
      className={cn(
        containerColumns,
        "min-h-[62px] border-b border-border px-[16px] py-[12px] last:border-b-0 focus-within:bg-accent",
      )}
    >
      <div className="min-w-0 wrap-anywhere">
        <strong className="text-[12px] font-[550]">
          {c.composeService || c.name}
        </strong>
        {c.composeService && (
          <small className="mt-[3px] block text-[11px] text-muted-foreground">
            {c.name}
            {c.composeOneoff?.toLowerCase() === "true" ? " · One-off task" : ""}
          </small>
        )}
      </div>
      <div className="min-w-0 text-[11px] text-muted-foreground max-[800px]:col-start-1 max-[800px]:row-start-2">
        <span className="block wrap-anywhere">{c.image}</span>
        {c.ports && (
          <code
            className="mt-[3px] block text-[10px] wrap-anywhere"
            title="Container ports"
          >
            {c.ports}
          </code>
        )}
      </div>
      <div className="min-w-0 max-[800px]:col-start-2 max-[800px]:row-span-2 max-[800px]:row-start-1">
        <StatusIndicator
          className="leading-[1.5]"
          tone={
            c.status.toLowerCase().includes("unhealthy")
              ? "warning"
              : c.state === "running"
                ? "success"
                : "muted"
          }
        >
          {c.status}
        </StatusIndicator>
      </div>
      <Button
        variant="ghost"
        className="px-[8px] max-[800px]:col-start-3 max-[800px]:row-span-2 max-[800px]:row-start-1"
        aria-label={`View logs for ${c.name}`}
        onClick={(event) =>
          onLogs(
            { source: "container", target: c.id, title: c.name },
            event.currentTarget,
          )
        }
      >
        Logs
      </Button>
    </li>
  );
}
