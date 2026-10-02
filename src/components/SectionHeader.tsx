import type { ComponentProps } from "react";
import { cn } from "../lib/utils";

export function SectionHeader({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="section-heading"
      className={cn(
        "mb-[12px] flex items-center justify-between gap-[12px] max-[850px]:flex-wrap",
        className,
      )}
      {...props}
    />
  );
}

export function SectionActions({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="section-actions"
      className={cn("flex flex-wrap items-center gap-[7px]", className)}
      {...props}
    />
  );
}

export function CollectionCount({ className, ...props }: ComponentProps<"p">) {
  return (
    <p
      data-slot="collection-count"
      className={cn("my-[8px] text-[11px] text-muted-foreground", className)}
      {...props}
    />
  );
}
