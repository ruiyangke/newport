export interface FileEntry {
  name: string;
  path: string;
  kind: "directory" | "file" | "symlink" | "other";
  size: number | null;
  modified: number | null;
  permissions: string | null;
}
export interface FileListing {
  path: string;
  entries: FileEntry[];
}
export interface FilePreview {
  kind: "text" | "image" | "pdf";
  content: string;
  mime: string;
}
export interface FileProgress {
  name: string;
  completed: number;
  total: number | null;
}
export const parentPath = (path: string) =>
  path.replace(/\/+$/, "").split("/").slice(0, -1).join("/") || "/";
export function fileSize(bytes: number | null) {
  if (bytes === null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const unit = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), 4);
  return `${(bytes / 1024 ** unit).toFixed(1)} ${["B", "KiB", "MiB", "GiB", "TiB"][unit]}`;
}
