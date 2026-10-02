import { cn } from "../lib/utils";
import type { Service, Log } from "../domain/cockpit";
import { Button } from "./controls";
import { StatusIndicator } from "./StatusIndicator";

const serviceColumns =
  "grid grid-cols-[minmax(0,1fr)_135px_45px] items-center gap-[16px] px-[16px] max-[760px]:grid-cols-[minmax(0,1fr)_86px_45px] max-[760px]:gap-[10px] max-[760px]:px-[12px]";

export function ServiceColumns() {
  return (
    <div
      aria-hidden="true"
      className={cn(
        serviceColumns,
        "border-b border-border bg-(--native-toolbar) py-[9px] text-[10px] text-muted-foreground",
      )}
    >
      <span>Service</span>
      <span>State</span>
      <span className="text-center">Logs</span>
    </div>
  );
}

export function ServiceRow({
  service,
  onLogs,
}: {
  service: Service;
  onLogs: (log: Log, origin: HTMLElement) => void;
}) {
  return (
    <li
      data-scroll-anchor
      data-slot="services-row"
      className={cn(
        serviceColumns,
        "min-h-[62px] border-b border-border py-[12px] last:border-b-0 focus-within:bg-accent",
      )}
    >
      <div className="min-w-0 wrap-anywhere">
        <strong
          className="line-clamp-2 text-[12px] font-semibold"
          title={service.name}
        >
          {service.name}
        </strong>
        {service.description && (
          <small className="mt-[3px] block text-[11px] leading-[1.5] text-muted-foreground">
            {service.description}
          </small>
        )}
      </div>
      <div className="min-w-0 wrap-anywhere">
        <StatusIndicator
          tone={
            service.active === "failed"
              ? "danger"
              : service.active === "active"
                ? "success"
                : "muted"
          }
        >
          {service.active[0]?.toUpperCase()}
          {service.active.slice(1)}
        </StatusIndicator>
        {service.sub !== service.active && (
          <small className="mt-[3px] block text-[11px] leading-[1.5] text-muted-foreground">
            {service.sub}
          </small>
        )}
        {service.load !== "loaded" && (
          <small className="mt-[3px] block text-[11px] leading-[1.5] text-muted-foreground">
            Load: {service.load}
          </small>
        )}
      </div>
      <Button
        variant="ghost"
        className="px-[8px]"
        aria-label={`View logs for ${service.name}`}
        onClick={(event) =>
          onLogs(
            {
              source: "service",
              target: service.name,
              title: service.name,
            },
            event.currentTarget,
          )
        }
      >
        Logs
      </Button>
    </li>
  );
}
