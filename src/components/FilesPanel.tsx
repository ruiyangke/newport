import { FileListRow } from "./FileListRow";
import { useWorkspaceState } from "../state/workspace";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { keys, useServerScope } from "../query/keys";
import { fileListingOptions } from "../query/files";
import { FilePreviewDialog } from "./FilePreviewDialog";
import { FileIcon } from "./FileIcon";
import { memo, useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import {
  ArrowUp,
  ChevronDown,
  ChevronRight,
  FolderGit2,
  Home,
  RefreshCw,
  Upload,
} from "lucide-react";
import { toast } from "sonner";
import { desktop } from "../api/desktop";
import { GitProjects } from "../api/gitProjects";
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
import { Table, TableBody, TableHeader, TableRow } from "./ui/table";
import { cn } from "../lib/utils";
import { FileTableHead as TableHead } from "./FileTable";
const PAGE_SIZE = 100;
type Load = (path: string) => Promise<FileListing>;

/** The message the failed command actually returned, never a paraphrase. */
function errorText(reason: unknown) {
  return reason && typeof reason === "object" && "message" in reason
    ? String(reason.message)
    : String(reason);
}
/** A bookmark needs a name; the folder's own name is the one the user just read. */
function projectLabel(path: string) {
  const trimmed = path.replace(/\/+$/, "");
  return trimmed.split("/").pop() || trimmed || path;
}

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
      <div
        data-slot="folder-branch"
        className="flex items-center rounded-[5px] data-[current=true]:bg-accent"
        data-current={current === path}
      >
        <Button
          variant="ghost"
          size="icon"
          className="w-[20px] min-w-[20px] p-0"
          aria-label={`${open ? "Collapse" : "Expand"} ${label}`}
          aria-expanded={open}
          disabled={busy || depth >= 16}
          onClick={() => void expand()}
        >
          {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        </Button>
        <button
          data-slot="folder-name"
          className="flex h-[28px] min-w-0 flex-1 cursor-pointer items-center gap-[6px] border-0 border-current bg-transparent text-left text-inherit focus-visible:rounded-[5px] focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-ring [&_svg]:text-muted-foreground"
          aria-current={current === path ? "location" : undefined}
          onClick={() => navigate(path)}
          title={path}
        >
          <FileIcon name={label} kind="directory" open={open} size={14} />
          <span className="truncate">{label}</span>
        </button>
      </div>
      {open && (
        <ul className="m-0 list-none py-0 pr-0 pl-[12px]">
          {busy && (
            <li
              data-slot="file-tree-note"
              className="p-[6px] text-[11px] text-muted-foreground wrap-anywhere"
              role="status"
            >
              Loading…
            </li>
          )}
          {error && (
            <li
              data-slot="file-tree-note"
              className="p-[6px] text-[11px] text-muted-foreground wrap-anywhere"
              role="alert"
            >
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
            ) && (
              <li
                data-slot="file-tree-note"
                className="p-[6px] text-[11px] text-muted-foreground wrap-anywhere"
              >
                No subfolders
              </li>
            )}
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
  const [listingPath, setListingPath] = useWorkspaceState(
    server.id,
    "files.path",
  );
  const { data: listing } = useQuery({
    ...fileListingOptions(scope, listingPath),
    enabled: false,
  });
  const [pathInput, setPathInput] = useWorkspaceState(server.id, "files.input");
  const [home, setHome] = useWorkspaceState(server.id, "files.home");
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");
  const [query, setQuery] = useWorkspaceState(server.id, "files.query");
  const [hidden, setHidden] = useWorkspaceState(server.id, "files.hidden");
  const [page, setPage] = useWorkspaceState(server.id, "files.page");
  const [tree, setTree] = useWorkspaceState(server.id, "files.tree");
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
  const [savingProject, setSavingProject] = useState<string | null>(null);
  const [projectError, setProjectError] = useState<{
    path: string;
    message: string;
  } | null>(null);
  const routerNavigate = useNavigate();
  const projectWorkspace = useRef<GitProjects | null>(null);
  const projectSequence = useRef(0);
  const operations = useRef(new Set<string>());
  const navigation = useRef(0);
  const latestPath = useRef(listingPath);
  const initialPath = useRef(listingPath);
  const [attemptedPath, setAttemptedPath] = useState(listingPath);
  const listElement = useRef<HTMLDivElement>(null);
  const previewOperation = useRef<string | null>(null);
  const transferOperation = transfer?.operation;
  const activeTransfer = useRef<string | null>(null);
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
  // A bookmark save that outlives this connection scope belongs to a workspace
  // the user has left: drop its session and let its sequence go stale so it
  // cannot navigate or write state afterwards.
  useEffect(() => {
    const sequence = projectSequence;
    const workspace = projectWorkspace;
    return () => {
      sequence.current++;
      const open = workspace.current;
      workspace.current = null;
      void open?.dispose().catch(() => undefined);
    };
  }, [scope.connection]);
  const navigate = useCallback(
    async (path: string) => {
      const sequence = ++navigation.current;
      setAttemptedPath(path);
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
    [
      load,
      cache,
      scope,
      setListingPath,
      setPathInput,
      setPage,
      setQuery,
      setHome,
    ],
  );
  useEffect(() => {
    mounted.current = true;
    void navigate(initialPath.current);
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
    if (previewOperation.current) cancel(previewOperation.current);
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
  /**
   * Save a project bookmark for a browsed folder, then hand over to Projects.
   * Files says nothing about what the folder holds: the save reports what the
   * server found, and Projects describes the repository once it opens it.
   */
  async function openAsProject(path: string) {
    if (projectWorkspace.current) return;
    const sequence = ++projectSequence.current;
    const workspace = new GitProjects(server.id);
    projectWorkspace.current = workspace;
    setSavingProject(path);
    setProjectError(null);
    try {
      await workspace.add(path, projectLabel(path));
      if (!mounted.current || sequence !== projectSequence.current) return;
      void routerNavigate(`/servers/${encodeURIComponent(server.id)}/projects`);
    } catch (reason) {
      // Nothing was bookmarked. Stay here and show what the save returned.
      if (mounted.current && sequence === projectSequence.current)
        setProjectError({ path, message: errorText(reason) });
    } finally {
      if (projectWorkspace.current === workspace)
        projectWorkspace.current = null;
      // Projects opens its own session; this one has no reader left.
      void workspace.dispose().catch(() => undefined);
      if (mounted.current && sequence === projectSequence.current)
        setSavingProject(null);
    }
  }
  async function transferFile(direction: "upload" | "download", path: string) {
    if (activeTransfer.current) return;
    const operation = crypto.randomUUID();
    activeTransfer.current = operation;
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
      if (activeTransfer.current === operation) {
        activeTransfer.current = null;
        if (mounted.current) setTransfer(null);
      }
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
    <section
      data-slot="files-workspace"
      className="flex min-h-0 flex-1 flex-col text-foreground"
      aria-label="Remote files"
    >
      <div
        data-slot="files-toolbar"
        className="flex shrink-0 items-center gap-[8px] px-[16px] py-[12px] max-[800px]:gap-[4px] max-[800px]:px-[10px]"
      >
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
          className="min-w-0 flex-1"
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
          <RefreshCw
            size={14}
            className={
              busy
                ? "animate-[spin_0.8s_linear_infinite] motion-reduce:animate-none"
                : undefined
            }
          />
        </Button>
        <Button
          disabled={!!transfer || busy || !listing}
          onClick={() => void transferFile("upload", listing!.path)}
        >
          <Upload size={14} />
          Upload
        </Button>
        <Button
          size="icon"
          aria-label="Open this folder as project"
          loading={!!listing && savingProject === listing.path}
          disabled={busy || !listing || !!savingProject}
          onClick={() => void openAsProject(listing!.path)}
        >
          {listing && savingProject === listing.path ? null : (
            <FolderGit2 size={14} />
          )}
        </Button>
      </div>
      <div
        data-slot="files-filters"
        className="flex shrink-0 items-center gap-[8px] border-t border-border px-[16px] py-[7px] max-[800px]:gap-[4px] max-[800px]:px-[10px]"
      >
        <Button
          variant="ghost"
          className="aria-pressed:bg-accent"
          aria-pressed={tree}
          onClick={() => setTree(!tree)}
        >
          Folders
        </Button>
        <Input
          className="w-[min(240px,45%)]"
          aria-label="Filter files"
          placeholder="Filter this folder…"
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setPage(0);
          }}
        />
        {query && (
          <Button
            variant="ghost"

            onClick={() => {
              setQuery("");
              setPage(0);
            }}
          >
            Clear filter
          </Button>
        )}
        <Button
          variant="ghost"
          className="ml-auto aria-pressed:bg-accent"
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
        <div
          data-slot="file-error"
          className="p-[10px] text-(--red) wrap-anywhere"
          role="alert"
        >
          Cannot open {attemptedPath}: {error}{" "}
          <Button onClick={() => go(attemptedPath)}>Retry</Button>
        </div>
      )}
      {projectError && (
        <div
          data-slot="files-project-error"
          className="flex items-center gap-[8px] border-t border-border bg-muted px-[16px] py-[8px] text-[12px] text-(--red) wrap-anywhere"
          role="alert"
        >
          <span className="min-w-0 flex-1">
            Cannot save a project for {projectError.path}:{" "}
            {projectError.message}
            <small className="block text-muted-foreground">
              No project was saved. Nothing has changed on the server.
            </small>
          </span>
          <Button onClick={() => setProjectError(null)}>Dismiss</Button>
        </div>
      )}
      <div
        data-slot="files-layout"
        className={cn(
          "grid min-h-0 flex-1 border-y border-border",
          tree
            ? "grid-cols-[180px_minmax(0,1fr)] max-[800px]:grid-cols-[140px_minmax(0,1fr)]"
            : "grid-cols-[minmax(0,1fr)]",
        )}
      >
        {tree && (
          <nav
            data-slot="file-tree"
            className="overflow-auto border-r border-border bg-(--native-toolbar) p-[8px]"
            aria-label="Folder tree"
          >
            <ul className="m-0 list-none p-0" key={treeVersion}>
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
          data-slot="file-list"
          className="min-w-0 overflow-auto focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
          aria-busy={busy}
          role="region"
          aria-label="Files in current folder"
          tabIndex={0}
          ref={listElement}
        >
          <Table className="table-fixed" containerClassName="overflow-visible">
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead column="size">Size</TableHead>
                <TableHead column="modified">Modified</TableHead>
                <TableHead column="actions">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {visible.map((entry) => (
                <FileListRow
                  key={entry.path}
                  entry={entry}
                  busy={busy}
                  transferring={!!transfer}
                  savingProject={savingProject}
                  onOpenFolder={go}
                  onPreview={(entry) => void openPreview(entry)}
                  onOpenProject={(path) => void openAsProject(path)}
                  onDownload={(path) => void transferFile("download", path)}
                />
              ))}
            </TableBody>
          </Table>
          {!visible.length && (
            <p
              data-slot="file-empty"
              className="px-[16px] py-[35px] text-center text-muted-foreground"
              role="status"
            >
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
      <footer
        data-slot="files-footer"
        className="flex min-h-[36px] shrink-0 items-center justify-between gap-[8px] px-[16px] py-[7px] text-[11px] text-muted-foreground"
      >
        <span role="status">
          {busy
            ? "Reading folder…"
            : `${entries.length} ${entries.length === 1 ? "item" : "items"}`}
        </span>
        {pages > 1 && (
          <div className="flex items-center gap-[6px]">
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
        <div
          data-slot="file-transfer"
          className="m-0 flex shrink-0 items-center gap-[14px] border-t border-border px-[16px] py-[10px]"
          role="status"
        >
          <div className="flex min-w-0 flex-1 flex-col gap-[3px]">
            <strong className="truncate font-medium">
              {transfer.direction === "upload" ? "Uploading" : "Downloading"}
              {transfer.progress?.name ? ` ${transfer.progress.name}` : "…"}
            </strong>
            <span className="text-[11px] text-muted-foreground">
              {transfer.progress?.name
                ? `${fileSize(transfer.progress.completed)}${transfer.progress.total !== null ? ` of ${fileSize(transfer.progress.total)}` : ""}`
                : "Choose a file in the file dialog…"}
            </span>
            <progress
              className="h-[5px] w-full accent-primary"
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
