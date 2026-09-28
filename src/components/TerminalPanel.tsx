import { useLayoutEffect, useRef, useState } from "react";
import { useSelector } from "@tanstack/react-store";
import { type Server } from "../types";
import { useServerScope } from "../query/keys";
import { getTerminalSession, type TerminalSession } from "../terminal/session";
import { Button } from "./controls";
import "@xterm/xterm/css/xterm.css";
import "./terminal.css";

export default function TerminalPanel({ server }: { server: Server }) {
  const { connection } = useServerScope(server.id);
  const [session, setSession] = useState<{
    key: string;
    value: TerminalSession;
  } | null>(null);
  useLayoutEffect(() => {
    setSession({
      key: connection,
      value: getTerminalSession(connection, server.id),
    });
  }, [connection, server.id]);
  return session?.key === connection ? (
    <TerminalView key={connection} session={session.value} server={server} />
  ) : null;
}
function TerminalView({
  session,
  server,
}: {
  session: TerminalSession;
  server: Server;
}) {
  const host = useRef<HTMLDivElement>(null);
  const { status, error, active, started } = useSelector(
    session.state,
    (state) => state,
  );
  useLayoutEffect(() => session.attach(host.current!), [session]);
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
            onClick={() =>
              active ? session.disconnect() : void session.connect()
            }
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
            Keeps running while you browse
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
          className="terminal-mount"
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
                onClick={() => void session.connect()}
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
