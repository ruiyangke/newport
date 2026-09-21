import { useCallback, useEffect, useRef, useState } from "react";
import { desktop } from "../api/desktop";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Button } from "./controls";
import { type Server } from "../types";
import "@xterm/xterm/css/xterm.css";
import "./terminal.css";

type Session = {
  id: string;
  ready: boolean;
  queued: number;
  writes: Promise<void>;
};

export default function TerminalPanel({ server }: { server: Server }) {
  const host = useRef<HTMLDivElement>(null);
  const terminal = useRef<Terminal | null>(null);
  const fit = useRef<FitAddon | null>(null);
  const session = useRef<Session | null>(null);
  const [status, setStatus] = useState("Disconnected");
  const [error, setError] = useState("");
  const [active, setActive] = useState(false);
  const [started, setStarted] = useState(false);
  const mounted = useRef(false);

  const disconnect = useCallback(() => {
    const current = session.current;
    session.current = null;
    if (terminal.current) terminal.current.options.disableStdin = true;
    if (current)
      void desktop("terminal_close", { session: current.id }).catch(() => {});
    if (mounted.current) {
      setActive(false);
      setStatus("Disconnected");
    }
  }, []);
  const failed = useCallback(
    (current: Session, reason: unknown) => {
      if (session.current !== current) return;
      disconnect();
      setError(String(reason));
      setStatus("Connection failed");
    },
    [disconnect],
  );
  const send = useCallback(
    (bytes: Uint8Array) => {
      const current = session.current;
      if (!current?.ready) return;
      if (current.queued + bytes.length > 1024 * 1024) {
        failed(
          current,
          "Terminal input could not keep up. Reconnect before continuing.",
        );
        return;
      }
      current.queued += bytes.length;
      current.writes = current.writes
        .then(async () => {
          for (let offset = 0; offset < bytes.length; offset += 4096) {
            if (session.current !== current) return;
            await desktop("terminal_write", {
              session: current.id,
              data: Array.from(bytes.subarray(offset, offset + 4096)),
            });
          }
        })
        .catch((reason) => failed(current, reason))
        .finally(() => {
          current.queued -= bytes.length;
        });
    },
    [failed],
  );

  useEffect(() => {
    mounted.current = true;
    const term = new Terminal({
      fontFamily: 'Menlo, Monaco, "Porthop Symbols", ui-monospace, monospace',
      fontSize: 12,
      cursorBlink: false,
      scrollback: 5000,
      screenReaderMode: true,
      disableStdin: true,
      minimumContrastRatio: 4.5,
      allowProposedApi: false,
    });
    const addon = new FitAddon();
    term.loadAddon(addon);
    terminal.current = term;
    fit.current = addon;
    const theme = () => {
      const dark = document.documentElement.classList.contains("dark");
      term.options.theme = dark
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
    };
    theme();
    term.open(host.current!);
    term.textarea?.setAttribute("aria-label", "Remote terminal input");
    const encoder = new TextEncoder();
    const data = term.onData((value) => send(encoder.encode(value)));
    const binary = term.onBinary((value) =>
      send(Uint8Array.from(value, (c) => c.charCodeAt(0))),
    );
    let frame = 0;
    let lastSize = "";
    const resize = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        if (!host.current?.clientWidth || !host.current.clientHeight) return;
        addon.fit();
        const current = session.current;
        const size = `${term.cols}:${term.rows}`;
        if (current?.ready && size !== lastSize) {
          lastSize = size;
          void desktop("terminal_resize", {
            session: current.id,
            cols: term.cols,
            rows: term.rows,
          }).catch((reason) => failed(current, reason));
        }
      });
    };
    const observer = new ResizeObserver(resize);
    observer.observe(host.current!);
    const appearance = new MutationObserver(theme);
    appearance.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });
    resize();
    return () => {
      mounted.current = false;
      disconnect();
      cancelAnimationFrame(frame);
      observer.disconnect();
      appearance.disconnect();
      data.dispose();
      binary.dispose();
      term.dispose();
      terminal.current = null;
      fit.current = null;
    };
    // Each server owns a fresh terminal, and cleanup always closes its SSH session.
  }, [server.id, disconnect, failed, send]);

  const connect = async () => {
    if (session.current || !terminal.current) return;
    const term = terminal.current;
    fit.current?.fit();
    term.reset();
    setError("");
    setStatus("Connecting…");
    setActive(true);
    const current: Session = {
      id: crypto.randomUUID(),
      ready: false,
      queued: 0,
      writes: Promise.resolve(),
    };
    session.current = current;
    try {
      // Load prompt icons before xterm measures and renders the remote shell.
      await document.fonts.load('12px "Porthop Symbols"');
      if (session.current !== current) return;
      await desktop("terminal_open", {
        id: server.id,
        session: current.id,
        cols: term.cols,
        rows: term.rows,
      });
      if (session.current !== current) return;
      while (session.current === current) {
        const event = await desktop("terminal_read", {
          session: current.id,
        });
        if (session.current !== current) return;
        if (!event) continue;
        if (event.type === "ready") {
          setStarted(true);
          current.ready = true;
          term.options.disableStdin = false;
          setStatus("Connected");
          fit.current?.fit();
          await desktop("terminal_resize", {
            session: current.id,
            cols: term.cols,
            rows: term.rows,
          });
          term.focus();
        } else if (event.type === "data") {
          // Wait for rendering before pulling more bytes: bounded end-to-end flow control.
          await new Promise<void>((resolve) =>
            term.write(new Uint8Array(event.data), resolve),
          );
        } else if (event.type === "error") {
          throw new Error(event.data);
        } else {
          setStatus(
            event.data === null
              ? "Shell closed"
              : `Shell exited · ${event.data}`,
          );
          break;
        }
      }
    } catch (reason) {
      if (session.current === current) {
        setError(String(reason));
        setStatus("Connection failed");
      }
    } finally {
      // Also closes a late open reply after navigation or cancelling connection setup.
      await desktop("terminal_close", { session: current.id }).catch(() => {});
      if (session.current === current) {
        session.current = null;
        term.options.disableStdin = true;
        setActive(false);
      }
    }
  };

  return (
    <section className="remote-terminal" aria-label="SSH terminal">
      <div className="terminal-toolbar">
        <div className="terminal-identity">
          <h2>SSH terminal</h2>
        </div>
        <span className="terminal-status" role="status">
          {status}
        </span>
        {(active || started) && (
          <Button
            variant={active ? "outline" : "default"}
            onClick={active ? disconnect : () => void connect()}
          >
            {active
              ? status === "Connecting…"
                ? "Cancel connection"
                : "Disconnect terminal"
              : "Connect terminal"}
          </Button>
        )}
        {active && (
          <span className="terminal-lifecycle">
            Disconnects when you leave this view
          </span>
        )}
      </div>
      {started && !active && !error && (
        <p className="terminal-transcript">
          Session ended. Reconnect to open a new shell.
        </p>
      )}
      {error && (
        <p className="cockpit-error" role="alert">
          {error}
        </p>
      )}
      <div className="terminal-frame">
        <div
          className="terminal-surface"
          ref={host}
          style={{ visibility: started ? "visible" : "hidden" }}
        />
        {!started && (
          <div className="terminal-welcome">
            <h3>{active ? "Connecting…" : "Open a remote shell"}</h3>
            <p>
              {server.sshUser}@{server.sshHost}
            </p>
            {!active && (
              <Button
                variant="default"
                className="primary"
                onClick={() => void connect()}
              >
                Connect terminal
              </Button>
            )}
          </div>
        )}
      </div>
      <p className="cockpit-footnote">
        Background jobs may continue after disconnecting.
      </p>
    </section>
  );
}
