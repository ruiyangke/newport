// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { getTerminalSession, terminalTitle } from "./session";
import { retainTerminals } from "./registry";
const api = vi.hoisted(() => ({ desktop: vi.fn() }));
const xterm = vi.hoisted(() => ({
  titles: [] as Array<(value: string) => void>,
}));
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
    onTitleChange = (listener: (value: string) => void) => {
      xterm.titles.push(listener);
    };
    write = vi.fn((_data, done) => done());
  },
}));
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit = vi.fn();
  },
}));
const webgl = vi.hoisted(() => {
  class WebglAddon {
    static instances: WebglAddon[] = [];
    static failed = false;
    dispose = vi.fn();
    clearTextureAtlas = vi.fn();
    loss: (() => void) | undefined;
    constructor() {
      if (WebglAddon.failed) throw new Error("WebGL2 is unavailable");
      WebglAddon.instances.push(this);
    }
    onContextLoss(listener: () => void) {
      this.loss = listener;
    }
  }
  return WebglAddon;
});
vi.mock("@xterm/addon-webgl", () => ({ WebglAddon: webgl }));
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => {
    resolve = yes;
  });
  return { promise, resolve };
}
beforeEach(() => {
  api.desktop.mockReset();
  webgl.instances.length = 0;
  webgl.failed = false;
  xterm.titles.length = 0;
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
  const terminal = getTerminalSession("a", "tab", "a");
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
  expect(getTerminalSession("a", "tab", "a")).toBe(terminal);
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
  const terminal = getTerminalSession("a", "tab", "a");
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
  const terminal = getTerminalSession("a", "tab", "a");
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
  const terminal = getTerminalSession("a", "tab", "a");
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
  const terminal = getTerminalSession("a", "tab", "a");
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
it("renders with WebGL while attached and releases the context on detach", async () => {
  const terminal = getTerminalSession("gpu", "tab", "gpu");
  const host = document.createElement("div");
  document.body.append(host);
  const detach = terminal.attach(host);
  expect(webgl.instances).toHaveLength(1);
  const addon = webgl.instances[0];
  expect(terminal.term.loadAddon).toHaveBeenCalledWith(addon);
  // A detached atlas is stale once the Nerd Font finishes loading.
  await vi.waitFor(() =>
    expect(addon.clearTextureAtlas).toHaveBeenCalledOnce(),
  );
  detach();
  expect(addon.dispose).toHaveBeenCalledOnce();
  const reattach = terminal.attach(host);
  expect(webgl.instances).toHaveLength(2);
  reattach();
  expect(webgl.instances[1].dispose).toHaveBeenCalledOnce();
  host.remove();
});
it("keeps the DOM renderer when WebGL2 is unavailable", () => {
  webgl.failed = true;
  const terminal = getTerminalSession("no-gpu", "tab", "no-gpu");
  const host = document.createElement("div");
  document.body.append(host);
  const detach = terminal.attach(host);
  expect(webgl.instances).toHaveLength(0);
  expect(terminal.term.open).toHaveBeenCalledOnce();
  expect(terminal.term.loadAddon).toHaveBeenCalledTimes(1);
  detach();
  host.remove();
});
it("falls back to the DOM renderer when the GPU context is lost", () => {
  const terminal = getTerminalSession("lost", "tab", "lost");
  const host = document.createElement("div");
  document.body.append(host);
  const detach = terminal.attach(host);
  const addon = webgl.instances[0];
  addon.loss?.();
  expect(addon.dispose).toHaveBeenCalledOnce();
  // A later detach must not dispose the addon twice.
  detach();
  expect(addon.dispose).toHaveBeenCalledOnce();
  // The next attach starts from a fresh context.
  const reattach = terminal.attach(host);
  expect(webgl.instances).toHaveLength(2);
  reattach();
  host.remove();
});
it("releases the GPU context when the session is disposed", () => {
  const terminal = getTerminalSession("gone", "tab", "gone");
  const host = document.createElement("div");
  document.body.append(host);
  terminal.attach(host);
  const addon = webgl.instances[0];
  retainTerminals(new Set());
  expect(addon.dispose).toHaveBeenCalledOnce();
  host.remove();
});
it("titles a tab from the shell and clears it for a new connection", async () => {
  api.desktop.mockImplementation((command) =>
    command === "terminal_read"
      ? Promise.resolve({ type: "exit", data: 0 })
      : Promise.resolve(),
  );
  const terminal = getTerminalSession("title", "tab", "title");
  xterm.titles.at(-1)!("dev@host: ~");
  expect(terminal.state.state.title).toBe("dev@host: ~");
  const connecting = terminal.connect();
  expect(terminal.state.state.title).toBe("");
  await connecting;
  expect(terminal.state.state.title).toBe("");
});
it("keeps shell titles single-line and bounded", () => {
  expect(terminalTitle("  dev\u0007host: ~ ")).toBe("devhost: ~");
  // Bidi overrides from the remote shell must not reorder the label.
  expect(terminalTitle("safe\u202Eevil")).toBe("safeevil");
  expect(terminalTitle("\u2066x\u2069")).toBe("x");
  expect(terminalTitle("x".repeat(200))).toHaveLength(120);
  // A title is never cut through a surrogate pair.
  expect(terminalTitle("a".repeat(119) + "\u{1F600}")).toBe("a".repeat(119));
});
