import { cn } from "cn";

/**
 * One letter for what happened to a file, in one vocabulary everywhere a file
 * is listed: the Changes list, the diff header, and a commit's files. Colour is
 * the second channel, never the only one -- the letter carries the meaning,
 * and the accessible name spells it out.
 */
export type GitStatusMark = "A" | "M" | "D" | "R" | "C" | "T" | "?" | "!";

const NAMES: Record<GitStatusMark, string> = {
  A: "Added",
  M: "Modified",
  D: "Deleted",
  R: "Renamed",
  C: "Copied",
  T: "Type changed",
  "?": "Untracked",
  "!": "Conflicted",
};
/*
 * The app's data colours: green, orange and red are state tokens that
 * src/styles.css remaps for dark, and primary is the one accent. The tint is
 * the same ink at low strength, so the badge reads as a mark, not a button.
 */
const TONES: Record<GitStatusMark, string> = {
  A: "text-(--green) bg-[color-mix(in_srgb,var(--green)_14%,transparent)]",
  "?": "text-(--green) bg-[color-mix(in_srgb,var(--green)_14%,transparent)]",
  M: "text-(--orange) bg-[color-mix(in_srgb,var(--orange)_16%,transparent)]",
  D: "text-(--red) bg-[color-mix(in_srgb,var(--red)_14%,transparent)]",
  "!": "text-white bg-(--red)",
  R: "text-primary bg-[color-mix(in_srgb,var(--primary)_14%,transparent)]",
  C: "text-primary bg-[color-mix(in_srgb,var(--primary)_14%,transparent)]",
  T: "text-primary bg-[color-mix(in_srgb,var(--primary)_14%,transparent)]",
};

/** A commit file's agent status, in the same letters as the Changes list. */
export function markForStatus(status: string): GitStatusMark {
  switch (status) {
    case "Added":
      return "A";
    case "Deleted":
      return "D";
    case "Renamed":
      return "R";
    case "Copied":
      return "C";
    case "Typechange":
      return "T";
    case "Untracked":
      return "?";
    case "Conflicted":
      return "!";
    default:
      return "M";
  }
}

export function gitStatusName(mark: GitStatusMark) {
  return NAMES[mark];
}

export function GitStatusBadge({
  mark,
  className,
  decorative = false,
}: {
  mark: GitStatusMark;
  className?: string;
  /** Hidden from assistive technology when the row already names the state. */
  decorative?: boolean;
}) {
  return (
    <span
      className={cn(
        "git-status-badge inline-flex size-[16px] flex-none items-center justify-center rounded-[4px] font-mono text-[10px] leading-none font-bold",
        TONES[mark],
        className,
      )}
      data-mark={mark}
      title={NAMES[mark]}
      {...(decorative
        ? { "aria-hidden": true }
        : { role: "img", "aria-label": NAMES[mark] })}
    >
      {mark}
    </span>
  );
}
