import { GitBlobPreview } from "./GitBlobPreview";
import { useRef, useState } from "react";
import type { GitStatus } from "../domain/gitResponses";
import type { GitWriteAction } from "../domain/git";
import { Button } from "./controls";
import { Modal } from "./Editors";
import { GitNotice } from "./GitNotice";

type Entry = GitStatus["entries"][number];
type Side = "ours" | "theirs" | "base";
const LABELS: Record<Side, { title: string; detail: string }> = {
  ours: { title: "Ours", detail: "the branch you are on" },
  theirs: { title: "Theirs", detail: "the incoming change" },
  base: { title: "Base", detail: "the common ancestor" },
};
function messageOf(reason: unknown) {
  return reason instanceof Error ? reason.message : String(reason);
}

/**
 * Inspect the three recorded sides of a conflict and resolve to one of them.
 * The agent writes the blob it already holds; no content is sent from here.
 */
export function GitConflictControls({
  repoId,
  snapshot,
  entry,
  disabled = false,
  blockedReason,
  onAction,
}: {
  repoId: string;
  snapshot: string;
  entry: Entry;
  disabled?: boolean;
  blockedReason?: string;
  onAction?: (action: GitWriteAction, snapshot: string) => Promise<boolean>;
}) {
  const conflict = entry.conflict;
  const [viewing, setViewing] = useState<Side | null>(null);
  const [confirm, setConfirm] = useState<Side | null>(null);
  const [error, setError] = useState("");
  const [applying, setApplying] = useState(false);
  const pending = useRef(false);
  const oidOf = (side: Side) => conflict?.[side]?.oid.hex ?? null;

  async function resolve(side: Side) {
    if (!onAction || disabled || blockedReason || pending.current) return;
    pending.current = true;
    setApplying(true);
    try {
      await onAction(
        {
          kind: "conflict.resolve",
          entryIds: [entry.entryId],
          side,
          expectedOid: oidOf(side),
        },
        snapshot,
      );
    } catch (reason) {
      setError(messageOf(reason));
    } finally {
      pending.current = false;
      setApplying(false);
      setConfirm(null);
    }
  }

  if (!conflict)
    return (
      <GitNotice tone="info">
        This file is conflicted, but its recorded sides are unavailable. Refresh
        the repository.
      </GitNotice>
    );
  const sides: Side[] = ["ours", "theirs", "base"];
  return (
    <section className="git-conflict" aria-label="Conflict sides">
      <GitNotice tone="info">
        Unresolved conflict. Choose one side below, or edit the file in Terminal
        and stage it. Combining both sides is not done here.
      </GitNotice>
      {error && <GitNotice tone="error">{error}</GitNotice>}
      <ul className="git-conflict-sides m-0 flex list-none flex-col gap-[8px] px-[12px] pt-0 pb-[12px]">
        {sides.map((side) => {
          const present = !!conflict[side];
          return (
            <li key={side} className="border-b border-border px-0 py-[8px]">
              <div className="git-conflict-row flex items-center gap-[12px]">
                <div className="min-w-0 flex-1">
                  <strong>{LABELS[side].title}</strong>
                  <span> — {LABELS[side].detail}</span>
                  <p className="mt-[2px]! text-[12px] text-muted-foreground">
                    {present ? (
                      <code>{conflict[side]!.oid.hex.slice(0, 12)}</code>
                    ) : (
                      "Absent — this side deleted the file."
                    )}
                  </p>
                </div>
                {present && (
                  <Button
                    size="sm"
                    variant="ghost"
                    aria-label={`View ${LABELS[side].title}`}
                    onClick={() => {
                      // Viewing another side clears an earlier failure.
                      if (viewing !== side) setError("");
                      setViewing(viewing === side ? null : side);
                    }}
                  >
                    {viewing === side ? "Hide" : "View"}
                  </Button>
                )}
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={
                    disabled || !!blockedReason || applying || !onAction
                  }
                  aria-label={`Use ${LABELS[side].title}`}
                  onClick={() => setConfirm(side)}
                >
                  {present ? `Use ${LABELS[side].title}` : "Delete the file"}
                </Button>
              </div>
              {/* Keep the content with the side it belongs to. */}
              {viewing === side && oidOf(side) && (
                <GitBlobPreview
                  key={`${repoId}:${oidOf(side)}`}
                  repoId={repoId}
                  oid={oidOf(side)!}
                  label={`${LABELS[side].title} content`}
                />
              )}
            </li>
          );
        })}
      </ul>
      {confirm && (
        <Modal
          title={
            conflict[confirm]
              ? `Use ${LABELS[confirm].title}?`
              : "Delete this file?"
          }
          busy={applying}
          onClose={() => setConfirm(null)}
        >
          <p>
            {conflict[confirm]
              ? `The working file is replaced with ${LABELS[confirm].title} — ${LABELS[confirm].detail} — and staged as the resolution. The other side is discarded for this file.`
              : "That side deleted the file, so resolving to it removes the file and stages the deletion."}
          </p>
          <div className="editor-actions">
            <Button disabled={applying} onClick={() => setConfirm(null)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              disabled={applying}
              onClick={() => void resolve(confirm)}
            >
              {conflict[confirm] ? `Use ${LABELS[confirm].title}` : "Delete"}
            </Button>
          </div>
        </Modal>
      )}
    </section>
  );
}
