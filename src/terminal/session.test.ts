// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { getTerminalSession } from "./session";
import { retainTerminals } from "./registry";
const api = vi.hoisted(() => ({ desktop: vi.fn() }));
vi.mock("../api/desktop", () => api);
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    options = {};
    cols = 80;
    rows = 24;
    open = vi.fn();
    dispose = vi.fn();
    reset = vi.fn();
    focus = vi.fn();
    loadAddon = vi.fn();
    onData = vi.fn();
    onBinary = vi.fn();
    write = vi.fn((_data, done) => done());
  },
}));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit = vi.fn();
  },
}));
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => {
    resolve = yes;
  });
  return { promise, resolve };
}
beforeEach(() => {
  api.desktop.mockReset();
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  vi.stubGlobal("requestAnimationFrame", () => 1);
  vi.stubGlobal("cancelAnimationFrame", () => {});
  Object.defineProperty(document, "fonts", {
    configurable: true,
    value: { load: async () => [] },
  });
});
afterEach(() => {
  retainTerminals(new Set());
  vi.unstubAllGlobals();
});
it("detaches the view without closing, then reattaches the same terminal", async () => {
  const read = deferred<unknown>();
  api.desktop.mockImplementation((command) =>
    command === "terminal_read" ? read.promise : Promise.resolve(),
  );
  const terminal = getTerminalSession("a", "a");
  const host = document.createElement("div");
  document.body.append(host);
  const detach = terminal.attach(host);
  const connecting = terminal.connect();
  await vi.waitFor(() =>
    expect(api.desktop).toHaveBeenCalledWith(
      "terminal_read",
      expect.anything(),
    ),
  );
  detach();
  expect(
    api.desktop.mock.calls.some(([command]) => command === "terminal_close"),
  ).toBe(false);
  expect(getTerminalSession("a", "a")).toBe(terminal);
  const detachAgain = terminal.attach(host);
  expect(terminal.term.open).toHaveBeenCalledTimes(1);
  read.resolve({ type: "exit", data: 0 });
  await connecting;
  expect(terminal.state.state.status).toBe("Shell exited · 0");
  detachAgain();
  host.remove();
});
it("closes a late open after its server is removed without resurrecting status", async () => {
  const opening = deferred<void>();
  api.desktop.mockImplementation((command) =>
    command === "terminal_open" ? opening.promise : Promise.resolve(),
  );
  const terminal = getTerminalSession("a", "a");
  const connecting = terminal.connect();
  await vi.waitFor(() =>
    expect(api.desktop).toHaveBeenCalledWith(
      "terminal_open",
      expect.anything(),
    ),
  );
  retainTerminals(new Set());
  opening.resolve();
  await connecting;
  expect(terminal.term.dispose).toHaveBeenCalledOnce();
  expect(
    api.desktop.mock.calls.filter(([command]) => command === "terminal_close"),
  ).toHaveLength(2);
  expect(
    api.desktop.mock.calls.some(([command]) => command === "terminal_read"),
  ).toBe(false);
  expect(terminal.state.state.active).toBe(false);
});
it("a late event from a cancelled session cannot overwrite a new connection", async () => {
  const oldRead = deferred<unknown>();
  const newRead = deferred<unknown>();
  let reads = 0;
  api.desktop.mockImplementation((command) =>
    command === "terminal_read"
      ? ++reads === 1
        ? oldRead.promise
        : newRead.promise
      : Promise.resolve(),
  );
  const terminal = getTerminalSession("a", "a");
  const first = terminal.connect();
  await vi.waitFor(() => expect(reads).toBe(1));
  terminal.disconnect();
  const second = terminal.connect();
  await vi.waitFor(() => expect(reads).toBe(2));
  oldRead.resolve({ type: "error", data: "obsolete failure" });
  await first;
  expect(terminal.state.state).toMatchObject({
    active: true,
    error: "",
    status: "Connecting…",
  });
  newRead.resolve({ type: "exit", data: 0 });
  await second;
});

it("does not steal focus when a cancelled setup finishes its resize", async () => {
  const resize = deferred<void>();
  api.desktop.mockImplementation((command) => {
    if (command === "terminal_read") return Promise.resolve({ type: "ready" });
    if (command === "terminal_resize") return resize.promise;
    return Promise.resolve();
  });
  const terminal = getTerminalSession("a", "a");
  const host = document.createElement("div");
  document.body.append(host);
  const detach = terminal.attach(host);
  const running = terminal.connect();
  await vi.waitFor(() =>
    expect(api.desktop).toHaveBeenCalledWith(
      "terminal_resize",
      expect.anything(),
    ),
  );
  terminal.disconnect();
  resize.resolve();
  await running;
  expect(terminal.term.focus).not.toHaveBeenCalled();
  detach();
  host.remove();
});

it("drains queued output before resetting for a new connection", async () => {
  let parsed!: () => void;
  let reads = 0;
  api.desktop.mockImplementation((command) => {
    if (command === "terminal_read")
      return Promise.resolve(
        ++reads === 1
          ? { type: "data", data: [65] }
          : { type: "exit", data: 0 },
      );
    return Promise.resolve();
  });
  const terminal = getTerminalSession("a", "a");
  vi.mocked(terminal.term.write).mockImplementation((_data, callback) => {
    parsed = callback!;
  });
  const first = terminal.connect();
  await vi.waitFor(() => expect(terminal.term.write).toHaveBeenCalledOnce());
  terminal.disconnect();
  const second = terminal.connect();
  await Promise.resolve();
  expect(terminal.term.reset).toHaveBeenCalledTimes(1);
  parsed();
  await Promise.all([first, second]);
  expect(terminal.term.reset).toHaveBeenCalledTimes(2);
});
