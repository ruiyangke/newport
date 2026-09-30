import { useState } from "react";
import type { GitHistory, GitRepository } from "../domain/gitResponses";
import type { GitWriteAction } from "../domain/git";
import { Button, Textarea } from "./controls";
import { Modal } from "./Editors";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "./ui/dropdown-menu";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";
const operations = {
  merge: "Merge into current branch",
  cherry_pick: "Cherry-pick commit",
  revert: "Revert commit",
  rebase: "Rebase current branch onto commit",
  reset: "Reset to commit",
  amend: "Amend latest commit",
  checkout: "Check out commit",
} as const;
function completeMessage(commit: GitHistory["entries"][number]) {
  if (commit.messageTruncated) return false;
  try {
    return (
      new TextDecoder("utf-8", { fatal: true }).decode(
        Uint8Array.from(atob(commit.message.bytesB64), (char) =>
          char.charCodeAt(0),
        ),
      ) === commit.message.display
    );
  } catch {
    return false;
  }
}
export function GitHistoryActions({
  commit,
  snapshot,
  conflicted = false,
  repository,
  disabled,
  busy,
  error,
  onAction,
}: {
  commit: GitHistory["entries"][number];
  snapshot?: string;
  conflicted?: boolean;
  repository: GitRepository;
  disabled: boolean;
  busy: boolean;
  error: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
}) {
  const resetBlockedReason = conflicted
    ? "Resolve conflicts before resetting."
    : undefined;
  const [kind, setKind] = useState<keyof typeof operations | null>(null);
  const [parent, setParent] = useState("1");
  const [resetMode, setResetMode] = useState<"soft" | "mixed" | "hard">("soft");
  // Refusing to create a merge commit is a strategy choice, reset per dialog.
  const [fastForward, setFastForward] = useState(false);
  const [confirmedHead, setConfirmedHead] = useState<string | null>(null);
  const [confirmedSnapshot, setConfirmedSnapshot] = useState<
    string | undefined
  >();
  const [amendMessage, setAmendMessage] = useState("");
  const [confirmedCommit, setConfirmedCommit] = useState("");
  const messageComplete = completeMessage(commit);
  const latest = commit.oid.hex === repository.head.oid?.hex;
  const confirmationStale =
    (kind === "reset" || kind === "amend" || kind === "checkout") &&
    (confirmedSnapshot !== snapshot ||
      confirmedHead !== repository.head.oid?.hex ||
      confirmedCommit !== commit.oid.hex);
  const checkoutBlocked = kind === "checkout" && (!snapshot || conflicted);
  const amendBlocked =
    kind === "amend" &&
    (!latest || !amendMessage.trim() || !!resetBlockedReason || !snapshot);
  const replay = kind === "cherry_pick" || kind === "revert";
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button disabled={disabled}>Commit actions</Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {(Object.keys(operations) as (keyof typeof operations)[]).map(
            (action) => (
              <DropdownMenuItem
                key={action}
                disabled={
                  (action === "checkout" && (!snapshot || conflicted)) ||
                  (action === "amend" &&
                    (!latest || !snapshot || !!resetBlockedReason)) ||
                  (action === "reset" &&
                    (!repository.head.oid || !snapshot || !!resetBlockedReason))
                }
                onSelect={() => {
                  setKind(action);
                  setParent("1");
                  setResetMode("soft");
                  setFastForward(false);
                  setConfirmedHead(repository.head.oid?.hex ?? null);
                  setConfirmedSnapshot(snapshot);
                  setConfirmedCommit(commit.oid.hex);
                  setAmendMessage(
                    messageComplete ? commit.message.display : "",
                  );
                }}
              >
                {action === "checkout" && conflicted
                  ? "Checkout unavailable: resolve conflicts first"
                  : action === "amend" && !latest
                    ? "Amend unavailable: select the latest commit"
                    : action === "amend" && resetBlockedReason
                      ? "Amend unavailable: resolve conflicts first"
                      : action === "reset" && resetBlockedReason
                        ? `Reset unavailable: ${resetBlockedReason}`
                        : `${operations[action]}…`}
              </DropdownMenuItem>
            ),
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      {kind && (
        <Modal
          title={operations[kind]}
          onClose={() => setKind(null)}
          busy={busy}
        >
          <div className="git-integration-confirm">
            {error && <p role="alert">{error}</p>}
            <p>
              Current branch:{" "}
              <strong>
                {repository.head.name?.display.replace(/^refs\/heads\//, "") ??
                  "Detached HEAD"}
              </strong>
            </p>
            <p>
              Selected commit: <code>{commit.oid.hex.slice(0, 12)}</code> ·{" "}
              {commit.message.display.split("\n")[0]}
            </p>
            <p>
              {kind === "checkout"
                ? "Switch your working files to this commit without moving any branch. You will be in detached HEAD state."
                : kind === "amend"
                  ? "Replace the latest commit with your staged content and the message below. Its commit ID will change; the original author will be preserved."
                  : kind === "reset"
                    ? "Move the current branch (or detached HEAD) to this commit. Choose what happens to your staged and working changes."
                    : kind === "merge"
                      ? "Combine this commit’s history with the current branch. Conflicts may require resolution before completing the merge."
                      : kind === "rebase"
                        ? "Replay the current branch’s commits onto the selected commit. This rewrites their commit IDs."
                        : kind === "revert"
                          ? "Create a new commit reversing the changes introduced by this commit."
                          : "Apply the changes introduced by this commit to the current branch."}
            </p>
            {kind === "merge" && (
              <>
                <label className="git-branch-force">
                  <input
                    type="checkbox"
                    checked={fastForward}
                    disabled={busy}
                    onChange={(event) => setFastForward(event.target.checked)}
                  />
                  Fast-forward only
                </label>
                <p>
                  {fastForward
                    ? "Move the current branch straight to this commit. It is refused unless this commit already contains the branch’s history, so no merge commit is created and no conflict can occur."
                    : "A merge commit is created when the histories have diverged."}
                </p>
              </>
            )}
            {kind === "checkout" && (
              <>
                <p>
                  To keep new commits you make here, create a branch before
                  switching away. Use the branch selector to return to a branch.
                </p>
                <p>
                  Local changes are kept when possible. Checkout is refused if
                  it would overwrite them. Commit or stash conflicting changes
                  before switching.
                </p>
                {confirmationStale && (
                  <p role="alert">
                    The repository or selected commit changed. Close this dialog
                    and review the checkout again.
                  </p>
                )}
                {conflicted && (
                  <p role="alert">Resolve conflicts before checking out.</p>
                )}
              </>
            )}
            {kind === "amend" && (
              <>
                {!messageComplete && (
                  <p role="status">
                    The original message is truncated or cannot be represented
                    as text. Enter a complete replacement message.
                  </p>
                )}
                <label>
                  Replacement commit message
                  <Textarea
                    aria-label="Replacement commit message"
                    autoFocus
                    value={amendMessage}
                    onChange={(event) => setAmendMessage(event.target.value)}
                    disabled={busy}
                    rows={5}
                  />
                </label>
                <p>
                  Any staged changes will be included. Unstaged changes will
                  remain in your working files. No remote branch will be
                  updated.
                </p>
                {confirmationStale && (
                  <p role="alert">
                    The repository changed. Close this dialog and review the
                    latest commit again.
                  </p>
                )}
                {resetBlockedReason && (
                  <p role="alert">Resolve conflicts before amending.</p>
                )}
              </>
            )}
            {kind === "reset" && (
              <>
                <label>
                  Reset mode
                  <Select
                    value={resetMode}
                    onValueChange={(value) =>
                      setResetMode(value as "soft" | "mixed" | "hard")
                    }
                    disabled={busy}
                  >
                    <SelectTrigger aria-label="Reset mode">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="soft">
                        Soft — keep staged and working changes
                      </SelectItem>
                      <SelectItem value="mixed">
                        Mixed — keep files, unstage changes
                      </SelectItem>
                      <SelectItem value="hard">
                        Hard — discard tracked changes
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </label>
                <p>
                  {resetMode === "soft"
                    ? "Moves the branch pointer only. Staged changes and working files stay as they are."
                    : resetMode === "mixed"
                      ? "Resets staging to the selected commit. Working files stay as they are, with differences left unstaged."
                      : "Replaces tracked working files and staging with the selected commit. Uncommitted tracked changes will be lost. Untracked files are kept; collisions are refused."}
                </p>
                <p>
                  This changes local history. No remote branch will be changed.
                </p>
                {confirmationStale && (
                  <p role="alert">
                    The repository changed. Close this dialog and review the
                    reset again.
                  </p>
                )}
                {resetBlockedReason && <p role="alert">{resetBlockedReason}</p>}
              </>
            )}
            {replay && commit.parents.length > 1 && (
              <label>
                Mainline parent
                <Select
                  value={parent}
                  onValueChange={setParent}
                  disabled={busy}
                >
                  <SelectTrigger aria-label="Mainline parent">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {commit.parents.map((oid, index) => (
                      <SelectItem key={index} value={String(index + 1)}>
                        Parent {index + 1} · {oid.hex.slice(0, 8)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </label>
            )}
            <footer>
              <Button disabled={busy} onClick={() => setKind(null)}>
                Cancel
              </Button>
              <Button
                variant={
                  kind === "reset" && resetMode === "hard"
                    ? "destructive"
                    : undefined
                }
                disabled={
                  disabled ||
                  confirmationStale ||
                  amendBlocked ||
                  checkoutBlocked ||
                  (kind === "reset" && (!confirmedHead || !!resetBlockedReason))
                }
                onClick={async () => {
                  if (
                    disabled ||
                    confirmationStale ||
                    amendBlocked ||
                    checkoutBlocked ||
                    (kind === "reset" &&
                      (!confirmedHead || !!resetBlockedReason))
                  )
                    return;
                  const targetOid = commit.oid.hex;
                  const action: GitWriteAction =
                    kind === "checkout"
                      ? {
                          kind: "checkout",
                          target: { kind: "detached", oid: confirmedCommit },
                        }
                      : kind === "amend"
                        ? {
                            kind: "commit.amend",
                            expectedOid: confirmedHead!,
                            message: amendMessage,
                          }
                        : kind === "reset"
                          ? {
                              kind,
                              targetOid,
                              expectedOid: confirmedHead!,
                              mode: resetMode,
                            }
                          : kind === "rebase"
                            ? { kind, upstreamOid: targetOid }
                            : kind === "merge"
                              ? {
                                  kind: fastForward
                                    ? "merge.fast_forward"
                                    : kind,
                                  targetOid,
                                }
                              : {
                                  kind,
                                  targetOid,
                                  mainline:
                                    commit.parents.length > 1
                                      ? Number(parent)
                                      : 0,
                                };
                  if (await onAction(action)) setKind(null);
                }}
              >
                {kind === "checkout"
                  ? "Check out commit"
                  : kind === "amend"
                    ? "Replace latest commit"
                    : kind === "reset"
                      ? resetMode === "hard"
                        ? "Discard changes and reset"
                        : "Reset to commit"
                      : kind === "merge"
                        ? fastForward
                          ? "Fast-forward"
                          : "Merge"
                        : kind === "rebase"
                          ? "Start rebase"
                          : kind === "revert"
                            ? "Revert commit"
                            : "Cherry-pick"}
              </Button>
            </footer>
          </div>
        </Modal>
      )}
    </>
  );
}
