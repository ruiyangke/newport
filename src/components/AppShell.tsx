import type { ComponentProps } from "react";
import { Tabs } from "./ui/tabs";
import { cn } from "../lib/utils";

export function AppShell({
  collapsed,
  immersive,
  className,
  ...props
}: ComponentProps<typeof Tabs> & { collapsed: boolean; immersive: boolean }) {
  return (
    <Tabs
      data-slot="app-shell"
      className={cn(
        "relative grid h-dvh gap-0 overflow-hidden font-sans",
        collapsed || immersive
          ? "grid-cols-[minmax(0,1fr)]"
          : "grid-cols-[var(--source-width)_minmax(0,1fr)]",
        className,
      )}
      {...props}
    />
  );
}
export function AppSidebar({
  windows,
  hidden,
  className,
  ...props
}: ComponentProps<"aside"> & { windows: boolean }) {
  return (
    <aside
      data-slot="sidebar"
      hidden={hidden}
      className={cn(
        "min-h-0 select-none flex-col overflow-auto border-r border-r-[color-mix(in_srgb,var(--border)_65%,transparent)] px-[10px] text-foreground [--foreground:#191a1e] [--muted-foreground:#494c55] [--native-selection-foreground:#191a1e] [--native-selection:color-mix(in_srgb,var(--foreground)_9%,transparent)] dark:[--foreground:inherit] dark:[--muted-foreground:inherit] dark:[--native-selection-foreground:inherit] dark:[--native-selection:color-mix(in_srgb,var(--foreground)_13%,transparent)] [backdrop-filter:blur(24px)_saturate(110%)] [-webkit-backdrop-filter:blur(24px)_saturate(110%)] in-[.native-vibrancy]:[backdrop-filter:none] in-[.native-vibrancy]:[-webkit-backdrop-filter:none]",
        windows
          ? "bg-sidebar"
          : "bg-[color-mix(in_srgb,var(--sidebar)_76%,transparent)] in-[.native-vibrancy]:bg-[color-mix(in_srgb,var(--sidebar)_30%,transparent)]",
        "[@media(prefers-reduced-transparency:reduce),(prefers-contrast:more)]:bg-sidebar! [@media(prefers-reduced-transparency:reduce),(prefers-contrast:more)]:border-r-border [@media(prefers-reduced-transparency:reduce),(prefers-contrast:more)]:[backdrop-filter:none]! [@media(prefers-reduced-transparency:reduce),(prefers-contrast:more)]:[-webkit-backdrop-filter:none]!",
        hidden ? "hidden" : "flex",
        className,
      )}
      {...props}
    />
  );
}
export function SidebarResizer({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="sidebar-resizer"
      className={cn(
        "absolute top-[52px] bottom-0 left-[calc(var(--source-width)-3px)] z-20 w-[6px] cursor-col-resize touch-none select-none after:absolute after:inset-y-0 after:left-[3px] after:w-px after:content-[''] hover:after:bg-[color-mix(in_srgb,var(--muted-foreground)_30%,transparent)] focus-visible:outline-none focus-visible:after:w-[2px] focus-visible:after:bg-[color-mix(in_srgb,var(--primary)_60%,transparent)]",
        className,
      )}
      {...props}
    />
  );
}
export function WindowToolbar({
  windows,
  collapsed,
  immersive,
  className,
  ...props
}: ComponentProps<"header"> & {
  windows: boolean;
  collapsed: boolean;
  immersive: boolean;
}) {
  return (
    <header
      data-slot="window-toolbar"
      className={cn(
        "flex h-[52px] min-h-[52px] shrink-0 select-none items-center justify-between gap-[10px] border-b border-border bg-(--native-toolbar) py-[7px] pr-[16px] max-[700px]:gap-[7px] max-[700px]:pr-[10px]",
        collapsed && !windows ? "pl-[96px]" : "pl-[12px]",
        windows &&
          immersive &&
          "min-h-[40px] justify-end px-[8px] py-[4px] max-[700px]:pr-[8px]",
        className,
      )}
      {...props}
    />
  );
}
