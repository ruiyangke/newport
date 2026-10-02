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
 * The component owns the flat notice band and its error treatment.
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
      data-slot="git-projects-notice"
      role={tone === "error" ? "alert" : tone === "status" ? "status" : "note"}
      className={cn(
        // `static`, `text-start` and a normal gap undo base properties of the
        // primitive's card (it positions an action button this band never
        // has), so the band computes exactly as it did before.
        "static w-auto rounded-none border-0 border-b border-border bg-(--native-toolbar) px-[16px] py-[10px] text-start text-[length:inherit] wrap-anywhere",
        tone === "error"
          ? "m-0 flex flex-wrap items-center gap-[8px] text-(--danger)"
          : "block gap-[normal] text-foreground",
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
