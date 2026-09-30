// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GitRecoveryPanel, GitWriteControls } from "./GitWriteControls";
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
const status = decodeGitStatus({
  snapshot: "s",
  entries: [],
  nextCursor: null,
  metadata: {
    head: { name: null, oid: null, unborn: true, detached: false },
    operationState: "Clean",
    integration: null,
    ahead: null,
    behind: null,
    upstreamRef: null,
    basis: "stored_refs",
  },
});
it("guards form submission even if a disabled commit button is bypassed", async () => {
  const onAction = vi.fn();
  await act(async () =>
    root.render(
      <GitWriteControls
        status={status}
        disabled={false}
        message="A message"
        onMessage={() => {}}
        onAction={onAction}
      />,
    ),
  );
  await act(async () => {
    host
      .querySelector("form")!
      .dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
  expect(onAction).not.toHaveBeenCalled();
});
it("keeps unresolved outcomes queryable but cannot dismiss them", async () => {
  const onCheck = vi.fn();
  const onAcknowledge = vi.fn();
  await act(async () =>
    root.render(
      <GitRecoveryPanel
        receipts={[
          {
            operationId: "saved-id",
            serverId: "server",
            action: "commit",
            state: "outcome_unknown",
          },
        ]}
        error=""
        busy={false}
        onCheck={onCheck}
        onAcknowledge={onAcknowledge}
        onRefresh={() => {}}
      />,
    ),
  );
  expect(host.textContent).toContain("Saved outcomes on this server");
  expect(host.textContent).toContain("Repository not recorded");
  const buttons = [...host.querySelectorAll("button")];
  expect(
    buttons.some((button) => button.textContent?.includes("Dismiss")),
  ).toBe(false);
  await act(async () =>
    buttons.find((button) => button.textContent === "Check outcome")!.click(),
  );
  expect(onCheck).toHaveBeenCalledWith("saved-id");
  expect(onAcknowledge).not.toHaveBeenCalled();
});
