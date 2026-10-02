import { StatusDot } from "./StatusDot";
import type { ComponentProps } from "react";
import { cn } from "../lib/utils";

export function StatusIndicator({
  tone = "muted",
  className,
  children,
  ...props
}: ComponentProps<"span"> & {
  tone?: "muted" | "success" | "danger" | "warning";
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-[6px] text-[11px]",
        {
          muted: "text-muted-foreground",
          success: "text-(--success)",
          danger: "text-(--danger)",
          warning: "text-(--warning)",
        }[tone],
        className,
      )}
      {...props}
    >
      <StatusDot className="bg-current" />
      {children}
    </span>
  );
}
