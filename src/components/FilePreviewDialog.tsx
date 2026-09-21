import { lazy, Suspense, useState } from "react";
import { Download, WrapText, X } from "lucide-react";
import { type FileEntry, type FilePreview, fileSize } from "../domain/files";
import { Button } from "./controls";
import { FileIcon } from "./FileIcon";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "./ui/dialog";
const PdfPreview = lazy(() => import("./PdfPreview"));

export function FilePreviewDialog({
  entry,
  data,
  error,
  busy,
  onClose,
  onDownload,
}: {
  entry: FileEntry;
  data?: FilePreview;
  error?: string;
  busy: boolean;
  onClose: () => void;
  onDownload: () => void;
}) {
  const [wrap, setWrap] = useState(false);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent className="file-preview-dialog" showCloseButton={false}>
        <header className="file-preview-header">
          <FileIcon name={entry.name} kind={entry.kind} size={22} />
          <div className="file-preview-identity">
            <DialogTitle title={entry.name}>{entry.name}</DialogTitle>
            <DialogDescription title={entry.path}>
              {entry.path}
            </DialogDescription>
          </div>
          <Button disabled={busy} onClick={onDownload}>
            <Download size={14} aria-hidden="true" />
            Download
          </Button>
          <Button
            variant="ghost"
            size="icon"
            title=""
            aria-label="Close dialog"
            onClick={onClose}
          >
            <X size={16} aria-hidden="true" />
          </Button>
        </header>
        <div
          className={`file-preview-content file-preview-${data?.kind ?? "pending"}`}
          role="region"
          aria-label="File preview"
          tabIndex={0}
        >
          {error ? (
            <div className="file-preview-message">
              <p role="alert" className="file-error">
                {error}
              </p>
              <p>Download to open on your Mac.</p>
            </div>
          ) : !data ? (
            <p className="file-preview-message" role="status">
              Loading preview…
            </p>
          ) : data.kind === "text" ? (
            <pre className={wrap ? "file-text-wrap" : undefined}>
              {data.content || "(Empty file)"}
            </pre>
          ) : data.kind === "image" ? (
            <img
              src={`data:${data.mime};base64,${data.content}`}
              alt={entry.name}
            />
          ) : (
            <Suspense
              fallback={
                <p className="file-preview-message" role="status">
                  Loading PDF viewer…
                </p>
              }
            >
              <PdfPreview content={data.content} />
            </Suspense>
          )}
        </div>
        <footer className="file-preview-footer">
          <span>
            {fileSize(entry.size)}
            {entry.permissions && ` · Permissions ${entry.permissions}`}
          </span>
          {data?.kind === "text" && (
            <Button
              variant="ghost"
              aria-pressed={wrap}
              onClick={() => setWrap(!wrap)}
            >
              <WrapText size={14} aria-hidden="true" />
              Wrap lines
            </Button>
          )}
          {data?.kind === "pdf" && <span>PDF preview</span>}
          {data?.kind === "image" && <span>Image preview</span>}
        </footer>
      </DialogContent>
    </Dialog>
  );
}
