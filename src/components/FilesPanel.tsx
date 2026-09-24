import { useQuery, useQueryClient } from "@tanstack/react-query";
import { keys, useServerScope } from "../query/keys";
import { fileListingOptions } from "../query/files";
import { FilePreviewDialog } from "./FilePreviewDialog";
import { FileIcon } from "./FileIcon";
import { memo, useCallback, useEffect, useRef, useState } from "react";
import {
  ArrowUp,
  ChevronDown,
  ChevronRight,
  Download,
  Home,
  RefreshCw,
  Upload,
} from "lucide-react";
import { toast } from "sonner";
import { desktop } from "../api/desktop";
import {
  type FileEntry,
  type FileListing,
  type FilePreview,
  type FileProgress,
  fileSize,
  parentPath,
} from "../domain/files";
import { type Server } from "../types";
import { Button, Input } from "./controls";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./ui/table";
import "./files.css";
const PAGE_SIZE = 100;
type Load = (path: string) => Promise<FileListing>;

function FolderBranch({
  path,
  label,
  current,
  load,
  navigate,
  hidden,
  depth = 0,
}: {
  path: string;
  label: string;
  current: string;
  load: Load;
  navigate: (path: string) => void;
  hidden: boolean;
  depth?: number;
}) {
  const [open, setOpen] = useState(false);
  const [children, setChildren] = useState<FileEntry[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function expand() {
    if (open) {
      setOpen(false);
      return;
    }
    setOpen(true);
    setBusy(true);
    setError("");
    try {
      setChildren(
        (await load(path)).entries.filter(
          (entry) => entry.kind === "directory",
        ),
      );
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }
  return (
    <li>
      <div className="folder-branch" data-current={current === path}>
        <Button
          variant="ghost"
          size="icon"
          aria-label={`${open ? "Collapse" : "Expand"} ${label}`}
          aria-expanded={open}
          disabled={busy || depth >= 16}
          onClick={() => void expand()}
        >
          {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        </Button>
        <button
          className="folder-name"
          aria-current={current === path ? "location" : undefined}
          onClick={() => navigate(path)}
          title={path}
        >
          <FileIcon name={label} kind="directory" open={open} size={14} />
          <span>{label}</span>
        </button>
      </div>
      {open && (
        <ul>
          {busy && (
            <li className="file-tree-note" role="status">
              Loading…
            </li>
          )}
          {error && (
            <li className="file-tree-note" role="alert">
              {error}
              <button
                onClick={() => {
                  setOpen(false);
                  setError("");
                }}
              >
                Close
              </button>
            </li>
          )}
          {!busy &&
            !error &&
            children
              .filter((entry) => hidden || !entry.name.startsWith("."))
              .map((entry) => (
                <FolderBranch
                  key={entry.path}
                  path={entry.path}
                  label={entry.name}
                  current={current}
                  load={load}
                  navigate={navigate}
                  hidden={hidden}
                  depth={depth + 1}
                />
              ))}
          {!busy &&
            !error &&
            !children.some(
              (entry) => hidden || !entry.name.startsWith("."),
            ) && <li className="file-tree-note">No subfolders</li>}
        </ul>
      )}
    </li>
  );
}

export const FilesPanel = memo(function FilesPanel({
  server,
}: {
  server: Server;
}) {
  const cache = useQueryClient();
  const scope = useServerScope(server.id);
  const [listingPath, setListingPath] = useState(".");
  const { data: listing } = useQuery({
    ...fileListingOptions(scope, listingPath),
    enabled: false,
  });
  const [pathInput, setPathInput] = useState("");
  const [home, setHome] = useState("");
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [hidden, setHidden] = useState(false);
  const [page, setPage] = useState(0);
  const [tree, setTree] = useState(true);
  const [treeVersion, setTreeVersion] = useState(0);
  const [preview, setPreview] = useState<{
    entry: FileEntry;
    data?: FilePreview;
    error?: string;
  } | null>(null);
  const [transfer, setTransfer] = useState<{
    operation: string;
    direction: string;
    progress: FileProgress | null;
  } | null>(null);
  const operations = useRef(new Set<string>());
  const navigation = useRef(0);
  const latestPath = useRef("");
  const attemptedPath = useRef(".");
  const listElement = useRef<HTMLDivElement>(null);
  const previewOperation = useRef<string | null>(null);
  const transferOperation = transfer?.operation;
  const previewSequence = useRef(0);
  const mounted = useRef(false);
  const cancel = useCallback((operation: string) => {
    void desktop("files_cancel", { operation }).catch(() => {});
  }, []);
  const request = useCallback(
    async <T,>(work: (operation: string) => Promise<T>) => {
      const operation = crypto.randomUUID();
      operations.current.add(operation);
      try {
        return await work(operation);
      } finally {
        operations.current.delete(operation);
      }
    },
    [],
  );
  const load = useCallback<Load>(
    (path) => cache.fetchQuery(fileListingOptions(scope, path)),
    [cache, scope],
  );
  useEffect(
    () => () => {
      void cache.cancelQueries({ queryKey: [...keys.server(scope), "files"] });
    },
    [cache, scope],
  );
  const navigate = useCallback(
    async (path: string) => {
      const sequence = ++navigation.current;
      attemptedPath.current = path;
      setBusy(true);
      setError("");
      try {
        const result = await load(path);
        if (!mounted.current || sequence !== navigation.current) return;
        const sameFolder = latestPath.current === result.path;
        latestPath.current = result.path;
        cache.setQueryData(keys.files(scope, result.path), result);
        setListingPath(result.path);
        setPathInput(result.path);
        if (!sameFolder) {
          setPage(0);
          setQuery("");
        }
        if (path === ".") setHome(result.path);
      } catch (reason) {
        if (mounted.current && sequence === navigation.current)
          setError(String(reason));
      } finally {
        if (mounted.current && sequence === navigation.current) setBusy(false);
      }
    },
    [load, cache, scope],
  );
  useEffect(() => {
    mounted.current = true;
    void navigate(".");
    const active = operations.current;
    const navigationSequence = navigation;
    const previews = previewSequence;
    return () => {
      mounted.current = false;
      navigationSequence.current++;
      previews.current++;
      active.forEach(cancel);
    };
  }, [navigate, cancel]);
  useEffect(() => {
    if (!transferOperation) return;
    const operation = transferOperation;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = () => {
      void desktop("files_progress", { operation })
        .then((progress) => {
          if (active)
            setTransfer((previous) =>
              previous?.operation === operation
                ? { ...previous, progress }
                : previous,
            );
        })
        .catch(() => {})
        .finally(() => {
          if (active) timer = setTimeout(poll, 400);
        });
    };
    poll();
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [transferOperation]);
  function closePreview() {
    if (previewOperation.current) cancel(previewOperation.current);
    previewSequence.current++;
    setPreview(null);
  }
  async function openPreview(entry: FileEntry) {
    const sequence = ++previewSequence.current;
    setPreview({ entry });
    try {
      const data = await request((operation) => {
        previewOperation.current = operation;
        return desktop("files_preview", {
          id: server.id,
          operation,
          path: entry.path,
        });
      });
      if (mounted.current && sequence === previewSequence.current)
        setPreview({ entry, data });
    } catch (reason) {
      if (mounted.current && sequence === previewSequence.current)
        setPreview({ entry, error: String(reason) });
    }
  }
  async function transferFile(direction: "upload" | "download", path: string) {
    if (transfer) return;
    const operation = crypto.randomUUID();
    operations.current.add(operation);
    setTransfer({ operation, direction, progress: null });
    try {
      const result = await desktop(
        direction === "upload" ? "files_upload" : "files_download",
        { id: server.id, operation, path },
      );
      if (mounted.current && result) {
        toast.success(
          direction === "upload" ? "File uploaded." : "File downloaded.",
        );
        if (direction === "upload") {
          // A pre-upload listing must not satisfy the post-upload refresh.
          await cache.cancelQueries({
            queryKey: keys.files(scope, path),
            exact: true,
          });
          if (!mounted.current) return;
          setTreeVersion((value) => value + 1);
          if (latestPath.current === path) void navigate(path);
        }
      }
    } catch (reason) {
      if (mounted.current && !String(reason).includes("cancelled"))
        toast.error(String(reason));
    } finally {
      operations.current.delete(operation);
      if (mounted.current) setTransfer(null);
    }
  }
  const entries = (listing?.entries ?? []).filter(
    (entry) =>
      (hidden || !entry.name.startsWith(".")) &&
      entry.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()),
  );
  const pages = Math.max(1, Math.ceil(entries.length / PAGE_SIZE));
  const activePage = Math.min(page, pages - 1);
  const visible = entries.slice(
    activePage * PAGE_SIZE,
    (activePage + 1) * PAGE_SIZE,
  );
  useEffect(() => {
    if (listElement.current) listElement.current.scrollTop = 0;
  }, [activePage, query, hidden, listing?.path]);
  const go = (path: string) => {
    void navigate(path);
  };
  return (
    <section className="files-workspace" aria-label="Remote files">
      <div className="files-toolbar">
        <Button
          size="icon"
          aria-label="Home folder"
          disabled={busy}
          onClick={() => go(home || ".")}
        >
          <Home size={14} />
        </Button>
        <Button
          size="icon"
          aria-label="Parent folder"
          disabled={busy || !listing || listing.path === "/"}
          onClick={() => go(parentPath(listing!.path))}
        >
          <ArrowUp size={14} />
        </Button>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (pathInput.trim()) go(pathInput);
          }}
        >
          <Input
            aria-label="Remote path"
            value={pathInput}
            placeholder="Go to folder…"
            onChange={(event) => setPathInput(event.target.value)}
          />
        </form>
        <Button
          size="icon"
          aria-label="Refresh folder"
          disabled={busy || !listing}
          onClick={() => {
            setTreeVersion((value) => value + 1);
            go(listing!.path);
          }}
        >
          <RefreshCw size={14} className={busy ? "spin" : ""} />
        </Button>
        <Button
          disabled={!!transfer || busy || !listing}
          onClick={() => void transferFile("upload", listing!.path)}
        >
          <Upload size={14} />
          Upload
        </Button>
      </div>
      <div className="files-filters">
        <Button
          variant="ghost"
          aria-pressed={tree}
          onClick={() => setTree(!tree)}
        >
          Folders
        </Button>
        <Input
          aria-label="Filter files"
          placeholder="Filter this folder…"
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setPage(0);
          }}
        />
        <Button
          variant="ghost"
          aria-pressed={hidden}
          onClick={() => {
            setHidden(!hidden);
            setPage(0);
          }}
        >
          Hidden files
        </Button>
      </div>
      {error && (
        <div className="file-error" role="alert">
          Cannot open {attemptedPath.current}: {error}{" "}
          <Button onClick={() => go(attemptedPath.current)}>Retry</Button>
        </div>
      )}
      <div className={`files-layout${tree ? "" : " files-no-tree"}`}>
        {tree && (
          <nav className="file-tree" aria-label="Folder tree">
            <ul key={treeVersion}>
              {home && (
                <FolderBranch
                  path={home}
                  label="Home"
                  current={listing?.path || ""}
                  load={load}
                  navigate={go}
                  hidden={hidden}
                />
              )}
              <FolderBranch
                path="/"
                label="Server"
                current={listing?.path || ""}
                load={load}
                navigate={go}
                hidden={hidden}
              />
            </ul>
          </nav>
        )}
        <div
          className="file-list"
          aria-busy={busy}
          role="region"
          aria-label="Files in current folder"
          tabIndex={0}
          ref={listElement}
        >
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead className="file-size">Size</TableHead>
                <TableHead className="file-modified">Modified</TableHead>
                <TableHead>
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {visible.map((entry) => (
                <TableRow key={entry.path}>
                  <TableCell>
                    <button
                      className="file-name"
                      disabled={busy}
                      title={entry.name}
                      onClick={() =>
                        entry.kind === "directory"
                          ? go(entry.path)
                          : void openPreview(entry)
                      }
                    >
                      <FileIcon name={entry.name} kind={entry.kind} />
                      <span>{entry.name}</span>
                      {entry.kind === "symlink" && <small>Link</small>}
                    </button>
                  </TableCell>
                  <TableCell className="file-size">
                    {entry.kind === "directory" ? "—" : fileSize(entry.size)}
                  </TableCell>
                  <TableCell className="file-modified">
                    {entry.modified
                      ? new Date(entry.modified * 1000).toLocaleDateString()
                      : "—"}
                  </TableCell>
                  <TableCell>
                    {entry.kind !== "directory" && entry.kind !== "other" && (
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label={`Download ${entry.name}`}
                        disabled={!!transfer || busy}
                        onClick={() =>
                          void transferFile("download", entry.path)
                        }
                      >
                        <Download size={14} />
                      </Button>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          {!visible.length && (
            <p className="file-empty" role="status">
              {busy
                ? "Reading folder…"
                : !listing
                  ? "Choose a folder."
                  : query
                    ? "No matching files."
                    : "Empty folder."}
            </p>
          )}
        </div>
      </div>
      <footer className="files-footer">
        <span role="status">
          {busy
            ? "Reading folder…"
            : `${entries.length} ${entries.length === 1 ? "item" : "items"}`}
        </span>
        {pages > 1 && (
          <div>
            <Button
              variant="ghost"
              disabled={activePage === 0}
              onClick={() => setPage(activePage - 1)}
            >
              Previous
            </Button>
            <span>
              {activePage + 1} / {pages}
            </span>
            <Button
              variant="ghost"
              disabled={activePage + 1 >= pages}
              onClick={() => setPage(activePage + 1)}
            >
              Next
            </Button>
          </div>
        )}
      </footer>
      {transfer && (
        <div className="file-transfer" role="status">
          <div>
            <strong>
              {transfer.direction === "upload" ? "Uploading" : "Downloading"}
              {transfer.progress?.name ? ` ${transfer.progress.name}` : "…"}
            </strong>
            <span>
              {transfer.progress?.name
                ? `${fileSize(transfer.progress.completed)}${transfer.progress.total !== null ? ` of ${fileSize(transfer.progress.total)}` : ""}`
                : "Choose a file in the file dialog…"}
            </span>
            <progress
              aria-label="File transfer"
              value={
                transfer.progress?.total
                  ? transfer.progress.completed
                  : undefined
              }
              max={transfer.progress?.total || 1}
            />
          </div>
          <Button onClick={() => cancel(transfer.operation)}>Cancel</Button>
        </div>
      )}
      {preview && (
        <FilePreviewDialog
          key={preview.entry.path}
          {...preview}
          busy={!!transfer}
          onClose={closePreview}
          onDownload={() => {
            void transferFile("download", preview.entry.path);
            closePreview();
          }}
        />
      )}
    </section>
  );
});
