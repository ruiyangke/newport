import type { ReactNode } from "react";
import { cn } from "../lib/utils";

export function MetricReading({
  name,
  dashed,
  children,
}: {
  name: string;
  dashed?: boolean;
  children: ReactNode;
}) {
  return (
    <span className="inline-flex items-center gap-[6px]">
      <i
        aria-hidden="true"
        className={cn(
          "w-[10px] border-t-2 border-t-chart-1",
          dashed && "border-(--green) border-dashed",
        )}
      />
      {name}{" "}
      <strong className="text-[14px] font-medium tracking-normal tabular-nums">
        {children}
      </strong>
    </span>
  );
}
