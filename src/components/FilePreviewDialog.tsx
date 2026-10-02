import { cn } from "../lib/utils";
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
      <DialogContent
        data-preview-dialog
        className="flex h-[min(760px,calc(100vh-48px))] max-h-[calc(100vh-48px)] w-[min(1040px,calc(100vw-48px))] max-w-none flex-col gap-0 overflow-hidden p-0 sm:max-w-none max-[800px]:h-[calc(100vh-24px)] max-[800px]:max-h-[calc(100vh-24px)] max-[800px]:w-[calc(100vw-24px)]"
        showCloseButton={false}
      >
        <header
          data-slot="file-preview-header"
          className="flex shrink-0 items-center gap-[12px] border-b border-border bg-transparent px-[20px] py-[16px] [&>svg]:text-muted-foreground max-[800px]:gap-[8px] [@media(max-height:560px),(max-width:540px)]:gap-[8px] [@media(max-height:560px),(max-width:540px)]:p-[12px]"
        >
          <FileIcon name={entry.name} kind={entry.kind} size={22} />
          <div data-slot="file-preview-identity" className="min-w-0 flex-1">
            <DialogTitle
              className="truncate text-[13px] leading-[1.35]"
              title={entry.name}
            >
              {entry.name}
            </DialogTitle>
            <DialogDescription
              className="mt-[2px] truncate text-[11px]"
              title={entry.path}
            >
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
          data-slot="file-preview-content"
          className={cn(
            "min-h-0 flex-1 overflow-auto focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
            !data && "flex items-center justify-center",
            data?.kind === "image" &&
              "flex items-center justify-center bg-muted p-[20px]",
            data?.kind === "pdf" && "overflow-hidden",
          )}
          role="region"
          aria-label="File preview"
          tabIndex={0}
        >
          {error ? (
            <div
              data-slot="file-preview-message"
              className="m-auto max-w-[65ch] p-[28px] text-center text-muted-foreground"
            >
              <p
                role="alert"
                data-slot="file-error"
                className="pb-[8px] text-(--red) wrap-anywhere"
              >
                {error}
              </p>
              <p>Download to open on your computer.</p>
            </div>
          ) : !data ? (
            <p
              data-slot="file-preview-message"
              className="m-auto max-w-[65ch] p-[28px] text-center text-muted-foreground"
              role="status"
            >
              Loading preview…
            </p>
          ) : data.kind === "text" ? (
            <pre
              className={cn(
                "m-0 p-[20px] font-[ui-monospace,SFMono-Regular,Menlo,monospace] text-[12px] leading-[1.7] tab-4",
                wrap ? "whitespace-pre-wrap wrap-anywhere" : "whitespace-pre",
              )}
            >
              {data.content || "(Empty file)"}
            </pre>
          ) : data.kind === "image" ? (
            <img
              className="block max-h-full max-w-full object-contain"
              src={`data:${data.mime};base64,${data.content}`}
              alt={entry.name}
            />
          ) : (
            <Suspense
              fallback={
                <p
                  data-slot="file-preview-message"
                  className="m-auto max-w-[65ch] p-[28px] text-center text-muted-foreground"
                  role="status"
                >
                  Loading PDF viewer…
                </p>
              }
            >
              <PdfPreview content={data.content} />
            </Suspense>
          )}
        </div>
        <footer
          data-slot="file-preview-footer"
          className="flex min-h-[40px] shrink-0 items-center justify-between gap-[8px] border-t border-border bg-transparent px-[20px] py-[10px] text-[11px] text-muted-foreground"
        >
          <span>
            {fileSize(entry.size)}
            {entry.permissions && ` · Permissions ${entry.permissions}`}
          </span>
          {data?.kind === "text" && (
            <Button
              variant="ghost"
              className="aria-pressed:bg-accent"
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
