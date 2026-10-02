import type { HTMLAttributes } from "react";
import { cn } from "../lib/utils";

export function GitEmptyState({
  as: Tag = "p",
  filtered = false,
  className,
  ...props
}: HTMLAttributes<HTMLElement> & { as?: "p" | "div"; filtered?: boolean }) {
  return (
    <Tag
      data-slot="git-projects-empty"
      className={cn(
        "text-center text-muted-foreground [&_h3]:text-foreground",
        filtered ? "px-[16px] py-[28px]" : "px-[20px] py-[40px]",
        className,
      )}
      {...props}
    />
  );
}
