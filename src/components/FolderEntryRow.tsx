import { File, Folder, Link2 } from "lucide-react";
import type { FileEntry } from "../domain/files";
import { Button } from "./controls";

export function FolderEntryRow({
  entry,
  disabled,
  onOpen,
  onChoose,
}: {
  entry: FileEntry;
  disabled?: boolean;
  onOpen: (path: string) => void;
  onChoose: (path: string) => void;
}) {
  const directory = entry.kind === "directory";
  const content = (
    <>
      {directory ? (
        <Folder size={13} className="text-[#3767c8]" aria-hidden="true" />
      ) : entry.kind === "symlink" ? (
        <Link2 size={13} aria-hidden="true" />
      ) : (
        <File size={13} aria-hidden="true" />
      )}
      <span className="truncate">{entry.name}</span>
    </>
  );
  return (
    <li
      data-slot="git-folder-row"
      title={entry.path}
      className="group/folder flex min-h-[28px] items-center gap-[6px] border-b border-b-[#e1e4e9] pr-[4px] pl-[6px] last:border-b-0 hover:bg-[#f8f9fb] dark:border-b-border dark:hover:bg-muted"
    >
      {directory ? (
        <button
          data-slot="git-folder-name"
          type="button"
          title={entry.path}
          onClick={() => onOpen(entry.path)}
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-[6px] border-0 border-current bg-transparent px-[2px] py-[4px] text-left text-[12px] text-[#252932] dark:text-foreground focus-visible:rounded-[5px] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[#3767c8]"
        >
          {content}
        </button>
      ) : (
        <span
          data-slot="git-folder-name"
          className="flex min-w-0 flex-1 items-center gap-[6px] px-[2px] py-[4px] text-[12px] text-[#626b79] dark:text-muted-foreground"
        >
          {content}
        </span>
      )}
      {directory ? (
        <Button
          variant="ghost"
          disabled={disabled}
          aria-label={`Choose ${entry.name}`}
          onClick={() => onChoose(entry.path)}
          className="h-[22px] rounded-[5px] px-[8px] py-0 text-[11px] opacity-0 group-hover/folder:opacity-100 focus-visible:opacity-100"
        >
          Choose
        </Button>
      ) : (
        <small className="pr-[6px] text-[11px] text-[#626b79] dark:text-muted-foreground">
          {entry.kind === "symlink" ? "Link" : "File"}
        </small>
      )}
    </li>
  );
}
