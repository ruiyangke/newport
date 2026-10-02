import type { ComponentProps } from "react";
import { Tabs as TabsPrimitive } from "radix-ui";
import { cn } from "../lib/utils";
import { Button } from "./controls";

export function SidebarNavigation({
  className,
  ...props
}: ComponentProps<typeof TabsPrimitive.List>) {
  return (
    <TabsPrimitive.List
      data-slot="source-list"
      className={cn(
        "flex h-auto w-full shrink-0 flex-col items-stretch justify-start gap-[2px] rounded-none bg-transparent p-0 text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}
export function SidebarTab({
  className,
  ...props
}: ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      data-slot="sidebar-tab"
      className={cn(
        "group/navigation relative flex h-[32px] min-h-[32px] w-full flex-none items-center justify-start gap-[9px] rounded-[6px] border-0 border-current bg-transparent px-[9px] py-0 text-[13px] leading-[1.428571] font-medium whitespace-nowrap text-foreground transition-all outline-none hover:bg-[color-mix(in_srgb,var(--foreground)_5%,transparent)] focus-visible:outline-1 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50 dark:font-normal dark:data-[state=active]:font-medium data-[state=active]:bg-(--native-selection) data-[state=active]:font-medium data-[state=active]:text-(--native-selection-foreground) data-[state=active]:hover:bg-(--native-selection) [&_svg]:size-[16px] [&_svg]:text-muted-foreground data-[state=active]:[&_svg]:text-primary",
        className,
      )}
      {...props}
    />
  );
}
export function SidebarAction({
  className,
  ...props
}: ComponentProps<typeof Button>) {
  return (
    <Button
      variant="ghost"
      className={cn(
        "h-[28px] justify-start gap-[10px] px-[10px] text-[12px] font-medium text-foreground dark:font-normal dark:text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}
