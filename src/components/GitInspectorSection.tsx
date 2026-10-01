import { useEffect, useRef, type ReactNode } from "react";
import { X } from "lucide-react";
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
  const overview = ["Stashes", "Tags", "Worktrees", "Remotes"].includes(title);
  useEffect(() => {
    section.current?.focus({ preventScroll: true });
  }, []);
  return (
    <section
      ref={section}
      tabIndex={-1}
      aria-label={title}
      className="git-inspector-section flex min-h-0 flex-1 flex-col outline-none"
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
        className="git-inspector-content min-h-0 flex-1 overflow-y-auto overscroll-contain pb-4 pt-3"
      >
        {children}
      </div>
    </section>
  );
}
