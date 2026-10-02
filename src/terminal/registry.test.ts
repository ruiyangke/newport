import { afterEach, expect, it, vi } from "vitest";
import {
  dropTerminalSession,
  getTerminalLayout,
  peekTerminalSession,
  retainTerminals,
  saveTerminalLayout,
  tabAfterClose,
  terminalResources,
} from "./registry";
import type { TerminalSession } from "./session";

afterEach(() => retainTerminals(new Set()));

it("activates the right neighbour when the active tab closes", () => {
  const tabs = [
    { id: "a", name: "Terminal 1" },
    { id: "b", name: "Terminal 2" },
    { id: "c", name: "Terminal 3" },
  ];
  // Closing the first or middle tab moves right; the last moves left.
  expect(tabAfterClose(tabs, "a", "a")).toBe("b");
  expect(tabAfterClose(tabs, "b", "b")).toBe("c");
  expect(tabAfterClose(tabs, "c", "c")).toBe("b");
  // Closing any other tab leaves the active one alone.
  expect(tabAfterClose(tabs, "a", "b")).toBe("b");
});

it("keeps one named tab per connection with monotonic numbering", () => {
  const layout = getTerminalLayout("connection");
  expect(layout.tabs).toHaveLength(1);
  expect(layout.tabs[0].name).toBe("Terminal 1");
  saveTerminalLayout("connection", {
    tabs: [...layout.tabs, { id: "second", name: "Terminal 2" }],
    active: "second",
    next: 3,
  });
  const reopened = getTerminalLayout("connection");
  expect(reopened.active).toBe("second");
  expect(reopened.next).toBe(3);
});

it("drops every session and layout for a connection that is gone", () => {
  const session = (dispose: () => void) =>
    ({ dispose }) as unknown as TerminalSession;
  const first = vi.fn();
  const second = vi.fn();
  terminalResources.set(
    "gone",
    new Map([
      ["one", session(first)],
      ["two", session(second)],
    ]),
  );
  terminalResources.set("kept", new Map([["one", session(vi.fn())]]));
  getTerminalLayout("gone");
  getTerminalLayout("kept");
  retainTerminals(new Set(["kept"]));
  expect(first).toHaveBeenCalledOnce();
  expect(second).toHaveBeenCalledOnce();
  expect(terminalResources.has("gone")).toBe(false);
  expect(terminalResources.has("kept")).toBe(true);
  expect(peekTerminalSession("gone", "one")).toBeUndefined();
});

it("disposes a closed tab and forgets it", () => {
  const dispose = vi.fn();
  terminalResources.set(
    "connection",
    new Map([["tab", { dispose } as unknown as TerminalSession]]),
  );
  dropTerminalSession("connection", "tab");
  expect(dispose).toHaveBeenCalledOnce();
  expect(peekTerminalSession("connection", "tab")).toBeUndefined();
});
