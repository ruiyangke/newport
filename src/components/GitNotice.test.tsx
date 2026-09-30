// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it } from "vitest";
import { GitNotice } from "./GitNotice";

it("speaks only as loudly as its tone: failures interrupt, the rest do not", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () =>
    root.render(
      <>
        <GitNotice tone="error">Cannot read the repository.</GitNotice>
        <GitNotice tone="status">Loading more…</GitNotice>
        <GitNotice tone="info">Showing the first 300 lines.</GitNotice>
      </>,
    ),
  );
  const notices = [
    ...host.querySelectorAll<HTMLElement>(".git-projects-notice"),
  ];
  // shadcn's Alert defaults to role="alert", an assertive live region; only a
  // failure may keep it.
  expect(notices.map((n) => n.getAttribute("role"))).toEqual([
    "alert",
    "status",
    "note",
  ]);
  expect(notices.map((n) => n.textContent)).toEqual([
    "Cannot read the repository.",
    "Loading more…",
    "Showing the first 300 lines.",
  ]);
  await act(async () => root.unmount());
  host.remove();
});
