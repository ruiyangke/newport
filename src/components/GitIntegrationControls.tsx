import { useState } from "react";
import type { GitStatus } from "../domain/gitResponses";
import type { GitWriteAction } from "../domain/git";
import { Button, Textarea } from "./controls";
import { Modal } from "./Editors";
const labels: Record<string, string> = {
  merge: "Merge",
  cherry_pick: "Cherry-pick",
  revert: "Revert",
  rebase: "Rebase",
};
export function GitIntegrationControls({
  status,
  disabled,
  busy,
  error,
  onAction,
}: {
  status: GitStatus;
  disabled: boolean;
  busy: boolean;
  error: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
}) {
  const [confirm, setConfirm] = useState<"continue" | "abort" | "skip" | null>(
    null,
  );
  const [commitMessage, setCommitMessage] = useState("");
  const integration = status.metadata.integration;
  if (!integration) return null;
  const label = labels[integration.kind] ?? "Git operation";
  const conflicts = status.entries.some((entry) => entry.conflicted);
  return (
    <section
      className="git-integration flex flex-wrap items-center gap-[16px] border-b border-border bg-muted px-[16px] py-[14px]"
      aria-label="Active Git operation"
    >
      <div className="min-w-[220px] flex-1">
        <strong className="text-[13px] font-[600]">
          {label} in progress
          {integration.total != null && integration.position != null
            ? ` · ${integration.position} of ${integration.total}`
            : ""}
        </strong>
        <p className="mt-[4px]! text-[12px] text-muted-foreground">
          {!integration.managed
            ? "This operation cannot be managed by Newport. Finish it with the tool that started it, then refresh."
            : conflicts
              ? "Resolve the conflicting files, then stage each resolution before continuing."
              : "Review the staged changes before continuing. Git checks for remaining conflicts."}
        </p>
      </div>
      {integration.managed && (
        <div className="git-integration-actions flex flex-wrap gap-[8px]">
          {integration.canContinue && (
            <Button
              disabled={disabled || conflicts}
              onClick={() => {
                setCommitMessage("");
                setConfirm("continue");
              }}
            >
              Continue {label.toLowerCase()}
            </Button>
          )}
          {integration.canSkip && (
            <Button disabled={disabled} onClick={() => setConfirm("skip")}>
              Skip commit…
            </Button>
          )}
          {integration.canAbort && (
            <Button disabled={disabled} onClick={() => setConfirm("abort")}>
              Abort…
            </Button>
          )}
        </div>
      )}
      {confirm && (
        <Modal
          title={
            confirm === "abort"
              ? `Abort ${label.toLowerCase()}`
              : confirm === "continue"
                ? `Continue ${label.toLowerCase()}`
                : "Skip current commit"
          }
          busy={busy}
          onClose={() => setConfirm(null)}
        >
          <div className="git-integration-confirm">
            {error && <p role="alert">{error}</p>}
            <p>
              {confirm === "abort"
                ? "Return to the state before this operation. Conflict resolutions and edits made during it may be discarded."
                : confirm === "continue"
                  ? "Continue with the staged resolution. Leave the message blank to use the default message for this operation."
                  : "Omit the current commit from this rebase and discard its pending resolution. Later commits will still be processed."}
            </p>
            {confirm === "continue" && (
              <label>
                Commit message (optional)
                <Textarea
                  value={commitMessage}
                  onChange={(event) => setCommitMessage(event.target.value)}
                  disabled={busy}
                  rows={3}
                />
              </label>
            )}
            <footer>
              <Button disabled={busy} onClick={() => setConfirm(null)}>
                Cancel
              </Button>
              <Button
                variant={confirm === "abort" ? "destructive" : undefined}
                disabled={disabled}
                onClick={async () => {
                  if (
                    await onAction(
                      confirm === "continue"
                        ? {
                            kind: "integration.continue",
                            ...(commitMessage.trim()
                              ? { message: commitMessage }
                              : {}),
                          }
                        : {
                            kind:
                              confirm === "abort"
                                ? "integration.abort"
                                : "integration.skip",
                          },
                    )
                  )
                    setConfirm(null);
                }}
              >
                {confirm === "abort"
                  ? "Abort operation"
                  : confirm === "continue"
                    ? "Continue operation"
                    : "Skip commit"}
              </Button>
            </footer>
          </div>
        </Modal>
      )}
    </section>
  );
}
