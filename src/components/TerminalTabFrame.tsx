import type { ComponentProps } from "react";
import { cn } from "../lib/utils";

export function TerminalTabFrame({
  className,
  ...props
}: ComponentProps<"div">) {
  return (
    <div
      data-slot="terminal-tab"
      className={cn(
        "group/terminal-tab flex max-w-[180px] shrink-0 items-center rounded-[5px] data-[active]:bg-[color-mix(in_srgb,var(--foreground)_8%,transparent)] data-[editing]:bg-[color-mix(in_srgb,var(--foreground)_8%,transparent)] [&:not([data-active]):hover]:bg-[color-mix(in_srgb,var(--foreground)_6%,transparent)]",
        className,
      )}
      {...props}
    />
  );
}
