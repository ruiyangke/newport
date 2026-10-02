import type { ComponentProps } from "react";
import { cn } from "../lib/utils";

/** The editor grows with its content; this pane owns scrolling in both directions. */
export function GitDiffViewport({
  className,
  ...props
}: ComponentProps<"div">) {
  return (
    <div
      data-slot="git-diff-code"
      className={cn(
        "min-h-0 flex-auto overflow-auto font-mono text-[12px]/[22px] [&_pre]:m-0 [&_pre]:font-[inherit] [&_pre]:whitespace-pre [&_pre]:tab-4",
        className,
      )}
      {...props}
    />
  );
}
