import { GitError } from "./GitForm";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { ArrowUp } from "lucide-react";
import { desktop } from "../api/desktop";
import { type FileEntry, parentPath } from "../domain/files";
import { Modal } from "./Editors";
import { Button, Input } from "./controls";
import { FolderEntryRow } from "./FolderEntryRow";

/** A directory the user picked next to the repository root it turned out to be in. */
interface RootChoice {
  chosen: string;
  root: string;
}

/**
 * Browses server directories so a repository can be picked instead of typed.
 * The parent owns what happens with the answer: this dialog only reports the
 * directory, and asks `resolveRoot` — when it is supplied — whether the pick
 * sits inside a repository so the user can say which one they meant.
 */
export function GitFolderChooser({
  serverId,
  open,
  initialPath,
  title = "Choose a folder",
  busy = false,
  onCancel,
  onChoose,
  resolveRoot,
}: {
  serverId: string;
  open: boolean;
  initialPath?: string;
  title?: string;
  busy?: boolean;
  onCancel: () => void;
  onChoose: (path: string) => void;
  resolveRoot?: (path: string) => Promise<string | null>;
}) {
  const [path, setPath] = useState("");
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [pathInput, setPathInput] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [attempted, setAttempted] = useState("");
  const [hidden, setHidden] = useState(false);
  const [choice, setChoice] = useState<RootChoice | null>(null);
  const [resolving, setResolving] = useState(false);
  // A listing that was superseded must never land on top of a newer one, so
  // every request carries a ticket and the late answer is dropped.
  const navigation = useRef(0);
  const pending = useRef<string | null>(null);
  const mounted = useRef(false);
  const start = useRef(initialPath);
  const pathId = useId();
  const noticeId = useId();
  useEffect(() => {
    start.current = initialPath;
  }, [initialPath]);

  const cancel = useCallback((operation: string) => {
    void desktop("files_cancel", { operation }).catch(() => {});
  }, []);
  const navigate = useCallback(
    async (target: string) => {
      const superseded = pending.current;
      if (superseded) cancel(superseded);
      const ticket = ++navigation.current;
      const operation = crypto.randomUUID();
      pending.current = operation;
      setAttempted(target);
      setLoading(true);
      setError("");
      try {
        const listing = await desktop("files_list", {
          id: serverId,
          operation,
          path: target,
        });
        if (!mounted.current || ticket !== navigation.current) return;
        setPath(listing.path);
        setPathInput(listing.path);
        setEntries(listing.entries);
      } catch (reason) {
        // The previous listing stays on screen; only the error is new.
        if (mounted.current && ticket === navigation.current)
          setError(String(reason));
      } finally {
        if (pending.current === operation) pending.current = null;
        if (mounted.current && ticket === navigation.current) setLoading(false);
      }
    },
    [serverId, cancel],
  );
  useEffect(() => {
    if (!open) return;
    mounted.current = true;
    const sequence = navigation;
    const inFlight = pending;
    void navigate(start.current ?? ".");
    return () => {
      mounted.current = false;
      sequence.current++;
      if (inFlight.current) cancel(inFlight.current);
      inFlight.current = null;
    };
  }, [open, navigate, cancel]);

  if (!open) return null;

  const go = (target: string) => {
    setChoice(null);
    void navigate(target);
  };
  const blocked = busy || resolving;
  const visible = entries.filter(
    (entry) => hidden || !entry.name.startsWith("."),
  );
  const folders = visible.filter((entry) => entry.kind === "directory");
  const rest = visible.filter((entry) => entry.kind !== "directory");
  async function choose(target: string) {
    if (!resolveRoot) {
      onChoose(target);
      return;
    }
    setResolving(true);
    setError("");
    try {
      const root = await resolveRoot(target);
      if (!mounted.current) return;
      if (root && root !== target) setChoice({ chosen: target, root });
      else onChoose(target);
    } catch (reason) {
      if (mounted.current) setError(String(reason));
    } finally {
      if (mounted.current) setResolving(false);
    }
  }
  return (
    <Modal
      title={title}
      busy={blocked}
      onClose={onCancel}
    >
      <div
        data-slot="git-folder-body"
        className="flex min-w-0 flex-col gap-[10px] px-[24px] pt-[4px] pb-[8px]"
      >
        <div
          data-slot="git-folder-toolbar"
          className="flex items-end gap-[8px]"
        >
          <Button
            size="icon"
            aria-label="Parent folder"
            disabled={!path || path === "/"}
            onClick={() => go(parentPath(path))}
          >
            <ArrowUp size={14} aria-hidden="true" />
          </Button>
          <form
            data-slot="git-folder-form"
            className="flex min-w-0 flex-1 flex-wrap items-center gap-[8px]"
            onSubmit={(event) => {
              event.preventDefault();
              if (pathInput.trim()) go(pathInput.trim());
            }}
          >
            <label
              data-slot="git-folder-label"
              className="whitespace-nowrap text-[11px] font-medium text-[#626b79] dark:text-muted-foreground"
              htmlFor={pathId}
            >
              Server path
            </label>
            <Input
              className="h-[28px] min-w-[140px] flex-1 rounded-[6px] text-[12px]"
              id={pathId}
              value={pathInput}
              placeholder="/srv/projects"
              spellCheck={false}
              autoComplete="off"
              onChange={(event) => setPathInput(event.target.value)}
            />
            <Button type="submit" disabled={!pathInput.trim()}>
              Go
            </Button>
          </form>
        </div>
        <div
          data-slot="git-folder-crumbs"
          className="flex min-w-0 items-center justify-between gap-[8px]"
        >
          <span
            dir="rtl"
            data-slot="git-folder-current"
            className="truncate text-left font-mono text-[12px] text-[#626b79] dark:text-muted-foreground"
            title={path}
          >
            <bdi>{path || "No folder open yet."}</bdi>
          </span>
          <Button
            variant="ghost"
            aria-pressed={hidden}
            onClick={() => setHidden(!hidden)}
          >
            Hidden folders
          </Button>
        </div>
        {error && (
          <GitError
            data-slot="git-folder-error"
            className="m-0 wrap-anywhere text-(--danger)"
            role="alert"
          >
            Cannot open {attempted}: {error}
          </GitError>
        )}
        <div
          data-slot="git-folder-list"
          className="h-[260px] overflow-y-auto rounded-[6px] border border-[#e1e4e9] bg-white dark:border-border dark:bg-card"
          role="region"
          aria-label="Folders"
          aria-busy={loading}
        >
          <ul className="m-0 list-none p-0">
            {[...folders, ...rest].map((entry) => (
              <FolderEntryRow
                key={entry.path}
                entry={entry}
                disabled={blocked}
                onOpen={go}
                onChoose={(path) => void choose(path)}
              />
            ))}
          </ul>
          {!folders.length && (
            <p
              data-slot="git-folder-empty"
              className="m-0 px-[8px] py-[10px] text-[12px] text-[#626b79] dark:text-muted-foreground"
              role="status"
            >
              {loading
                ? "Reading folder…"
                : path
                  ? "No subfolders here."
                  : "Choose a folder."}
            </p>
          )}
        </div>
        {loading && !!folders.length && (
          <p
            data-slot="git-folder-note"
            className="m-0 pb-[2px] text-[11px] text-[#626b79] dark:text-muted-foreground"
            role="status"
          >
            Reading folder…
          </p>
        )}
      </div>
      {choice ? (
        <div
          data-slot="git-folder-resolve"
          className="flex flex-col gap-[8px] px-[24px]"
          role="group"
          aria-label="Which one"
        >
          <p
            id={noticeId}
            className="m-0 rounded-[6px] border border-[#e1e4e9] bg-[#f8f9fb] px-[10px] py-[8px] text-[12px] leading-[18px] wrap-anywhere dark:border-border dark:bg-muted"
          >
            You picked {choice.chosen}; its repository root is {choice.root}.
          </p>
          <div
            data-slot="git-folder-actions"
            className="flex flex-wrap justify-end gap-[8px] pb-[20px]"
          >
            <Button disabled={blocked} onClick={() => setChoice(null)}>
              Back
            </Button>
            <Button
              disabled={blocked}
              onClick={() => onChoose(choice.chosen)}
              title={choice.chosen}
            >
              Use the folder I picked
            </Button>
            <Button
              variant="default"
              disabled={blocked}
              onClick={() => onChoose(choice.root)}
              title={choice.root}
            >
              Use repository root
            </Button>
          </div>
        </div>
      ) : (
        <div
          data-slot="git-folder-actions"
          className="flex flex-wrap justify-end gap-[8px] px-[24px] pt-[8px] pb-[20px]"
        >
          <Button disabled={blocked} onClick={onCancel}>
            Cancel
          </Button>
          <Button
            variant="default"
            loading={resolving}
            disabled={!path || blocked}
            onClick={() => void choose(path)}
          >
            Choose this folder
          </Button>
        </div>
      )}
    </Modal>
  );
}
