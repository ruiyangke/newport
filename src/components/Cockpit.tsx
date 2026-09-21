import {
  lazy,
  memo,
  Suspense,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { type Server } from "../types";
import { type Log } from "../domain/cockpit";
import { OverviewPanel } from "./OverviewPanel";
import { ContainersPanel } from "./ContainersPanel";
import { LogsPanel } from "./LogsPanel";
import { ServicesCollectionPanel } from "./ServicesCollectionPanel";

const TerminalPanel = lazy(() => import("./TerminalPanel"));

export const Cockpit = memo(function Cockpit({
  server,
  tab = "overview",
}: {
  server: Server;
  tab?: string;
}) {
  const [log, setLog] = useState<Log | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const logOrigin = useRef<{
    element: HTMLElement | null;
    scroll: number;
  } | null>(null);
  const openLogs = (next: Log, origin: HTMLElement) => {
    logOrigin.current = {
      element: origin,
      scroll: root.current?.closest(".workspace-scroll")?.scrollTop ?? 0,
    };
    setLog(next);
  };
  useLayoutEffect(() => {
    const viewport = root.current?.closest(".workspace-scroll");
    if (log) {
      root.current
        ?.querySelector<HTMLElement>('[aria-label="Close logs"]')
        ?.focus({ preventScroll: true });
      if (viewport) viewport.scrollTop = 0;
    } else if (logOrigin.current) {
      logOrigin.current.element?.focus({ preventScroll: true });
      if (viewport) viewport.scrollTop = logOrigin.current.scroll;
      logOrigin.current = null;
    }
  }, [log]);
  useEffect(() => setLog(null), [tab]);
  return (
    <div className="cockpit" ref={root}>
      {tab === "services" && (
        <div hidden={!!log}>
          <ServicesCollectionPanel id={server.id} onLogs={openLogs} />
        </div>
      )}
      {tab === "containers" && (
        <div hidden={!!log}>
          <ContainersPanel server={server} onLogs={openLogs} />
        </div>
      )}
      {log ? (
        <LogsPanel
          key={`${log.source}:${log.target}`}
          id={server.id}
          log={log}
          close={() => setLog(null)}
        />
      ) : tab === "overview" ? (
        <OverviewPanel id={server.id} />
      ) : tab === "services" || tab === "containers" ? null : (
        <Suspense fallback={<p className="muted">Loading terminal…</p>}>
          <TerminalPanel key={server.id} server={server} />
        </Suspense>
      )}
      {(tab === "overview" || tab === "services") && (
        <p className="cockpit-footnote">
          Monitoring is read-only. No agent is installed on the server.
        </p>
      )}
    </div>
  );
});
