import { cn } from "../lib/utils";
import type { ReactNode } from "react";

/**
 * The one page header for workspace pages: an 18px title, an optional
 * supporting line, and a right-aligned column of actions with their own muted
 * status line. Every page uses it so titles, spacing, and the
 * "Updated …" + Refresh rhythm cannot drift page by page.
 */
export function PageHeader({
  title,
  subtitle,
  subtitleRole,
  meta,
  actions,
  className = "",
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  /** Set when the supporting line changes on its own, so it is announced. */
  subtitleRole?: "status";
  meta?: ReactNode;
  actions?: ReactNode;
  className?: string;
}) {
  return (
    <header
      data-slot="page-header"
      className={cn(
        "mb-[20px] flex items-start justify-between gap-[20px] max-[800px]:flex-wrap max-[800px]:gap-[12px]",
        className,
      )}
    >
      <div data-slot="page-header-copy" className="min-w-0">
        <h2 className="m-0 text-[18px] font-semibold tracking-normal wrap-anywhere">
          {title}
        </h2>
        {subtitle ? (
          <p
            role={subtitleRole}
            className="mt-[4px] text-[12px] text-muted-foreground wrap-anywhere"
          >
            {subtitle}
          </p>
        ) : null}
      </div>
      {(actions || meta) && (
        <div
          data-slot="page-header-side"
          className="flex shrink-0 flex-col items-end gap-[6px] max-[800px]:items-start"
        >
          {actions ? (
            <div
              data-slot="page-header-actions"
              className="flex flex-wrap items-center justify-end gap-[8px]"
            >
              {actions}
            </div>
          ) : null}
          {meta ? (
            <span
              data-slot="page-header-meta"
              className="text-[11px] text-muted-foreground"
            >
              {meta}
            </span>
          ) : null}
        </div>
      )}
    </header>
  );
}
