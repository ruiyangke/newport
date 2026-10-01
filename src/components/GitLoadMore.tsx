import { useEffect, useRef } from "react";
import { Button } from "./controls";

/** Prefetch near the bottom of the list; keep a keyboard-accessible fallback. */
export function GitLoadMore({
  cursor,
  loading,
  error,
  disabled = false,
  automatic = true,
  scrollOnly = false,
  onLoad,
  label,
  endLabel,
}: {
  cursor: string | null;
  loading: boolean;
  error: string;
  disabled?: boolean;
  automatic?: boolean;
  /** Require a new scroll to the bottom for each page; never drain on mount. */
  scrollOnly?: boolean;
  onLoad: () => void;
  label: string;
  endLabel: string;
}) {
  const sentinel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const node = sentinel.current;
    if (!node || !cursor || loading || error || disabled || !automatic) return;
    let root = node.parentElement;
    while (root && !/(auto|scroll)/.test(getComputedStyle(root).overflowY))
      root = root.parentElement;
    if (scrollOnly) {
      const viewport =
        node.closest<HTMLElement>("[data-git-scroll-root]") ?? root;
      if (!viewport) return;
      let requested = false;
      const onScroll = () => {
        if (
          !requested &&
          viewport.scrollTop > 0 &&
          viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop <=
            2
        ) {
          requested = true;
          onLoad();
        }
      };
      viewport.addEventListener("scroll", onScroll, { passive: true });
      return () => viewport.removeEventListener("scroll", onScroll);
    }
    if (typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) onLoad();
      },
      { root, rootMargin: "0px 0px 120px 0px" },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, [cursor, loading, error, disabled, automatic, scrollOnly, onLoad]);
  return (
    <div
      ref={sentinel}
      className="shrink-0 px-[16px] py-[8px] text-[11px] text-muted-foreground"
    >
      {error && (
        <p role="alert" className="mb-[6px] text-destructive">
          {error}
        </p>
      )}
      {cursor ? (
        <Button
          disabled={disabled || loading}
          onClick={onLoad}
          onKeyDown={(event) => event.stopPropagation()}
        >
          {loading
            ? "Loading…"
            : error
              ? `Retry: ${label.toLowerCase()}`
              : label}
        </Button>
      ) : (
        <span role="status">{endLabel}</span>
      )}
    </div>
  );
}
