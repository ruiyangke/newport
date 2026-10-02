import type { ComponentProps } from "react";
import { cn } from "../lib/utils";

export function ContainerProject({
  className,
  ...props
}: ComponentProps<"div">) {
  return (
    <div
      data-slot="container-project"
      className={cn(
        "relative mb-[16px] overflow-hidden rounded-[6px] border border-border",
        className,
      )}
      {...props}
    />
  );
}
export function ContainerProjectDetails({
  className,
  ...props
}: ComponentProps<"details">) {
  return (
    <details
      data-slot="compose-project"
      className={cn(
        "group/project m-0 overflow-hidden border-0 border-current bg-transparent",
        className,
      )}
      {...props}
    />
  );
}
export function ContainerProjectSummary({
  className,
  ...props
}: ComponentProps<"summary">) {
  return (
    <summary
      className={cn(
        "flex min-h-[65px] items-center gap-[10px] bg-(--native-toolbar) py-[14px] pr-[174px] pl-[14px] before:size-[6px] before:shrink-0 before:rotate-[-45deg] before:border-r-[1.5px] before:border-b-[1.5px] before:border-muted-foreground before:content-[''] group-open/project:before:rotate-45 max-[800px]:flex-wrap max-[800px]:pr-[152px]",
        className,
      )}
      {...props}
    />
  );
}
export function ContainerProjectControls({
  className,
  ...props
}: ComponentProps<"div">) {
  return (
    <div
      data-slot="container-project-controls"
      className={cn(
        "absolute top-[18px] right-[12px] flex items-center gap-[6px]",
        className,
      )}
      {...props}
    />
  );
}
