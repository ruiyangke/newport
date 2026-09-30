import { useState } from "react";
import { Undo2 } from "lucide-react";
import type { GitStatus } from "../domain/gitResponses";
import type { GitWriteAction } from "../domain/git";
import { Button } from "./controls";
import { Modal } from "./Editors";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";

export function GitDiscardControl({
  entry,
  status,
  disabled,
  busy,
  error,
  compact = false,
  onAction,
}: {
  entry: GitStatus["entries"][number];
  status: GitStatus;
  disabled: boolean;
  busy: boolean;
  error: string;
  /** An icon button, for the diff header; the name is the same either way. */
  compact?: boolean;
  onAction: (action: GitWriteAction) => Promise<boolean>;
}) {
  const [selection, setSelection] = useState<{
    entry: typeof entry;
    snapshot: string;
  } | null>(null);
  const [source, setSource] = useState<"index" | "head">("index");
  const blocked =
    disabled ||
    !!status.metadata.integration ||
    status.entries.some((item) => item.conflicted);
  const stale =
    !!selection &&
    (selection.snapshot !== status.snapshot ||
      selection.entry.entryId !== entry.entryId);
  return (
    <>
      <Button
        disabled={blocked}
        {...(compact
          ? {
              size: "icon" as const,
              "aria-label": "Discard changes…",
              className:
                "h-[26px]! w-[28px] text-muted-foreground hover:text-destructive",
            }
          : {})}
        onClick={() => {
          setSelection({
            entry: structuredClone(entry),
            snapshot: status.snapshot,
          });
          setSource(entry.unstaged || entry.untracked ? "index" : "head");
        }}
      >
        {compact ? <Undo2 size={14} aria-hidden="true" /> : "Discard changes…"}
      </Button>
      {selection && (
        <Modal
          title="Discard file changes"
          busy={busy}
          onClose={() => setSelection(null)}
        >
          <div className="git-integration-confirm">
            <p>
              <strong>
                {selection.entry.path?.display ??
                  selection.entry.oldPath?.display}
              </strong>
            </p>
            {selection.entry.oldPath &&
              selection.entry.path &&
              selection.entry.oldPath.bytesB64 !==
                selection.entry.path.bytesB64 && (
                <p>Previous path: {selection.entry.oldPath.display}</p>
              )}
            <label>
              Restore from
              <Select
                value={source}
                onValueChange={(value) => setSource(value as "index" | "head")}
                disabled={busy}
              >
                <SelectTrigger aria-label="Restore from">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem
                    value="index"
                    disabled={
                      !selection.entry.unstaged && !selection.entry.untracked
                    }
                  >
                    Staged version
                  </SelectItem>
                  <SelectItem value="head">Last commit</SelectItem>
                </SelectContent>
              </Select>
            </label>
            <p>
              {source === "index"
                ? "Discard this file’s unstaged changes and keep its staged changes."
                : "Discard this file’s staged and unstaged changes, restoring the last committed version."}
            </p>
            <p>
              Files absent from the selected version will be deleted. There is
              no automatic undo for discarded changes.
            </p>
            {stale && (
              <p role="alert">
                The selection changed. Close this dialog and review the file
                again.
              </p>
            )}
            {error && <p role="alert">{error}</p>}
            <footer>
              <Button disabled={busy} onClick={() => setSelection(null)}>
                Cancel
              </Button>
              <Button
                variant="destructive"
                disabled={blocked || stale}
                loading={busy}
                onClick={async () => {
                  if (blocked || stale) return;
                  if (
                    await onAction({
                      kind: "discard",
                      entryIds: [selection.entry.entryId],
                      source,
                    })
                  )
                    setSelection(null);
                }}
              >
                Discard changes
              </Button>
            </footer>
          </div>
        </Modal>
      )}
    </>
  );
}
