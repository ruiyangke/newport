// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  live: true,
  maximized: false,
  minimize: vi.fn(async () => {}),
  toggleMaximize: vi.fn(async () => {}),
  close: vi.fn(async () => {}),
  listeners: new Set<() => void>(),
}));
vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => tauri.live,
  invoke: vi.fn(),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    isMaximized: async () => tauri.maximized,
    onResized: async (listener: () => void) => {
      tauri.listeners.add(listener);
      return () => {
        tauri.listeners.delete(listener);
      };
    },
    minimize: tauri.minimize,
    toggleMaximize: tauri.toggleMaximize,
    close: tauri.close,
  }),
}));

import { WindowControls } from "./WindowControls";
import { TooltipProvider } from "./ui/tooltip";

const realPlatform = window.navigator.platform;
function setPlatform(value: string) {
  Object.defineProperty(window.navigator, "platform", {
    configurable: true,
    value,
  });
}

let root: Root;
let host: HTMLDivElement;
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  tauri.live = true;
  tauri.maximized = false;
  tauri.listeners.clear();
  tauri.minimize.mockClear();
  tauri.toggleMaximize.mockClear();
  tauri.close.mockClear();
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(() => root.unmount());
  host.remove();
  setPlatform(realPlatform);
  vi.unstubAllGlobals();
});

async function render() {
  await act(async () =>
    root.render(
      <TooltipProvider>
        <WindowControls />
      </TooltipProvider>,
    ),
  );
}

it("renders nothing outside a Windows desktop window", async () => {
  setPlatform("MacIntel");
  await render();
  expect(host.querySelector('[data-slot="window-controls"]')).toBeNull();
  tauri.live = false;
  setPlatform("Win32");
  await render();
  expect(host.querySelector('[data-slot="window-controls"]')).toBeNull();
});

it("drives minimize, maximize/restore and close on Windows", async () => {
  setPlatform("Win32");
  await render();
  const group = host.querySelector('[data-slot="window-controls"]');
  expect(group).not.toBeNull();
  const buttons = [...host.querySelectorAll("button")];
  expect(buttons.map((button) => button.getAttribute("aria-label"))).toEqual([
    "Minimize",
    "Maximize",
    "Close window",
  ]);
  await act(async () => buttons[0].click());
  expect(tauri.minimize).toHaveBeenCalledOnce();
  await act(async () => buttons[2].click());
  expect(tauri.close).toHaveBeenCalledOnce();
  // The resize listener keeps the maximize label honest.
  tauri.maximized = true;
  for (const listener of [...tauri.listeners]) listener();
  await act(async () => {});
  expect(
    host.querySelector("button[aria-label='Restore']"),
  ).not.toBeNull();
  await act(async () => (host.querySelector("button[aria-label='Restore']") as HTMLElement).click());
  expect(tauri.toggleMaximize).toHaveBeenCalledOnce();
});
