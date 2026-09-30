import type { ReactNode } from "react";
import { cn } from "cn";
import { Alert, AlertDescription } from "./ui/alert";

/**
 * A notice on the Git pages, built on shadcn `Alert`.
 *
 * Every notice has to say how loudly it speaks, because the primitive's
 * default is `role="alert"`: an assertive live region that interrupts a screen
 * reader mid-sentence. That is right for a failure and wrong for "Showing the
 * first 300 lines". So the tone is required, and maps to the role:
 *   error  -> `alert`  (interrupts; something went wrong)
 *   status -> `status` (polite; a state that changed)
 *   info   -> `note`   (not a live region; read when reached)
 *
 * The look is the design's, not the primitive's. Measured on the prototype, a
 * notice is a flat band -- surface tint, no border, no radius, 12px text -- so
 * the card treatment is removed here. The band's padding, fill and ink come
 * from the unlayered `.git-projects-notice` rule in projects.css, which the
 * utilities could not beat anyway, and which other rules key on (the
 * panel-error alignment, the rule where a notice meets the diff header).
 */
export function GitNotice({
  tone,
  className,
  children,
}: {
  tone: "error" | "status" | "info";
  className?: string;
  children: ReactNode;
}) {
  return (
    <Alert
      role={tone === "error" ? "alert" : tone === "status" ? "status" : "note"}
      className={cn(
        // `static`, `text-start` and a normal gap undo base properties of the
        // primitive's card (it positions an action button this band never
        // has), so the band computes exactly as it did before.
        "git-projects-notice static block w-auto gap-[normal] rounded-none border-0 text-start text-[length:inherit]",
        className,
      )}
    >
      {/* The description's own muted ink, `text-sm` and balanced wrapping
          would each change what the band reads as; it inherits instead. */}
      <AlertDescription className="text-inherit text-[length:inherit] [text-wrap:inherit]">
        {children}
      </AlertDescription>
    </Alert>
  );
}
