/** All app dialogs share Radix's open-state contract, including nested previews. */
export function hasOpenDialog() {
  return (
    document.querySelector(
      '[role="dialog"][data-state="open"], [role="alertdialog"][data-state="open"]',
    ) !== null
  );
}
