import { useEffect, useRef } from "react";
import { Button } from "./controls";

/** Prefetch near the bottom of the list; keep a keyboard-accessible fallback. */
export function GitLoadMore({
  cursor,
  loading,
  error,
  disabled = false,
  automatic = true,
  onLoad,
  label,
  endLabel,
}: {
  cursor: string | null;
  loading: boolean;
  error: string;
  disabled?: boolean;
  automatic?: boolean;
  onLoad: () => void;
  label: string;
  endLabel: string;
}) {
  const sentinel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const node = sentinel.current;
    if (
      !node ||
      !cursor ||
      loading ||
      error ||
      disabled ||
      !automatic ||
      typeof IntersectionObserver === "undefined"
    )
      return;
    let root = node.parentElement;
    while (root && !/(auto|scroll)/.test(getComputedStyle(root).overflowY))
      root = root.parentElement;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) onLoad();
      },
      { root, rootMargin: "0px 0px 120px 0px" },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, [cursor, loading, error, disabled, automatic, onLoad]);
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
