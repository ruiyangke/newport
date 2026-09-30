import type { GitStatus } from "../domain/gitResponses";
import type { GitOperationReceipt, GitWriteAction } from "../domain/git";
import { Button, Textarea } from "./controls";
import { GitDiscardControl } from "./GitDiscardControl";
import { useState } from "react";
import { Modal } from "./Editors";
import { Checkbox } from "./ui/checkbox";

/** Helper copy in the recovery panel: small and muted. */
const note = "text-[12px] text-muted-foreground";

/**
 * What can be done to the selected file as a whole: stage or unstage it, or
 * discard its changes. The diff header carries these; the group names the file
 * for anyone who cannot see the header's path.
 */
export function GitFileActions({
  entry,
  status,
  disabled,
  busy = false,
  error = "",
  compact = false,
  className = "",
  onAction,
}: {
  entry: GitStatus["entries"][number];
  status: GitStatus;
  disabled: boolean;
  busy?: boolean;
  error?: string;
  /** Discard as an icon, for the diff header. */
  compact?: boolean;
  className?: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
}) {
  return (
    <div
      className={`git-stage-controls flex flex-none flex-wrap items-center gap-[6px] ${className}`}
      role="group"
      aria-label={`Actions for ${entry.path?.display ?? entry.oldPath?.display ?? "the selected file"}`}
    >
      {(entry.unstaged || entry.untracked || entry.conflicted) && (
        <Button
          disabled={disabled}
          onClick={() => onAction({ kind: "stage", entryIds: [entry.entryId] })}
        >
          {entry.conflicted ? "Stage resolution" : "Stage file"}
        </Button>
      )}
      {entry.staged && !status.metadata.integration && (
        <Button
          disabled={disabled}
          onClick={() =>
            onAction({ kind: "unstage", entryIds: [entry.entryId] })
          }
        >
          Unstage file
        </Button>
      )}
      <GitDiscardControl
        entry={entry}
        status={status}
        disabled={disabled}
        busy={busy}
        error={error}
        compact={compact}
        onAction={onAction}
      />
    </div>
  );
}

export function GitWriteControls({
  entry,
  status,
  disabled,
  busy = false,
  error = "",
  message,
  onMessage,
  showCommit = true,
  onAction,
}: {
  entry?: GitStatus["entries"][number];
  status: GitStatus;
  disabled: boolean;
  busy?: boolean;
  error?: string;
  message: string;
  onMessage: (value: string) => void;
  /** The composer lives under the file list; the detail pane hides it. */
  showCommit?: boolean;
  onAction: (action: GitWriteAction) => Promise<boolean>;
}) {
  const canCommit =
    !disabled &&
    !!message.trim() &&
    status.entries.some((entry) => entry.staged) &&
    !status.metadata.integration;
  return (
    <div className="git-write-controls border-b border-border px-[16px] py-[12px]">
      {entry && (
        <GitFileActions
          entry={entry}
          status={status}
          disabled={disabled}
          busy={busy}
          error={error}
          onAction={onAction}
          className="mb-[16px] justify-end"
        />
      )}
      {showCommit && !status.metadata.integration && (
        <form
          className="git-commit-form grid gap-[8px]"
          onSubmit={(event) => {
            event.preventDefault();
            if (canCommit) onAction({ kind: "commit", message });
          }}
        >
          <label className="grid gap-[6px] text-[12px]">
            Commit message
            <Textarea
              value={message}
              onChange={(event) => onMessage(event.target.value)}
              disabled={disabled}
              placeholder="Describe the staged changes"
              rows={2}
              required
            />
          </label>
          <Button
            type="submit"
            className="[justify-self:start]"
            disabled={!canCommit}
          >
            Commit staged changes
          </Button>
        </form>
      )}
    </div>
  );
}

