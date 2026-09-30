import type { GitStatus } from "../domain/gitResponses";
import type { GitWriteAction } from "../domain/git";
import { Button, Input, Textarea } from "./controls";

/** Git's own convention for how long a subject line should be. */
const SUMMARY_LIMIT = 50;

/** Git's own convention: a summary line, a blank line, then the body. */
export function composeMessage(summary: string, description: string) {
  const head = summary.trim();
  const body = description.trim();
  return body ? `${head}\n\n${body}` : head;
}

/**
 * The commit composer sits under the file list, as the design has it. It counts
 * staged entries rather than selected rows, because this backend commits the
 * index, not a checkbox selection.
 */
export function GitCommitComposer({
  status,
  branch,
  summary,
  description,
  disabled,
  onSummary,
  onDescription,
  onAction,
}: {
  status: GitStatus;
  branch: string | null;
  summary: string;
  description: string;
  disabled: boolean;
  onSummary: (value: string) => void;
  onDescription: (value: string) => void;
  onAction: (action: GitWriteAction) => Promise<boolean>;
}) {
  const staged = status.entries.filter((entry) => entry.staged).length;
  const integrating = !!status.metadata.integration;
  const canCommit = !disabled && !!summary.trim() && staged > 0 && !integrating;
  if (integrating) return null;
  return (
    <form
      className="git-composer mt-auto flex flex-none flex-col gap-[6px] border-t border-border bg-(--native-toolbar) p-[10px]"
      aria-label="Commit"
      onSubmit={(event) => {
        event.preventDefault();
        if (canCommit)
          void onAction({
            kind: "commit",
            message: composeMessage(summary, description),
          });
      }}
    >
      {/* No avatar: an initial guessed from nothing ("Y" for "You") said less
          than no mark at all, and the author is Git's configuration on the
          server, which this form does not read. */}
      <Input
        aria-label="Commit summary"
        className="h-[28px]! rounded-[5px]! bg-background px-[8px] text-[12px]!"
        placeholder="Summary (required)"
        value={summary}
        disabled={disabled}
        onChange={(event) => onSummary(event.target.value)}
        required
      />
      {/* Advisory, never blocking: git's convention that a subject line stays
          short (50 is silent, 51 shows it). Text rather than a hover-only
          tooltip, so it reaches a reader who is not using a pointer. */}
      {summary.trim().length > SUMMARY_LIMIT && (
        <p
          className="git-composer-hint text-[11px] leading-[15px] text-muted-foreground"
          role="status"
        >
          Summaries under {SUMMARY_LIMIT} characters read best in logs. Put the
          detail in the description.
        </p>
      )}
      <Textarea
        aria-label="Commit description"
        className="min-h-[58px] resize-none rounded-[5px]! bg-background px-[8px] py-[6px] text-[12px]!"
        placeholder="Description"
        value={description}
        disabled={disabled}
        rows={3}
        onChange={(event) => onDescription(event.target.value)}
      />
      {/* A disabled primary button at half opacity read as broken, and in dark
          it put navy text on navy. Disabled is stated as a neutral instead. */}
      <Button
        type="submit"
        variant="default"
        className="git-composer-commit mt-[2px] h-[30px]! w-full rounded-[5px]! text-[12px]! font-medium! disabled:border-border disabled:bg-background disabled:text-muted-foreground disabled:opacity-100"
        disabled={!canCommit}
      >
        {staged === 0
          ? "No staged changes to commit"
          : `Commit ${staged} ${staged === 1 ? "file" : "files"}${branch ? ` to ${branch}` : ""}`}
      </Button>
    </form>
  );
}
