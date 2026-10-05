import type { ReactNode } from "react";
import { Tabs as TabsPrimitive } from "radix-ui";
import { Skeleton } from "../ui/skeleton";

/**
 * The right pane of a list + detail page: facts, a tab bar whose spare room
 * holds the active tab's controls, and the enter transition.
 */
export function Fact({
  label,
  detail,
  children,
}: {
  label: string;
  /** A quieter second line, such as the exact time behind "in 3h". */
  detail?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="min-w-0">
      <dt className="text-[11px] text-muted-foreground">{label}</dt>
      <dd className="m-0 mt-[1px] truncate text-[12.5px]">{children}</dd>
      {detail && (
        <dd className="m-0 truncate text-[11px] text-muted-foreground">
          {detail}
        </dd>
      )}
    </div>
  );
}

export const enter =
  "animate-in duration-200 fade-in slide-in-from-bottom-1 motion-reduce:animate-none";

export const tabTrigger =
  "-mb-px border-b-2 border-transparent px-[10px] pt-[4px] pb-[7px] text-[12.5px] font-medium text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:rounded-[4px] focus-visible:outline-2 focus-visible:outline-ring data-[state=active]:border-foreground data-[state=active]:text-foreground motion-reduce:transition-none";

/**
 * Tabs that fill the rest of the pane. On a narrow pane the side controls
 * drop below the tabs instead of wrapping inside the bar.
 */
export function DetailTabs<T extends string>({
  value,
  onValueChange,
  tabs,
  side,
  children,
}: {
  value: T;
  onValueChange: (value: T) => void;
  tabs: { value: T; label: string }[];
  side?: ReactNode;
  children: ReactNode;
}) {
  return (
    <TabsPrimitive.Root
      value={value}
      onValueChange={(next) => onValueChange(next as T)}
      className="flex min-h-[360px] flex-1 flex-col"
    >
      <div className="@container/tabs mb-[8px]">
        <div className="flex items-end gap-x-[12px] border-b border-border @max-[600px]/tabs:flex-col @max-[600px]/tabs:items-stretch @max-[600px]/tabs:border-b-0">
          <TabsPrimitive.List
            aria-label="Details"
            className="flex shrink-0 @max-[600px]/tabs:border-b @max-[600px]/tabs:border-border"
          >
            {tabs.map((tab) => (
              <TabsPrimitive.Trigger
                key={tab.value}
                value={tab.value}
                className={tabTrigger}
              >
                {tab.label}
              </TabsPrimitive.Trigger>
            ))}
          </TabsPrimitive.List>
          <div className="ml-auto flex min-w-0 justify-end pb-[6px] @max-[600px]/tabs:ml-0 @max-[600px]/tabs:justify-start @max-[600px]/tabs:pt-[8px] @max-[600px]/tabs:pb-0">
            {side}
          </div>
        </div>
      </div>
      {children}
    </TabsPrimitive.Root>
  );
}
export function DetailTab({
  value,
  children,
}: {
  value: string;
  children: ReactNode;
}) {
  return (
    <TabsPrimitive.Content
      value={value}
      className="flex min-h-0 flex-1 flex-col outline-none"
    >
      {children}
    </TabsPrimitive.Content>
  );
}

/** Content that scrolls inside the space a tab is given. */
export function Scroll({ children }: { children: ReactNode }) {
  return (
    <div className="relative min-h-0 flex-1">
      <div className="absolute inset-0 overflow-auto">{children}</div>
    </div>
  );
}

export function DetailSkeleton() {
  return (
    <div aria-hidden="true" className="flex flex-1 flex-col gap-[14px]">
      <div className="flex flex-col gap-[8px]">
        <Skeleton className="h-[18px] w-[220px] rounded-[4px]" />
        <Skeleton className="h-[10px] w-[160px] rounded-[3px]" />
        <Skeleton className="mt-[4px] h-[11px] w-[260px] rounded-[3px]" />
      </div>
      <div className="flex gap-[6px]">
        <Skeleton className="h-[28px] w-[84px] rounded-[7px]" />
        <Skeleton className="h-[28px] w-[64px] rounded-[7px]" />
      </div>
      <div className="flex gap-[28px]">
        {[60, 44, 52, 70].map((w, i) => (
          <div key={i} className="flex flex-col gap-[5px]">
            <Skeleton className="h-[8px] w-[40px] rounded-[3px]" />
            <Skeleton className="h-[11px] rounded-[3px]" style={{ width: w }} />
          </div>
        ))}
      </div>
      <Skeleton className="min-h-[240px] flex-1 rounded-[8px]" />
    </div>
  );
}

/** The master pane and detail pane, side by side or one at a time. */
export const masterGrid =
  "grid min-h-0 flex-1 grid-cols-[minmax(210px,280px)_minmax(0,1fr)] grid-rows-[minmax(0,1fr)] gap-[20px] @max-[600px]/page:grid-cols-1";
