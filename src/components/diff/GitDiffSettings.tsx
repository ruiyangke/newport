import { useId } from "react";
import { Settings2 } from "lucide-react";
import { Button, Select, SelectItem } from "../controls";
import { Popover, PopoverContent, PopoverTrigger } from "../ui/popover";
import { RadioGroup, RadioGroupItem } from "../ui/radio-group";
import { Label } from "../ui/label";
import { useCurrentGitState } from "../../state/git";

export const CONTEXT_CHOICES = [0, 1, 3, 10, 25] as const;

/**
 * How a diff is drawn, in one place on the diff's own header: unified or split
 * (followed by every diff on the page) and, where the comparison can be re-read
 * with a different amount, the context around each change. There is no
 * whitespace option: the agent's diff has none, and a control that did nothing
 * would be worse than its absence.
 */
export function GitDiffSettings({
  context,
  onContext,
}: {
  /** Omitted for diffs that cannot be re-read with other context (history). */
  context?: number;
  onContext?: (value: number) => void;
}) {
  const id = useId();
  const [layout, setLayout] = useCurrentGitState("diffLayout");
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          aria-label="Diff Settings"
          className="h-[26px]! w-[28px] flex-none text-muted-foreground hover:text-foreground"
        >
          <Settings2 size={15} aria-hidden="true" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        className="grid w-[220px] gap-[12px] rounded-[8px] p-[12px] text-[12px]"
      >
        {/* The unlayered bare `h3` rule claims size, weight and margin. */}
        <h3 className="text-[12px]! font-semibold!">Diff Settings</h3>
        <fieldset className="grid gap-[6px]">
          <legend className="mb-[6px] text-[11px] font-medium text-muted-foreground">
            Diff display
          </legend>
          <RadioGroup
            value={layout}
            onValueChange={(next) => setLayout(next as typeof layout)}
            className="gap-[6px]"
          >
            {(["unified", "split"] as const).map((value) => (
              <div key={value} className="flex h-[18px] items-center gap-[6px]">
                <RadioGroupItem value={value} id={`${id}-${value}`} />
                <Label
                  htmlFor={`${id}-${value}`}
                  className="text-[12px] font-normal"
                >
                  {value === "unified" ? "Unified" : "Split"}
                </Label>
              </div>
            ))}
          </RadioGroup>
        </fieldset>
        {context !== undefined && onContext && (
          <label className="grid gap-[6px]">
            <span className="text-[11px] font-medium text-muted-foreground">
              Context
            </span>
            <Select
              aria-label="Context lines"
              value={String(context)}
              onValueChange={(value) => onContext(Number(value))}
            >
              {CONTEXT_CHOICES.map((value) => (
                <SelectItem key={value} value={String(value)}>
                  {value === 1 ? "1 context line" : `${value} context lines`}
                </SelectItem>
              ))}
            </Select>
          </label>
        )}
      </PopoverContent>
    </Popover>
  );
}

/** "+3 −1", with the counts spelled out for assistive technology. */
export function DiffStat({
  additions,
  deletions,
}: {
  additions: number;
  deletions: number;
}) {
  const plural = (count: number, noun: string) =>
    `${count} ${noun}${count === 1 ? "" : "s"}`;
  return (
    <span
      className="git-diff-stat flex flex-none items-center gap-[6px] font-mono text-[11px] font-medium tabular-nums"
      role="img"
      aria-label={`${plural(additions, "addition")}, ${plural(deletions, "deletion")}`}
      title={`${plural(additions, "addition")}, ${plural(deletions, "deletion")}`}
    >
      <span className="text-(--green)">+{additions}</span>
      <span className="text-(--red)">−{deletions}</span>
    </span>
  );
}
