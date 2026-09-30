import { useCallback, useEffect, useId, useRef, useState } from "react";
import { ArrowUp, File, Folder, Link2 } from "lucide-react";
import { desktop } from "../api/desktop";
import { type FileEntry, parentPath } from "../domain/files";
import { Modal } from "./Editors";
import { Button, Input } from "./controls";
import "./folderChooser.css";

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
      className="git-folder-dialog"
    >
      <div className="git-folder-body">
        <div className="git-folder-toolbar">
          <Button
            size="icon"
            aria-label="Parent folder"
            disabled={!path || path === "/"}
            onClick={() => go(parentPath(path))}
          >
            <ArrowUp size={14} aria-hidden="true" />
          </Button>
          <form
            className="git-folder-form"
            onSubmit={(event) => {
              event.preventDefault();
              if (pathInput.trim()) go(pathInput.trim());
            }}
          >
            <label className="git-folder-label" htmlFor={pathId}>
              Server path
            </label>
            <Input
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
        <div className="git-folder-crumbs">
          <span className="git-folder-current" title={path}>
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
          <p className="git-folder-error" role="alert">
            Cannot open {attempted}: {error}
          </p>
        )}
        <div
          className="git-folder-list"
          role="region"
          aria-label="Folders"
          aria-busy={loading}
        >
          <ul>
            {folders.map((entry) => (
              <li key={entry.path} className="git-folder-row">
                <button
                  type="button"
                  className="git-folder-name"
                  title={entry.path}
                  onClick={() => go(entry.path)}
                >
                  <Folder size={13} aria-hidden="true" />
                  <span>{entry.name}</span>
                </button>
                <Button
                  variant="ghost"
                  disabled={blocked}
                  aria-label={`Choose ${entry.name}`}
                  onClick={() => void choose(entry.path)}
                >
                  Choose
                </Button>
              </li>
            ))}
            {rest.map((entry) => (
              <li
                key={entry.path}
                className="git-folder-row git-folder-other"
                title={entry.path}
              >
                <span className="git-folder-name">
                  {entry.kind === "symlink" ? (
                    <Link2 size={13} aria-hidden="true" />
                  ) : (
                    <File size={13} aria-hidden="true" />
                  )}
                  <span>{entry.name}</span>
                </span>
                <small>{entry.kind === "symlink" ? "Link" : "File"}</small>
              </li>
            ))}
          </ul>
          {!folders.length && (
            <p className="git-folder-empty" role="status">
              {loading
                ? "Reading folder…"
                : path
                  ? "No subfolders here."
                  : "Choose a folder."}
            </p>
          )}
        </div>
        {loading && !!folders.length && (
          <p className="git-folder-note" role="status">
            Reading folder…
          </p>
        )}
      </div>
      {choice ? (
        <div className="git-folder-resolve" role="group" aria-label="Which one">
          <p id={noticeId}>
            You picked {choice.chosen}; its repository root is {choice.root}.
          </p>
          <div className="git-folder-actions">
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
        <div className="git-folder-actions">
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
