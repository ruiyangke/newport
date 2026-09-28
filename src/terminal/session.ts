import { createStore } from "@tanstack/react-store";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { hasOpenDialog } from "../overlays";
import { desktop } from "../api/desktop";
import { terminalResources } from "./registry";

type Session = {
  id: string;
  ready: boolean;
  queued: number;
  writes: Promise<void>;
};
export class TerminalSession {
  readonly state = createStore({
    status: "Disconnected",
    error: "",
    active: false,
    started: false,
  });
  readonly element = document.createElement("div");
  readonly term = new Terminal({
    fontFamily: 'Menlo, Monaco, "Newport Symbols", ui-monospace, monospace',
    fontSize: 12,
    cursorBlink: false,
    scrollback: 5000,
    screenReaderMode: true,
    disableStdin: true,
    minimumContrastRatio: 4.5,
    allowProposedApi: false,
  });
  readonly fit = new FitAddon();
  private session: Session | null = null;
  private output: Promise<void> = Promise.resolve();
  private finishWrite?: () => void;
  private opened = false;
  private disposed = false;
  private frame = 0;
  private lastSize = "";
  private appearance: MutationObserver;
  private observer: ResizeObserver;
  constructor(private serverId: string) {
    this.element.className = "terminal-surface";
    this.term.loadAddon(this.fit);
    this.term.onData((value) => this.send(new TextEncoder().encode(value)));
    this.term.onBinary((value) =>
      this.send(Uint8Array.from(value, (c) => c.charCodeAt(0))),
    );
    this.appearance = new MutationObserver(() => this.theme());
    this.appearance.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    this.observer = new ResizeObserver(() => this.resize());
    this.theme();
  }
  private patch(value: Partial<typeof this.state.state>) {
    if (!this.disposed)
      this.state.setState((state) => ({ ...state, ...value }));
  }
  private theme() {
    const dark = document.documentElement.classList.contains("dark");
    this.term.options.theme = dark
      ? {
          background: "#1e1f21",
          foreground: "#eeeeef",
          cursor: "#eeeeef",
          selectionBackground: "#454349",
        }
      : {
          background: "#ffffff",
          foreground: "#25262a",
          cursor: "#25262a",
          selectionBackground: "#dad9dd",
        };
  }
  attach(host: HTMLElement) {
    if (this.disposed) throw new Error("Terminal session has been disposed");
    host.appendChild(this.element);
    if (!this.opened) {
      this.term.open(this.element);
      this.term.textarea?.setAttribute("aria-label", "Remote terminal input");
      this.opened = true;
    }
    this.observer.observe(host);
    this.resize();
    return () => {
      this.observer.unobserve(host);
      if (this.element.parentElement === host) this.element.remove();
    };
  }
  private resize() {
    cancelAnimationFrame(this.frame);
    this.frame = requestAnimationFrame(() => {
      if (
        !this.element.isConnected ||
        !this.element.clientWidth ||
        !this.element.clientHeight
      )
        return;
      this.fit.fit();
      const current = this.session;
      const size = `${this.term.cols}:${this.term.rows}`;
      if (current?.ready && size !== this.lastSize) {
        this.lastSize = size;
        void desktop("terminal_resize", {
          session: current.id,
          cols: this.term.cols,
          rows: this.term.rows,
        }).catch((reason) => this.failed(current, reason));
      }
    });
  }
  disconnect() {
    const current = this.session;
    this.session = null;
    this.term.options.disableStdin = true;
    if (current)
      void desktop("terminal_close", { session: current.id }).catch(() => {});
    this.patch({ active: false, status: "Disconnected" });
  }
  private failed(current: Session, reason: unknown) {
    if (this.session !== current) return;
    this.disconnect();
    this.patch({ error: String(reason), status: "Connection failed" });
  }
  private send(bytes: Uint8Array) {
    const current = this.session;
    if (!current?.ready) return;
    if (current.queued + bytes.length > 1024 * 1024) {
      this.failed(
        current,
        "Terminal input could not keep up. Reconnect before continuing.",
      );
      return;
    }
    current.queued += bytes.length;
    current.writes = current.writes
      .then(async () => {
        for (let offset = 0; offset < bytes.length; offset += 4096) {
          if (this.session !== current) return;
          await desktop("terminal_write", {
            session: current.id,
            data: Array.from(bytes.subarray(offset, offset + 4096)),
          });
        }
      })
      .catch((reason) => this.failed(current, reason))
      .finally(() => {
        current.queued -= bytes.length;
      });
  }
  async connect() {
    if (this.disposed || this.session) return;
    const term = this.term;
    if (this.element.isConnected) this.fit.fit();
    this.patch({ error: "", status: "Connecting…", active: true });
    const current: Session = {
      id: crypto.randomUUID(),
      ready: false,
      queued: 0,
      writes: Promise.resolve(),
    };
    this.lastSize = "";
    this.session = current;
    try {
      // Drain old queued output before resetting for a new shell.
      await this.output;
      if (this.session !== current) return;
      term.reset();
      // Load prompt icons before xterm measures and renders the remote shell.
      await document.fonts.load('12px "Newport Symbols"');
      if (this.session !== current) return;
      if (this.element.isConnected) this.fit.fit();
      await desktop("terminal_open", {
        id: this.serverId,
        session: current.id,
        cols: term.cols,
        rows: term.rows,
      });
      if (this.session !== current) return;
      while (this.session === current) {
        const event = await desktop("terminal_read", {
          session: current.id,
        });
        if (this.session !== current) return;
        if (!event) continue;
        if (event.type === "ready") {
          this.patch({ started: true, status: "Connected" });
          current.ready = true;
          term.options.disableStdin = false;
          if (this.element.isConnected) this.fit.fit();
          await desktop("terminal_resize", {
            session: current.id,
            cols: term.cols,
            rows: term.rows,
          });
          if (this.session !== current) return;
          if (this.element.isConnected && !hasOpenDialog()) term.focus();
        } else if (event.type === "data") {
          // Wait for parsing before pulling more bytes, even while detached.
          this.output = new Promise<void>((resolve) => {
            this.finishWrite = resolve;
            term.write(new Uint8Array(event.data), () => {
              this.finishWrite = undefined;
              resolve();
            });
          });
          await this.output;
        } else if (event.type === "error") {
          throw new Error(event.data);
        } else {
          this.patch({
            status:
              event.data === null
                ? "Shell closed"
                : `Shell exited · ${event.data}`,
          });
          break;
        }
      }
    } catch (reason) {
      if (this.session === current) {
        this.patch({ error: String(reason) });
        this.patch({ status: "Connection failed" });
      }
    } finally {
      // Close late open replies after cancellation or server removal.
      if (this.session === current) {
        this.session = null;
        term.options.disableStdin = true;
        this.patch({ active: false });
      }
      await desktop("terminal_close", { session: current.id }).catch(() => {});
    }
  }

  dispose() {
    if (this.disposed) return;
    this.disconnect();
    this.disposed = true;
    cancelAnimationFrame(this.frame);
    this.observer.disconnect();
    this.appearance.disconnect();
    this.finishWrite?.();
    this.finishWrite = undefined;
    this.term.dispose();
    this.element.remove();
  }
}
export function getTerminalSession(
  key: string,
  serverId: string,
): TerminalSession {
  const existing = terminalResources.get(key);
  if (existing) return existing;
  const session = new TerminalSession(serverId);
  terminalResources.set(key, session);
  return session;
}
