import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import { cn } from "../../lib/utils";
import { Skeleton } from "../ui/skeleton";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../ui/collapsible";

/**
 * The left pane of a list + detail page (Services, Containers): grouped
 * rows with a status dot, one of them chosen, walked with the arrow keys.
 */
export type Tone = "success" | "warning" | "danger" | "muted";

export const dotTone: Record<Tone, string> = {
  success: "bg-(--success)",
  warning: "bg-(--warning) animate-pulse motion-reduce:animate-none",
  danger: "bg-(--danger)",
  muted: "border-[1.5px] border-muted-foreground/40",
};
export const noteTone = (tone?: Tone) =>
  tone === "danger"
    ? "text-(--danger)"
    : tone === "warning"
      ? "text-(--warning)"
      : "text-muted-foreground";

export const rowClass = (selected: boolean) =>
  cn(
    "flex w-full min-w-0 scroll-my-[6px] items-center gap-[8px] rounded-[6px] px-[8px] py-[5px] text-left text-[12.5px] outline-none transition-colors focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring motion-reduce:transition-none",
    selected ? "bg-(--sidebar-accent) text-foreground" : "hover:bg-accent/60",
  );

/** A row's button props: chosen state, and focus that WebKit skips on click. */
export function selectable(selected: boolean, onSelect: () => void) {
  return {
    type: "button" as const,
    "data-master-row": true,
    "aria-current": selected || undefined,
    onClick: (event: { currentTarget: HTMLElement }) => {
      // WebKit doesn't focus buttons on click; the arrow keys start here.
      event.currentTarget.focus({ preventScroll: true });
      onSelect();
    },
  };
}

export function MasterNav({
  label,
  selected,
  children,
}: {
  label: string;
  selected: string | null;
  children: ReactNode;
}) {
  const nav = useRef<HTMLElement>(null);
  // Keep the chosen row in view: after arrow keys, a link from the detail,
  // or the default choice landing below the fold.
  useEffect(() => {
    nav.current
      ?.querySelector<HTMLElement>("[data-master-row][aria-current]")
      ?.scrollIntoView({ block: "nearest" });
  }, [selected]);
  // Up and down move through the rows and show each one, as in Mail.
  const onKeyDown = (event: KeyboardEvent) => {
    const step = {
      ArrowDown: 1,
      ArrowUp: -1,
      Home: -Infinity,
      End: Infinity,
    }[event.key];
    if (!step || event.altKey || event.metaKey || event.ctrlKey) return;
    const rows = Array.from(
      nav.current?.querySelectorAll<HTMLElement>("[data-master-row]") ?? [],
    );
    if (!rows.length) return;
    const at = rows.indexOf(document.activeElement as HTMLElement);
    if (at < 0 && Number.isFinite(step)) return;
    event.preventDefault();
    const next =
      rows[Math.max(0, Math.min(rows.length - 1, at + step))] ?? rows[0];
    next.focus();
    next.click();
  };
  return (
    <nav ref={nav} aria-label={label} className="min-w-0" onKeyDown={onKeyDown}>
      {children}
    </nav>
  );
}

export function MasterGroup({
  title,
  count,
  open,
  onToggle,
  children,
  tone,
}: {
  title: string;
  count: number;
  open: boolean;
  onToggle?: () => void;
  children: ReactNode;
  tone?: "danger";
}) {
  if (!count) return null;
  return (
    <Collapsible asChild open={open} onOpenChange={onToggle}>
      <li role="presentation" className="mt-[10px] first:mt-0">
        <CollapsibleTrigger
          disabled={!onToggle}
          aria-expanded={onToggle ? open : undefined}
          className={cn(
            "flex w-full items-center gap-[5px] rounded-[5px] px-[8px] py-[3px] text-left text-[10.5px] font-semibold tracking-[0.06em] text-muted-foreground uppercase outline-none focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default",
            onToggle && "hover:text-foreground",
            tone === "danger" && "text-(--danger)",
          )}
        >
          {onToggle && (
            <ChevronRight
              size={11}
              aria-hidden="true"
              className={cn(
                "transition-transform duration-200 motion-reduce:transition-none",
                open && "rotate-90",
              )}
            />
          )}
          {title}
          <span className="font-normal tabular-nums">{count}</span>
        </CollapsibleTrigger>
        <CollapsibleContent className="overflow-hidden data-[state=closed]:animate-collapsible-up data-[state=open]:animate-collapsible-down motion-reduce:animate-none">
          <ul
            role="list"
            aria-label={title}
            className="m-0 mt-[2px] list-none p-0"
          >
            {children}
          </ul>
        </CollapsibleContent>
      </li>
    </Collapsible>
  );
}

export function Dot({ tone }: { tone: Tone }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        "size-[7px] shrink-0 rounded-full transition-colors duration-300 motion-reduce:transition-none",
        dotTone[tone],
      )}
    />
  );
}

export function MasterRow({
  selected,
  onSelect,
  tone,
  name,
  badge,
  note,
  noteTone: toneOfNote,
  label,
  className,
}: {
  selected: boolean;
  onSelect: () => void;
  tone: Tone;
  name: ReactNode;
  badge?: ReactNode;
  note?: ReactNode;
  noteTone?: Tone;
  label: string;
  className?: string;
}) {
  return (
    <li>
      <button
        {...selectable(selected, onSelect)}
        aria-label={label}
        className={cn(rowClass(selected), className)}
      >
        <Dot tone={tone} />
        <span className="min-w-0 flex-1 truncate">{name}</span>
        {badge}
        {note !== undefined && (
          <span
            className={cn(
              "shrink-0 text-[11px] tabular-nums",
              noteTone(toneOfNote),
            )}
          >
            {note}
          </span>
        )}
      </button>
    </li>
  );
}

export function Badge({ children }: { children: ReactNode }) {
  return (
    <span className="shrink-0 rounded-[4px] bg-muted px-[4px] text-[10px] text-muted-foreground">
      {children}
    </span>
  );
}

/** The list's shape while the first reading is on its way. */
export function MasterListSkeleton({ label }: { label: string }) {
  const row = (width: number, i: number) => (
    <li key={i} className="flex items-center gap-[8px] px-[8px] py-[7px]">
      <Skeleton className="size-[7px] rounded-full" />
      <Skeleton
        className="h-[10px] rounded-[3px]"
        style={{ width: `${width}%` }}
      />
      <Skeleton className="ml-auto h-[9px] w-[24px] rounded-[3px]" />
    </li>
  );
  return (
    <ul role="list" aria-label={label} className="m-0 list-none p-0">
      <li className="px-[8px] pt-[4px] pb-[6px]">
        <Skeleton className="h-[8px] w-[64px] rounded-[3px]" />
      </li>
      {[46, 38, 58].map(row)}
      <li className="px-[8px] pt-[14px] pb-[6px]">
        <Skeleton className="h-[8px] w-[52px] rounded-[3px]" />
      </li>
      {[52, 34, 62, 44, 40, 56].map((w, i) => row(w, i + 3))}
    </ul>
  );
}
