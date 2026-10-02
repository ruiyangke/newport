import type { ComponentProps } from "react";
import { cn } from "../lib/utils";

export function StatusDot({ className, ...props }: ComponentProps<"span">) {
  return (
    <span
      data-slot="status-dot"
      aria-hidden="true"
      className={cn(
        "inline-block size-[5px] shrink-0 rounded-[50%] bg-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}
