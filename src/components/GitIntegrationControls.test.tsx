// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GitIntegrationControls } from "./GitIntegrationControls";
import { decodeGitStatus } from "../domain/gitResponses";
let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});
function status(managed: boolean) {
  return decodeGitStatus({
    snapshot: "s",
    entries: [],
    nextCursor: null,
    metadata: {
      head: { oid: null, name: null, unborn: false, detached: true },
      operationState: "Rebase",
      ahead: null,
      behind: null,
      upstreamRef: null,
      basis: "stored_refs",
      integration: {
        kind: "rebase",
        managed,
        canContinue: managed,
        canAbort: managed,
        canSkip: false,
        position: 2,
        total: 4,
      },
    },
  });
}
it("does not offer recovery writes for an unmanaged operation", async () => {
  const onAction = vi.fn();
  await act(async () =>
    root.render(
      <GitIntegrationControls
        status={status(false)}
        disabled={false}
        busy={false}
        error=""
        onAction={onAction}
      />,
    ),
  );
  expect(host.textContent).toContain("cannot be managed by Newport");
  expect(host.querySelectorAll("button")).toHaveLength(0);
  expect(onAction).not.toHaveBeenCalled();
});
it("preserves reported progress and never offers skip when the agent disallows it", async () => {
  await act(async () =>
    root.render(
      <GitIntegrationControls
        status={status(true)}
        disabled={true}
        busy={false}
        error=""
        onAction={vi.fn()}
      />,
    ),
  );
  expect(host.textContent).toContain("2 of 4");
  const buttons = [...host.querySelectorAll("button")];
  expect(buttons.map((button) => button.textContent)).toEqual([
    "Continue rebase",
    "Abort…",
  ]);
  expect(buttons.every((button) => button.disabled)).toBe(true);
});