const outcomeLabels: Record<GitOperationReceipt["state"], string> = {
  pending: "Outcome not yet confirmed",
  outcome_unknown: "Outcome unknown",
  reviewed_unknown: "Reviewed · outcome unknown",
  succeeded: "Completed",
  failed: "Failed",
  rejected: "Not applied",
  needs_resolution: "Needs conflict resolution",
};
export function GitRecoveryPanel({
  receipts,
  repositories = {},
  error,
  busy,
  onCheck,
  onAcknowledge,
  onReview,
  onRefresh,
}: {
  receipts: GitOperationReceipt[] | null;
  repositories?: Record<string, string>;
  error: string;
  busy: boolean;
  onCheck: (id: string) => void;
  onAcknowledge: (id: string) => void;
  onReview?: (id: string) => void;
  onRefresh: () => void;
}) {
  const [reviewing, setReviewing] = useState<string | null>(null);
  const [inspected, setInspected] = useState(false);
  const review = receipts?.find(
    (receipt) =>
      receipt.operationId === reviewing && receipt.state === "outcome_unknown",
  );
  if (!error && (!receipts || !receipts.length)) return null;
  return (
    <section
      className="git-recovery border-b border-border px-[16px] py-[12px]"
      aria-label="Git operation recovery"
    >
      <header className="flex flex-wrap items-center gap-[8px]">
        <h3 className="flex-1 text-[13px]! font-[600]!">
          Saved outcomes on this server
        </h3>
        <Button disabled={busy} onClick={onRefresh}>
          Refresh outcomes
        </Button>
      </header>
      <p className={note}>
        These operations may belong to another repository on this server.
      </p>
      {error && (
        <p role="alert" className={note}>
          {error}
        </p>
      )}
      {receipts?.map((receipt) => {
        const unresolved =
          receipt.state === "pending" || receipt.state === "outcome_unknown";
        return (
          <div
            className="git-recovery-row flex flex-wrap items-center gap-[8px] pt-[12px]"
            key={receipt.operationId}
          >
            <div className="min-w-[200px] flex-1 [overflow-wrap:anywhere]">
              <strong className="text-[12px] font-[500]">
                {receipt.action} · {outcomeLabels[receipt.state]}
              </strong>
              <p className={note}>
                {repositories[receipt.operationId] ?? "Repository not recorded"}
              </p>
              <code className="block text-[11px]! text-muted-foreground">
                {receipt.operationId}
              </code>
              {unresolved && (
                <p className={note}>
                  Check the result before repeating this operation.
                </p>
              )}
            </div>
            {receipt.state !== "rejected" && (
              <Button
                disabled={busy}
                onClick={() => onCheck(receipt.operationId)}
              >
                Check outcome
              </Button>
            )}
            {!unresolved && (
              <Button
                disabled={busy}
                onClick={() => onAcknowledge(receipt.operationId)}
              >
                Dismiss outcome
              </Button>
            )}
            {receipt.state === "outcome_unknown" && onReview && (
              <Button
                disabled={busy}
                onClick={() => {
                  setInspected(false);
                  setReviewing(receipt.operationId);
                }}
              >
                Review interrupted operation…
              </Button>
            )}
          </div>
        );
      })}
      {review && onReview && (
        <Modal
          title="Review interrupted operation"
          busy={busy}
          onClose={() => setReviewing(null)}
        >
          <div className="grid gap-4 px-6 pb-6 text-sm">
            <p>
              {review.action} may have changed the repository before it stopped.
              Inspect its branch, files, and remote state before allowing new
              operations.
            </p>
            <p>
              Recording this review keeps the original outcome unknown. It does
              not retry the operation or mark it successful.
            </p>
            <label className="flex items-start gap-2">
              <Checkbox
                checked={inspected}
                disabled={busy}
                onCheckedChange={(value) => setInspected(value === true)}
              />
              I inspected the repository and am ready to allow new operations.
            </label>
            <footer className="flex justify-end gap-2">
              <Button disabled={busy} onClick={() => setReviewing(null)}>
                Cancel
              </Button>
              <Button
                disabled={busy || !inspected}
                onClick={() => {
                  if (!inspected || busy) return;
                  onReview(review.operationId);
                  setReviewing(null);
                }}
              >
                Record review
              </Button>
            </footer>
          </div>
        </Modal>
      )}
    </section>
  );
}
