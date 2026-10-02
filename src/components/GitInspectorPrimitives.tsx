import type { ComponentProps } from "react";
import { cn } from "../lib/utils";
import { Button } from "./controls";

export function GitInspectorToolbar({
  className,
  ...props
}: ComponentProps<"div">) {
  return (
    <div
      data-slot="git-inspector-toolbar"
      className={cn(
        "mb-[10px] flex items-center justify-between gap-[8px]",
        className,
      )}
      {...props}
    />
  );
}
export function GitInspectorSearch({
  className,
  ...props
}: ComponentProps<"div">) {
  return (
    <div
      data-slot="git-inspector-search"
      className={cn(
        "relative mb-[10px] group-data-[full-page=true]/repository:max-w-[360px] [&>svg]:pointer-events-none [&>svg]:absolute [&>svg]:top-[8px] [&>svg]:left-[8px] [&>svg]:text-muted-foreground [&_input]:pl-[28px]",
        className,
      )}
      {...props}
    />
  );
}
export function GitInspectorList({
  className,
  ...props
}: ComponentProps<"ul">) {
  return (
    <ul
      data-slot="git-inspector-list"
      className={cn("-mx-[6px] my-0 list-none p-0", className)}
      {...props}
    />
  );
}
const rowLayout =
  "flex h-auto min-h-[44px] w-full items-center gap-[8px] px-[8px] py-[7px] text-left whitespace-normal";
export function GitInspectorRow({
  className,
  ...props
}: ComponentProps<typeof Button>) {
  return (
    <Button
      data-slot="git-inspector-row"
      variant="ghost"
      className={cn(
        rowLayout,
        "aria-pressed:bg-accent aria-pressed:text-accent-foreground",
        className,
      )}
      {...props}
    />
  );
}
export function GitInspectorSummary({
  className,
  ...props
}: ComponentProps<"summary">) {
  return (
    <summary
      data-slot="git-inspector-row"
      className={cn(
        rowLayout,
        "cursor-pointer list-none rounded-lg hover:bg-accent group-open/worktree:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden",
        className,
      )}
      {...props}
    />
  );
}
export function GitInspectorDetails({
  className,
  ...props
}: ComponentProps<"div">) {
  return (
    <div
      data-slot="git-inspector-details"
      className={cn(
        "pt-[10px] pr-[8px] pb-[14px] pl-[30px] wrap-anywhere [&>strong]:hidden",
        className,
      )}
      {...props}
    />
  );
}
