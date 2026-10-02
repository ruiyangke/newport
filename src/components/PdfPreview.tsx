import "./pdfTextLayer.css";
import { useEffect, useRef, useState } from "react";
import {
  getDocument,
  TextLayer,
  GlobalWorkerOptions,
  type PDFDocumentProxy,
} from "pdfjs-dist/legacy/build/pdf.mjs";
import workerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { Button } from "./controls";
GlobalWorkerOptions.workerSrc = workerUrl;

export default function PdfPreview({ content }: { content: string }) {
  return <DocumentPreview key={content} content={content} />;
}
function DocumentPreview({ content }: { content: string }) {
  const [document, setDocument] = useState<PDFDocumentProxy | null>(null);
  const [page, setPage] = useState(1);
  const [error, setError] = useState("");
  const [rendering, setRendering] = useState(true);
  const canvas = useRef<HTMLCanvasElement>(null);
  const textLayer = useRef<HTMLDivElement>(null);
  const scrollArea = useRef<HTMLDivElement>(null);
  const [availableWidth, setAvailableWidth] = useState(900);
  useEffect(() => {
    const area = scrollArea.current;
    if (!area) return;
    const observer = new ResizeObserver(() => {
      const style = getComputedStyle(area);
      setAvailableWidth(
        area.clientWidth -
          parseFloat(style.paddingLeft) -
          parseFloat(style.paddingRight),
      );
    });
    observer.observe(area);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    let active = true;
    const task = getDocument({
      data: Uint8Array.from(atob(content), (character) =>
        character.charCodeAt(0),
      ),
      useSystemFonts: true,
      useWasm: false,
      // Render pages and inert text only: no scripts, links or annotations.
    });
    void task.promise
      .then((pdf) => {
        if (active) setDocument(pdf);
      })
      .catch((reason: unknown) => {
        if (active) {
          setError(`Cannot preview this PDF: ${String(reason)}`);
          setRendering(false);
        }
      });
    return () => {
      active = false;
      void task.destroy().catch(() => {});
    };
  }, [content]);
  useEffect(() => {
    if (!document) return;
    let active = true;
    let text: TextLayer | undefined;
    let render:
      | ReturnType<Awaited<ReturnType<PDFDocumentProxy["getPage"]>>["render"]>
      | undefined;
    void document
      .getPage(page)
      .then(async (pdfPage) => {
        if (!active || !canvas.current || !textLayer.current) return;
        const natural = pdfPage.getViewport({ scale: 1 });
        const scale = Math.min(
          1.5,
          Math.max(200, availableWidth) / natural.width,
          1600 / natural.height,
        );
        const viewport = pdfPage.getViewport({ scale });
        const target = canvas.current;
        target.width = Math.ceil(viewport.width);
        target.height = Math.ceil(viewport.height);
        const layer = textLayer.current;
        layer.replaceChildren();
        layer.style.setProperty("--total-scale-factor", String(scale));
        target.parentElement!.style.width = `${viewport.width}px`;
        target.parentElement!.style.height = `${viewport.height}px`;
        text = new TextLayer({
          container: layer,
          viewport,
          textContentSource: pdfPage.streamTextContent(),
        });
        render = pdfPage.render({ canvas: target, viewport });
        await Promise.all([render.promise, text.render()]);
        if (active) setRendering(false);
      })
      .catch((reason: unknown) => {
        if (active) {
          setError(String(reason));
          setRendering(false);
        }
      });
    return () => {
      active = false;
      render?.cancel();
      text?.cancel();
    };
  }, [document, page, availableWidth]);
  return (
    <div data-slot="pdf-preview" className="flex h-full min-h-0 flex-col">
      <div
        data-slot="pdf-navigation"
        className="flex shrink-0 items-center justify-center gap-[12px] border-b border-border bg-background p-[8px] text-[11px] tabular-nums"
      >
        <Button
          size="icon"
          aria-label="Previous PDF page"
          disabled={!document || page === 1 || rendering}
          onClick={() => {
            setRendering(true);
            setPage(page - 1);
          }}
        >
          <ChevronLeft size={14} />
        </Button>
        <span aria-live="polite">
          {document ? `Page ${page} of ${document.numPages}` : "Loading PDF…"}
        </span>
        <Button
          size="icon"
          aria-label="Next PDF page"
          disabled={!document || page === document.numPages || rendering}
          onClick={() => {
            setRendering(true);
            setPage(page + 1);
          }}
        >
          <ChevronRight size={14} />
        </Button>
      </div>
      {error ? (
        <p
          role="alert"
          data-slot="file-error"
          className="p-[10px] text-(--red) wrap-anywhere"
        >
          {error}. Download the file to open it.
        </p>
      ) : (
        <div
          data-slot="pdf-page"
          className="min-h-0 flex-1 overflow-auto bg-muted p-[24px] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring max-[800px]:p-[12px]"
          ref={scrollArea}
          role="region"
          aria-label="PDF page content"
          tabIndex={0}
        >
          <div data-slot="pdf-sheet" className="relative m-auto bg-white">
            <canvas
              className="m-auto block h-auto max-w-full bg-white shadow-[0_2px_8px_#00000018]"
              ref={canvas}
              aria-hidden="true"
            />
            <div className="textLayer" ref={textLayer} />
          </div>
        </div>
      )}
    </div>
  );
}
