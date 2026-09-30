import type { KeyboardEvent } from "react";

/**
 * The keys that ask for a context menu from the keyboard: Shift+F10, and the
 * ContextMenu key found on many keyboards.
 */
export const asksForContextMenu = (event: KeyboardEvent) =>
  event.key === "ContextMenu" || (event.shiftKey && event.key === "F10");
/**
 * cmdk keeps focus in the filter and only marks the highlighted row, so a
 * keyboard request for a context menu reaches the filter, not the row. This
 * hands it to the highlighted row as the pointer would, at the row's start and
 * just below it, so the row's own menu opens anchored to that row. Returns
 * whether there was a row to open it on.
 */
export function openHighlightedRowMenu(input: HTMLInputElement) {
  const id = input.getAttribute("aria-activedescendant");
  const row = id ? input.ownerDocument.getElementById(id) : null;
  if (!row) return false;
  const bounds = row.getBoundingClientRect();
  row.dispatchEvent(
    new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      button: 2,
      clientX: bounds.left + 10,
      clientY: bounds.bottom,
    }),
  );
  return true;
}
