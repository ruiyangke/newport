import type { ComponentProps } from "react";
import { cn } from "../lib/utils";
import { TabsContent } from "./ui/tabs";

type Layout = { mode?: string };
const isPane = (mode?: string) => mode === "files" || mode === "projects";
const isFull = (mode?: string) => isPane(mode) || mode === "terminal";

export function WorkspaceViewport({
  mode,
  className,
  ...props
}: ComponentProps<"div"> & Layout) {
  return (
    <div
      data-slot="workspace-scroll"
      className={cn(
        "min-h-0 flex-1",
        isFull(mode) ? "flex flex-col overflow-hidden" : "overflow-auto",
        isPane(mode) && "min-w-0",
        className,
      )}
      {...props}
    />
  );
}
export function WorkspaceDetail({
  mode,
  className,
  ...props
}: ComponentProps<"div"> & Layout) {
  return (
    <div
      data-slot="detail"
      className={cn(
        isFull(mode)
          ? "m-0 flex min-h-0 min-w-0 w-full max-w-none flex-1 flex-col p-0"
          : "m-auto max-w-[1500px] px-[22px] pt-[16px] pb-[22px] max-[700px]:px-[16px] max-[700px]:pt-[14px] max-[700px]:pb-[20px]",
        isPane(mode) && "overflow-hidden",
        className,
      )}
      {...props}
    />
  );
}
export function WorkspaceContent({
  mode,
  className,
  ...props
}: ComponentProps<typeof TabsContent> & Layout) {
  return (
    <TabsContent
      data-slot="workspace-content"
      className={cn(
        "min-w-0 p-0",
        isFull(mode) && "flex min-h-0 flex-1 flex-col",
        isPane(mode) && "overflow-hidden",
        className,
      )}
      {...props}
    />
  );
}
