import type { ComponentProps } from "react";
import { cn } from "../lib/utils";

export function MonitoringSection({
  flush,
  className,
  ...props
}: ComponentProps<"section"> & { flush?: boolean }) {
  return (
    <section
      data-slot="cockpit-section"
      className={cn(
        flush ? "m-0 p-0" : "mt-[22px] border-t border-border pt-[20px]",
        className,
      )}
      {...props}
    />
  );
}
