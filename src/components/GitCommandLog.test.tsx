// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { GitCommandLog } from "./GitCommandLog";
const listener = vi.hoisted(() => ({
  callback: undefined as undefined | ((event: { payload: unknown }) => void),
  stop: vi.fn(),
}));
vi.mock("./GitLogTerminal", () => ({
  GitLogTerminal: ({ text }: { text: string }) => <pre>{text}</pre>,
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name, callback) => {
    listener.callback = callback;
    return listener.stop;
  }),
}));
it("keeps repository logs scoped, bounded, clearable and accessible from the footer", async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      <GitCommandLog serverId="server" repoId="repo">
        Read just now
      </GitCommandLog>,
    );
  });
  const trigger = [...container.querySelectorAll("button")].find(
    (button) => button.textContent === "Log",
  )!;
  expect(trigger.getAttribute("aria-expanded")).toBe("false");
  await act(async () => {
    listener.callback?.({
      payload: {
        serverId: "other",
        repoId: "repo",
        entry: { command: "foreign", output: "hidden" },
      },
    });
    for (let n = 0; n < 105; n++)
      listener.callback?.({
        payload: {
          serverId: "server",
          repoId: "repo",
          entry: {
            command: `git command-${n}`,
            output: `diagnostic-${n}`,
            durationMs: 12,
            exitCode: 0,
            interrupted: false,
          },
        },
      });
    trigger.click();
  });
  expect(container.textContent).not.toContain("foreign");
  expect(container.textContent).not.toContain("diagnostic-0");
  expect(container.textContent).toContain("diagnostic-104");
  expect(trigger.getAttribute("aria-expanded")).toBe("true");
  await act(async () => {
    (
      container.querySelector(
        '[aria-label="Clear Git log"]',
      ) as HTMLButtonElement
    ).click();
  });
  expect(container.textContent).not.toContain("diagnostic-104");
  expect(container.textContent).toContain("Run a Git action");
  await act(async () => {
    (
      container.querySelector(
        '[aria-label="Close Git log"]',
      ) as HTMLButtonElement
    ).click();
  });
  expect(document.activeElement).toBe(trigger);
  await act(async () => root.unmount());
  expect(listener.stop).toHaveBeenCalledOnce();
  container.remove();
});
