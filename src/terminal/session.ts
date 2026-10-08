import { createStore } from "@tanstack/react-store";
import { Terminal, type ILinkHandler } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { UnicodeGraphemesAddon } from "@xterm/addon-unicode-graphemes";
import { WebglAddon } from "@xterm/addon-webgl";
import { toast } from "sonner";
import { hasOpenDialog } from "../overlays";
import { desktop } from "../api/desktop";
import { terminalResources } from "./registry";

/** Remote output is untrusted: only a deliberate modified click opens http(s). */
async function openExternalLink(url: string) {
  if (!/^https?:\/\//i.test(url)) return;
  try {
    await desktop("open_url", { url });
  } catch (reason) {
    toast.error(`Could not open the link. ${String(reason)}`);
  }
}
const linkHandler: ILinkHandler = {
  activate: (event, text) => {
    if (!event.metaKey && !event.ctrlKey) return;
    void openExternalLink(text);
  },
  allowNonHttpProtocols: false,
};
// A dropped full-screen program leaves modes behind; clear them so the
// replacement shell and its notice start on the normal screen.
const reconnectReset =
  "\x1b[?1049l\x1b[r\x1b[0m\x1b[?25h\x1b[?2004l" +
  "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l" +
  "\x1b[?1l\x1b[4l\x1b[?6l\x1b[?7h";
function reconnectNotice() {
  return `${reconnectReset}\r\n\x1b[33m[SSH connection lost. Reconnecting…]\x1b[0m\r\n`;
}
function reconnectedNotice() {
  return "\x1b[32m[Reconnected · new shell]\x1b[0m\r\n";
}

type Session = {
  id: string;
  ready: boolean;
  lost: boolean;
  queued: number;
  writes: Promise<void>;
};
/**
 * Poll interval (ms) while a session is backgrounded -- window hidden or
 * view detached. Idle attached sessions already wait on the native 500 ms
 * read timeout; backgrounded ones rest here too, so eight idle sessions
 * cost ~4 IPC/s instead of ~16.
 */
export const TERMINAL_BACKGROUND_POLL_MS = 2_000;
export class TerminalSession {
  readonly state = createStore({
    status: "Disconnected",
    error: "",
    active: false,
    started: false,
    title: "",
    /** True while an interrupted transport is being retried. */
    reconnecting: false,
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
    // The Unicode graphemes addon reads `terminal.unicode`, which xterm gates
    // behind this flag; nothing else in the app relies on proposed API.
    allowProposedApi: true,
    linkHandler,
  });
  readonly fit = new FitAddon();
  readonly search = new SearchAddon();
  private session: Session | null = null;
  private output: Promise<void> = Promise.resolve();
  private finishWrite?: () => void;
  private opened = false;
  private disposed = false;
  private frame = 0;
  private lastSize = "";
  private gpu?: WebglAddon;
  private appearance: MutationObserver;
  private observer: ResizeObserver;
  constructor(private serverId: string) {
    this.element.dataset.slot = "terminal-surface";
    this.element.className =
      "min-h-0 min-w-0 flex-1 overflow-hidden [&_.xterm]:h-full [&_.xterm]:px-[10px] [&_.xterm]:py-[8px] [&_.xterm-viewport]:bg-background! [&_.xterm-viewport]:overscroll-contain";
    this.term.loadAddon(this.fit);
    this.term.loadAddon(this.search);
    // Unicode 15 widths with grapheme clustering: TUIs place their cursor by
    // display width, and the default tables mis-size emoji and variation
    // selectors, which drifts every redraw.
    this.term.loadAddon(new UnicodeGraphemesAddon());
    this.term.onData((value) => this.send(new TextEncoder().encode(value)));
    this.term.onBinary((value) =>
      this.send(Uint8Array.from(value, (c) => c.charCodeAt(0))),
    );
    // Shells and full-screen programs name their tab with OSC 0/2.
    this.term.onTitleChange((value) =>
      this.patch({ title: terminalTitle(value) }),
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
    this.enableGpu();
    this.observer.observe(host);
    this.resize();
    // A tab switched into view resumes at full speed immediately.
    this.wakeRest?.();
    return () => {
      this.observer.unobserve(host);
      this.disableGpu();
      if (this.element.parentElement === host) this.element.remove();
    };
  }
  /** Hardware rendering exists only while the view is mounted. */
  private enableGpu() {
    if (this.gpu || this.disposed) return;
    try {
      const addon = new WebglAddon();
      // A lost context falls back to the DOM renderer instead of a dead canvas.
      addon.onContextLoss(() => {
        if (this.gpu === addon) this.disableGpu();
      });
      this.term.loadAddon(addon);
      this.gpu = addon;
      void this.refreshGlyphAtlas(addon);
    } catch {
      // WebGL2 is unavailable; xterm keeps its DOM renderer.
    }
  }
  private disableGpu() {
    const addon = this.gpu;
    this.gpu = undefined;
    addon?.dispose();
  }
  private async refreshGlyphAtlas(addon: WebglAddon) {
    try {
      // The atlas may be built before the Nerd Font finishes loading.
      await document.fonts.load('12px "Newport Symbols"');
    } catch {
      return;
    }
    if (this.gpu === addon && !this.disposed) addon.clearTextureAtlas();
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
    this.patch({ active: false, status: "Disconnected", reconnecting: false });
    this.wakeRest?.();
  }
  /**
   * A hidden window or a detached view needs no interactive latency. The
   * native output channel is bounded with a blocking sender, so slowing
   * the poll only applies backpressure -- output is delayed, never lost.
   */
  private backgrounded() {
    return document.hidden || !this.element.isConnected;
  }
  private failed(current: Session, reason: unknown) {
    if (this.session !== current) return;
    this.disconnect();
    this.patch({ error: String(reason), status: "Connection failed" });
  }
  /** Queue terminal output and wait for parsing, even while detached. */
  private async render(data: string | Uint8Array) {
    this.output = new Promise<void>((resolve) => {
      this.finishWrite = resolve;
      this.term.write(data, () => {
        this.finishWrite = undefined;
        resolve();
      });
    });
    await this.output;
  }
  private pendingInput = "";
  /** Wakes a backgrounded read loop early; attach/disconnect preempt it. */
  private wakeRest: (() => void) | null = null;
  /**
   * Types text into the shell, without pressing Enter, as soon as it is
   * ready: the reader reviews a handed-over command before running it.
   */
  type(text: string) {
    if (this.session?.ready) this.send(new TextEncoder().encode(text));
    else this.pendingInput += text;
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
          // A reconnect invalidates input aimed at the dropped shell.
          if (this.session !== current || !current.ready) return;
          await desktop("terminal_write", {
            session: current.id,
            data: Array.from(bytes.subarray(offset, offset + 4096)),
          });
        }
      })
      .catch((reason) => {
        // A write that raced a reconnect is not fatal; the read loop reports
        // a session that is really gone.
        if (current.ready) this.failed(current, reason);
      })
      .finally(() => {
        current.queued -= bytes.length;
      });
  }
  async connect() {
    if (this.disposed || this.session) return;
    const term = this.term;
    if (this.element.isConnected) this.fit.fit();
    this.patch({
      error: "",
      status: "Connecting…",
      active: true,
      title: "",
      reconnecting: false,
    });
    const current: Session = {
      id: crypto.randomUUID(),
      ready: false,
      lost: false,
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
        if (!event) {
          // An empty poll rests below when backgrounded, like any iteration.
        } else if (event.type === "ready") {
          const resumed = current.lost;
          current.lost = false;
          this.patch({
            started: true,
            status: "Connected",
            reconnecting: false,
          });
          current.ready = true;
          term.options.disableStdin = false;
          if (resumed) await this.render(reconnectedNotice());
          if (this.element.isConnected) this.fit.fit();
          await desktop("terminal_resize", {
            session: current.id,
            cols: term.cols,
            rows: term.rows,
          });
          if (this.session !== current) return;
          if (this.pendingInput) {
            const text = this.pendingInput;
            this.pendingInput = "";
            this.send(new TextEncoder().encode(text));
          }
          if (this.element.isConnected && !hasOpenDialog()) term.focus();
        } else if (event.type === "data") {
          await this.render(new Uint8Array(event.data));
        } else if (event.type === "reconnecting") {
          current.ready = false;
          term.options.disableStdin = true;
          this.patch({
            error: "",
            status: `Reconnecting · ${event.data.attempt}`,
            reconnecting: true,
          });
          if (!current.lost) {
            current.lost = true;
            await this.render(reconnectNotice());
          }
        } else if (event.type === "error") {
          throw new Error(event.data);
        } else {
          this.patch({
            status:
              event.data === null
                ? "Shell closed"
                : `Shell exited · ${event.data}`,
            reconnecting: false,
          });
          break;
        }
        // A hidden or detached tab needs no interactive latency: rest here
        // so idle background sessions stop polling at full rate. Exits and
        // errors leave immediately above; the native output channel is
        // bounded with a blocking sender, so a slower reader only applies
        // backpressure -- output is delayed, never lost.
        if (this.session === current && this.backgrounded()) {
          await new Promise<void>((resolve) => {
            const done = () => {
              if (this.wakeRest === done) this.wakeRest = null;
              resolve();
            };
            this.wakeRest = done;
            setTimeout(done, TERMINAL_BACKGROUND_POLL_MS);
          });
          if (this.session !== current) return;
        }
      }
    } catch (reason) {
      if (this.session === current) {
        this.patch({ error: String(reason), reconnecting: false });
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
    this.disableGpu();
    this.finishWrite?.();
    this.finishWrite = undefined;
    this.term.dispose();
    this.element.remove();
  }
}
/** Bidi overrides could reorder a tab label; drop them with other controls. */
function isBidiControl(code: number) {
  return (
    code === 0x061c ||
    code === 0x200e ||
    code === 0x200f ||
    (code >= 0x202a && code <= 0x202e) ||
    (code >= 0x2066 && code <= 0x2069)
  );
}
/** OSC 0/2 titles come from the remote shell: keep them bounded and single-line. */
export function terminalTitle(value: string): string {
  let result = "";
  for (let index = 0; index < value.length; index++) {
    const code = value.charCodeAt(index);
    if (code >= 32 && !(code >= 127 && code <= 159) && !isBidiControl(code))
      result += value[index];
  }
  result = result.trim();
  if (result.length > 120) {
    // Never split a surrogate pair at the cut.
    const end =
      result.charCodeAt(119) >= 0xd800 && result.charCodeAt(119) <= 0xdbff
        ? 119
        : 120;
    result = result.slice(0, end);
  }
  return result;
}
export function getTerminalSession(
  connection: string,
  tabId: string,
  serverId: string,
): TerminalSession {
  let sessions = terminalResources.get(connection);
  if (!sessions) {
    sessions = new Map();
    terminalResources.set(connection, sessions);
  }
  let session = sessions.get(tabId);
  if (!session) {
    session = new TerminalSession(serverId);
    sessions.set(tabId, session);
  }
  return session;
}
