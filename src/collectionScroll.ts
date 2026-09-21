type ScrollSnapshot = {
  scroller: HTMLElement;
  top: number;
  anchor?: HTMLElement;
  offset: number;
};

// Capture immediately before replacing a sample, not when its request starts:
// the user may have scrolled while the server was responding.
export function captureCollectionScroll(
  scope: HTMLElement | null,
): ScrollSnapshot[] {
  if (!scope || scope.closest("[hidden]")) return [];
  const workspace = scope.closest<HTMLElement>(".workspace-scroll");
  if (!workspace) return [];
  const scrollers = [
    workspace,
    ...scope.querySelectorAll<HTMLElement>('[data-slot="table-container"]'),
  ];
  return scrollers.map((scroller) => {
    const bounds = scroller.getBoundingClientRect();
    const candidates =
      scroller === workspace
        ? scope.querySelectorAll<HTMLElement>(
            '[data-scroll-anchor], [data-slot="table-container"]',
          )
        : scroller.querySelectorAll<HTMLElement>("tbody tr");
    const anchor = Array.from(candidates).find((row) => {
      const rect = row.getBoundingClientRect();
      return rect.bottom > bounds.top && rect.top < bounds.bottom;
    });
    return {
      scroller,
      top: scroller.scrollTop,
      anchor,
      offset: anchor ? anchor.getBoundingClientRect().top - bounds.top : 0,
    };
  });
}

export function restoreCollectionScroll(snapshots: ScrollSnapshot[]) {
  for (const { scroller, top, anchor, offset } of snapshots) {
    if (!scroller.isConnected) continue;
    if (anchor?.isConnected && scroller.contains(anchor)) {
      scroller.scrollTop +=
        anchor.getBoundingClientRect().top -
        scroller.getBoundingClientRect().top -
        offset;
    } else {
      // A removed row falls back to its previous position, clamped by the browser.
      scroller.scrollTop = top;
    }
  }
}
