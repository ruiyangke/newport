/**
 * The Changes view's file list and diff, side by side on a shadcn resizable
 * split, or one at a time when the layout is too narrow for two.
 *
 * It owns both halves of the layout question: the list's width (the design's
 * 250px default, 240–560px), and the narrow single-pane flow — which pane is
 * showing, the "Show diff" bar under the list, and, through
 * `GitChangesPaneContext`, the "Back to changes" button the diff draws in its
 * own toolbar.
 */
import {
  createContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { cn } from "cn";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "../ui/resizable";
import { Button } from "../controls";

/**
 * The list column, in pixels: its default and its useful range. 290 fits a
 * status badge, a two-level directory and a typical file name without cutting
 * into the name; the prototype's 250 predates the badges and cut most paths
 * down to their last segment.
 */
export const LIST_WIDTH = 290;
export const MIN_LIST_WIDTH = 240;
export const MAX_LIST_WIDTH = 560;
/**
 * Below this the two columns would both be too narrow to read: the list's 290
 * leaves the diff about 400px, the least its header and a line of code need.
 * Measured on the layout element, which sits beside the app's sidebar.
 */
export const SINGLE_PANE_WIDTH = 700;

export type ChangesPane = "list" | "diff";
export const GitChangesPaneContext = createContext<{
  narrow: boolean;
  pane: ChangesPane;
  setPane: (pane: ChangesPane) => void;
} | null>(null);

/** The list/diff resizable pair, shared by the Changes view and the commit inspector. */
export function ListDiffPanels({
  list,
  detail,
  label,
  listClassName,
  detailClassName,
  className,
}: {
  list: ReactNode;
  detail: ReactNode;
  /** The handle's accessible name. */
  label: string;
  listClassName?: string;
  detailClassName?: string;
  className?: string;
}) {
  return (
    <ResizablePanelGroup orientation="horizontal" className={className}>
      <ResizablePanel
        defaultSize={`${LIST_WIDTH}px`}
        minSize={`${MIN_LIST_WIDTH}px`}
        maxSize={`${MAX_LIST_WIDTH}px`}
        // Widening the window gives the room to the diff, not the list.
        groupResizeBehavior="preserve-pixel-size"
        className={listClassName}
      >
        {list}
      </ResizablePanel>
      <ResizableHandle aria-label={label} />
      <ResizablePanel minSize="200px" className={detailClassName}>
        {detail}
      </ResizablePanel>
    </ResizablePanelGroup>
  );
}

export function GitChangesSplit({
  list,
  detail,
  detailTitle,
  detailKey,
  className,
  listClassName,
  detailClassName,
}: {
  list: ReactNode;
  detail: ReactNode;
  /** The selected file's name, for the "Show diff" bar under a narrow list. */
  detailTitle?: string;
  /** Changing this (a newly selected file) brings a narrow layout to the diff. */
  detailKey?: string;
  /** The frame: size, borders. Not a grid; the split lays out its own panes. */
  className?: string;
  listClassName?: string;
  detailClassName?: string;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [narrow, setNarrow] = useState(false);
  const [pane, setPane] = useState<ChangesPane>("diff");
  const [shownKey, setShownKey] = useState(detailKey);
  if (shownKey !== detailKey) {
    setShownKey(detailKey);
    setPane("diff");
  }
  useEffect(() => {
    const element = root.current;
    const measure = () => {
      // jsdom and a detached first paint report 0, where the window is the
      // only honest measure available.
      const width = element?.getBoundingClientRect().width || window.innerWidth;
      setNarrow(width < SINGLE_PANE_WIDTH);
    };
    measure();
    window.addEventListener("resize", measure);
    const observer =
      element && typeof ResizeObserver !== "undefined"
        ? new ResizeObserver(measure)
        : null;
    if (element) observer?.observe(element);
    return () => {
      window.removeEventListener("resize", measure);
      observer?.disconnect();
    };
  }, []);
  const context = useMemo(() => ({ narrow, pane, setPane }), [narrow, pane]);
  const shown = narrow ? pane : undefined;
  return (
    <div
      ref={root}
      className={cn("flex min-h-0 min-w-0", className)}
      data-git-changes-pane={shown}
    >
      <GitChangesPaneContext.Provider value={context}>
        {narrow ? (
          // Both stay mounted, so going back to the list and returning keeps
          // the diff's comparison and context choices.
          <>
            <div
              className={cn("flex min-w-0 flex-1 flex-col", listClassName)}
              hidden={pane !== "list"}
            >
              {list}
              {detailTitle && (
                <header className="git-diff-toolbar border-t border-border">
                  <strong>{detailTitle}</strong>
                  <Button className="flex-none" onClick={() => setPane("diff")}>
                    Show diff
                  </Button>
                </header>
              )}
            </div>
            <div
              className={cn("min-w-0 flex-1", detailClassName)}
              hidden={pane !== "diff"}
            >
              {detail}
            </div>
          </>
        ) : (
          <ListDiffPanels
            list={list}
            detail={detail}
            label="Resize the file list"
            listClassName={listClassName}
            detailClassName={detailClassName}
          />
        )}
      </GitChangesPaneContext.Provider>
    </div>
  );
}
