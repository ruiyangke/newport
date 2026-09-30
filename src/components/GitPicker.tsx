import type { ComponentProps, ReactNode } from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "cn";
import { Button } from "./ui/button";

/**
 * The toolbar's repository and branch pickers: one line, as wide as what they
 * hold. The two-line "Current repository / name" form spent a 56px toolbar and
 * a 250px slot on a label the icon already gives, and left the chevron ~180px
 * from the value it opens. The label is still the control's name -- it is in
 * the tooltip and, through the caller's aria-label, the accessible name.
 *
 * `!` only where `src/styles.css` has an UNLAYERED `[data-slot="button"]` claim
 * (height, radius, padding-inline, font-size, font-weight): unlayered CSS beats
 * `@layer utilities` whatever the specificity.
 */
export function GitPicker({
  icon,
  label,
  value,
  valueTitle,
  emphasis = true,
  className,
  ...props
}: {
  icon: ReactNode;
  label: string;
  value: ReactNode;
  valueTitle?: string;
  /** The repository leads; the branch beside it is set a step quieter. */
  emphasis?: boolean;
} & Omit<ComponentProps<typeof Button>, "value">) {
  return (
    <Button
      variant="ghost"
      data-label={label}
      className={cn(
        "h-[30px]! max-w-[280px] min-w-0 flex-[0_1_auto] justify-start gap-[6px] rounded-[6px]! border-0 bg-transparent px-[8px]! text-left text-foreground hover:bg-accent aria-expanded:bg-accent",
        className,
      )}
      {...props}
    >
      <span className="flex flex-none text-muted-foreground" aria-hidden="true">
        {icon}
      </span>
      {/* The value is the part that truncates, so it carries the whole name
          as its title: reachable with a pointer as well as through the
          button's accessible name. */}
      <strong
        className={cn(
          "min-w-0 truncate text-[13px]",
          emphasis ? "font-semibold" : "font-medium",
        )}
        title={valueTitle}
      >
        {value}
      </strong>
      <ChevronDown
        size={13}
        aria-hidden="true"
        className="flex-none text-muted-foreground"
      />
    </Button>
  );
}
