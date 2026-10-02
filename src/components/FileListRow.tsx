import { Download, FolderGit2 } from "lucide-react";
import { type FileEntry, fileSize } from "../domain/files";
import { FileIcon } from "./FileIcon";
import { Button } from "./controls";
import { TableRow } from "./ui/table";
import { FileTableCell as TableCell } from "./FileTable";
export function FileListRow({
  entry,
  busy,
  transferring,
  savingProject,
  onOpenFolder,
  onPreview,
  onOpenProject,
  onDownload,
}: {
  entry: FileEntry;
  busy: boolean;
  transferring: boolean;
  savingProject: string | null;
  onOpenFolder: (path: string) => void;
  onPreview: (entry: FileEntry) => void;
  onOpenProject: (path: string) => void;
  onDownload: (path: string) => void;
}) {
  return (
    <TableRow>
      <TableCell>
        <button
          data-slot="file-name"
          className="flex w-full min-w-0 cursor-pointer items-center gap-[7px] border-0 border-current bg-transparent px-0 py-[5px] text-left text-inherit focus-visible:rounded-[5px] focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-ring [&_svg]:text-muted-foreground"
          disabled={busy}
          title={entry.name}
          onClick={() =>
            entry.kind === "directory"
              ? onOpenFolder(entry.path)
              : onPreview(entry)
          }
        >
          <FileIcon name={entry.name} kind={entry.kind} />
          <span className="truncate">{entry.name}</span>
          {entry.kind === "symlink" && <small>Link</small>}
        </button>
      </TableCell>
      <TableCell column="size">
        {entry.kind === "directory" ? "—" : fileSize(entry.size)}
      </TableCell>
      <TableCell column="modified">
        {entry.modified
          ? new Date(entry.modified * 1000).toLocaleDateString()
          : "—"}
      </TableCell>
      <TableCell column="actions">
        <div
          data-slot="files-project-actions"
          className="flex items-center justify-end gap-[2px] [&_[aria-busy=true]]:text-primary"
        >
          {entry.kind === "directory" && (
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Open ${entry.name} as project`}
              loading={savingProject === entry.path}
              disabled={busy || !!savingProject}
              onClick={() => onOpenProject(entry.path)}
            >
              {savingProject === entry.path ? null : <FolderGit2 size={14} />}
            </Button>
          )}
          {entry.kind !== "directory" && entry.kind !== "other" && (
            <Button
              variant="ghost"
              size="icon"
              aria-label={`Download ${entry.name}`}
              disabled={transferring || busy}
              onClick={() => onDownload(entry.path)}
            >
              <Download size={14} />
            </Button>
          )}
        </div>
      </TableCell>
    </TableRow>
  );
}
