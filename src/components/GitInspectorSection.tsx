import { useEffect, useRef, type ReactNode } from "react";
import { X } from "lucide-react";
import { cn } from "../lib/utils";
import { Button } from "./controls";

/** An in-flow tool surface: no overlay, focus trap, or body scroll lock. */
export function GitInspectorSection({
  title,
  busy,
  onClose,
  children,
  fill = false,
}: {
  title: string;
  busy: boolean;
  onClose: () => void;
  children: ReactNode;
  fill?: boolean;
}) {
  const section = useRef<HTMLElement>(null);
  const overview = ["Stashes", "Tags", "Worktrees", "Remotes", "Repository settings"].includes(
    title,
  );
  useEffect(() => {
    section.current?.focus({ preventScroll: true });
  }, []);
  return (
    <section
      data-slot="git-inspector-section"
      ref={section}
      tabIndex={-1}
      aria-label={title}
      className="flex min-h-0 min-w-0 flex-1 flex-col outline-none"
      data-fill={fill}
      onKeyDown={(event) => {
        if (event.key === "Escape" && !event.defaultPrevented && !busy) {
          event.stopPropagation();
          onClose();
        }
      }}
    >
      <header
        className={
          overview
            ? "absolute top-2 right-3"
            : "flex flex-none items-center justify-between gap-2 px-4 py-3"
        }
      >
        <h3 className={overview ? "sr-only" : "text-[13px] font-medium"}>
          {title}
        </h3>
        <Button
          className="group-data-[full-page=true]/repository:hidden"
          size="icon"
          variant="ghost"
          disabled={busy}
          onClick={onClose}
          aria-label="Close inspector"
        >
          <X size={14} aria-hidden="true" />
        </Button>
      </header>
      <div
        data-git-scroll-root
        data-slot="git-inspector-content"
        className={cn(
          "min-h-0 min-w-0 flex-1 overscroll-contain",
          fill
            ? "flex overflow-hidden p-0"
            : "overflow-y-auto pb-4 pt-3 group-data-[full-page=true]/repository:pt-[16px] [&>div]:px-[16px] [&>form]:px-[16px] group-data-[full-page=true]/repository:[&>div]:mx-auto group-data-[full-page=true]/repository:[&>div]:w-full group-data-[full-page=true]/repository:[&>div]:max-w-[1100px] group-data-[full-page=true]/repository:[&>div]:px-[24px]",
        )}
      >
        {children}
      </div>
    </section>
  );
}
