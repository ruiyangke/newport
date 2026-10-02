import type { ComponentProps } from "react";
import { cn } from "../lib/utils";

export function GitDiffSelection({
  className,
  ...props
}: ComponentProps<"div">) {
  return (
    <div
      data-slot="git-diff-selection"
      className={cn(
        "flex flex-wrap items-center gap-[12px] border-b border-border bg-(--git-tint) px-[12px] py-[8px] text-[12px]",
        className,
      )}
      {...props}
    />
  );
}
