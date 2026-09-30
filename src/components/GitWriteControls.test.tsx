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

it("requires explicit inspection before recording an unknown outcome review", async () => {
  const onReview = vi.fn();
  const onCheck = vi.fn();
  const onAcknowledge = vi.fn();
  await act(async () =>
    root.render(
      <GitRecoveryPanel
        receipts={[
          {
            operationId: "uncertain",
            serverId: "server",
            action: "pull.fast_forward",
            state: "outcome_unknown",
          },
        ]}
        error=""
        busy={false}
        onCheck={onCheck}
        onAcknowledge={onAcknowledge}
        onRefresh={() => {}}
        onReview={onReview}
      />,
    ),
  );
  const button = (text: string) =>
    [...document.querySelectorAll("button")].find(
      (b) => b.textContent === text,
    )!;
  await act(async () => button("Review interrupted operation…").click());
  expect(button("Record review").disabled).toBe(true);
  expect(document.body.textContent).toContain("does not retry the operation");
  await act(async () => button("Record review").click());
  expect(onReview).not.toHaveBeenCalled();
  await act(async () =>
    (document.querySelector('[role="checkbox"]') as HTMLButtonElement).click(),
  );
  await act(async () => button("Record review").click());
  expect(onReview).toHaveBeenCalledExactlyOnceWith("uncertain");
  expect(onCheck).not.toHaveBeenCalled();
  expect(onAcknowledge).not.toHaveBeenCalled();
});

it("does not offer review for an operation whose outcome has not been checked", async () => {
  await act(async () =>
    root.render(
      <GitRecoveryPanel
        receipts={[
          {
            operationId: "pending",
            serverId: "server",
            action: "pull.fast_forward",
            state: "pending",
          },
        ]}
        error=""
        busy={false}
        onCheck={vi.fn()}
        onAcknowledge={vi.fn()}
        onRefresh={() => {}}
        onReview={vi.fn()}
      />,
    ),
  );
  expect(host.textContent).not.toContain("Review interrupted operation…");
  expect(host.textContent).toContain("Check outcome");
});
