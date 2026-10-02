import { useLayoutEffect, useRef, useState } from "react";
import { createStore, useSelector } from "@tanstack/react-store";
import {
  Maximize2,
  Minimize2,
  Plus,
  Terminal as TerminalIcon,
  X,
} from "lucide-react";
import { type Server } from "../types";
import { useServerScope } from "../query/keys";
import {
  dropTerminalSession,
  getTerminalLayout,
  peekTerminalSession,
  saveTerminalLayout,
  tabAfterClose,
  type TerminalLayout,
  type TerminalTab,
} from "../terminal/registry";
import { getTerminalSession, type TerminalSession } from "../terminal/session";
import { immersiveStore, setImmersive } from "../immersive";
import { Button } from "./controls";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "./ui/tabs";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "./ui/context-menu";
import "@xterm/xterm/css/xterm.css";
import "./terminal.css";

/** The native backend accepts eight concurrent terminal sessions. */
const MAX_TERMINALS = 8;

// A tab renders before its session exists and after it closes; the fallback
// keeps the subscription unconditional.
const emptyTabState = createStore({
  status: "",
  error: "",
  title: "",
  active: false,
});

export default function TerminalPanel({ server }: { server: Server }) {
  const { connection } = useServerScope(server.id);
  const [state, setState] = useState<{
    connection: string;
    layout: TerminalLayout;
  } | null>(null);
  useLayoutEffect(() => {
    setState({ connection, layout: getTerminalLayout(connection) });
  }, [connection]);
  if (state?.connection !== connection) return null;
  return (
    <TerminalWorkspace
      key={connection}
      connection={connection}
      layout={state.layout}
      update={(layout) => {
        saveTerminalLayout(connection, layout);
        setState({ connection, layout });
      }}
      server={server}
    />
  );
}

function TerminalWorkspace({
  connection,
  layout,
  update,
  server,
}: {
  connection: string;
  layout: TerminalLayout;
  update: (layout: TerminalLayout) => void;
  server: Server;
}) {
  const immersive = useSelector(immersiveStore, (state) => state.active);
  const activeKey = `${connection}:${layout.active}`;
  const [active, setActive] = useState<{
    key: string;
    session: TerminalSession;
  } | null>(null);
  useLayoutEffect(() => {
    const existing = peekTerminalSession(connection, layout.active);
    const session =
      existing ?? getTerminalSession(connection, layout.active, server.id);
    setActive({ key: activeKey, session });
    // A new tab opens its shell immediately. Sessions that ended on their own
    // (or were disconnected) wait for an explicit reconnect from the tab menu,
    // so a failing host cannot be hammered.
    if (!existing) void session.connect();
  }, [activeKey, connection, layout.active, server.id]);
  const session = active?.session;

  // Inactive sessions keep running; only the visible one is attached.
  const selectTab = (id: string) => {
    if (id !== layout.active) update({ ...layout, active: id });
  };
  const addTab = () => {
    const id = crypto.randomUUID();
    update({
      tabs: [...layout.tabs, { id, name: `Terminal ${layout.next}` }],
      active: id,
      next: layout.next + 1,
    });
  };
  const closeTab = (id: string) => {
    // The last tab has no close control; the layout always keeps a session.
    if (layout.tabs.length <= 1) return;
    dropTerminalSession(connection, id);
    const tabs = layout.tabs.filter((tab) => tab.id !== id);
    update({
      ...layout,
      tabs,
      active: tabAfterClose(layout.tabs, id, layout.active),
    });
  };
  const renameTab = (id: string, custom?: string) => {
    update({
      ...layout,
      tabs: layout.tabs.map((tab) =>
        tab.id === id ? { ...tab, custom } : tab,
      ),
    });
  };

  return (
    <Tabs
      className="remote-terminal"
      aria-label="SSH terminal"
      value={layout.active}
      onValueChange={selectTab}
    >
      {session && (
        <TerminalView
          session={session}
          server={server}
          immersive={immersive}
          layout={layout}
          connection={connection}
          onAdd={addTab}
          onClose={closeTab}
          onRename={renameTab}
          onImmersive={() => setImmersive(!immersive)}
        />
      )}
    </Tabs>
  );
}

function statusTone(status: string, error: string) {
  if (error) return "error";
  if (status === "Connected") return "connected";
  if (status === "Connecting…") return "connecting";
  return "";
}

/** Custom name, then the shell's OSC title, then the default name. */
function tabDisplayName(connection: string, tab: TerminalTab): string {
  const session = peekTerminalSession(connection, tab.id);
  return tab.custom || session?.state.state.title || tab.name;
}

function TerminalTabs({
  connection,
  layout,
  dragRegion,
  onAdd,
  onClose,
  onRename,
}: {
  connection: string;
  layout: TerminalLayout;
  dragRegion: boolean;
  onAdd: () => void;
  onClose: (id: string) => void;
  onRename: (id: string, custom?: string) => void;
}) {
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const startRename = (tab: TerminalTab) => {
    setDraft(tabDisplayName(connection, tab));
    setEditing(tab.id);
  };
  const commitRename = (tab: TerminalTab) => {
    // Empty text clears the custom name and returns to the shell's title.
    onRename(tab.id, draft.trim().slice(0, 60) || undefined);
    setEditing(null);
  };
  return (
    <div className="terminal-tabs">
      <TabsList
        className="terminal-tablist"
        aria-label="Terminal sessions"
        {...(dragRegion ? { "data-tauri-drag-region": true } : {})}
      >
        {layout.tabs.map((tab) =>
          editing === tab.id ? (
            <div key={tab.id} className="terminal-tab" data-editing="">
              <input
                className="terminal-tab-input"
                value={draft}
                autoFocus
                maxLength={60}
                aria-label={`Rename ${tab.name}`}
                onFocus={(event) => event.currentTarget.select()}
                onChange={(event) => setDraft(event.target.value)}
                onBlur={() => commitRename(tab)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    commitRename(tab);
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    setEditing(null);
                  }
                }}
              />
            </div>
          ) : (
            <TerminalTabItem
              key={tab.id}
              tab={tab}
              connection={connection}
              active={tab.id === layout.active}
              canClose={layout.tabs.length > 1}
              onClose={() => onClose(tab.id)}
              onRename={() => startRename(tab)}
              onResetName={() => onRename(tab.id, undefined)}
            />
          ),
        )}
      </TabsList>
      <Button
        size="icon-xs"
        variant="ghost"
        className="terminal-tab-add"
        aria-label="New terminal"
        disabled={layout.tabs.length >= MAX_TERMINALS}
        onClick={onAdd}
      >
        <Plus />
      </Button>
    </div>
  );
}

function TerminalTabItem({
  tab,
  connection,
  active,
  canClose,
  onClose,
  onRename,
  onResetName,
}: {
  tab: TerminalTab;
  connection: string;
  active: boolean;
  canClose: boolean;
  onClose: () => void;
  onRename: () => void;
  onResetName: () => void;
}) {
  const session = peekTerminalSession(connection, tab.id);
  const {
    status,
    error,
    title,
    active: sessionActive,
  } = useSelector(session?.state ?? emptyTabState, (state) => state);
  const name = tab.custom || title || tab.name;
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div className="terminal-tab" data-active={active ? "" : undefined}>
          <TabsTrigger
            value={tab.id}
            className="terminal-tab-select"
            title={name}
            onClick={(event) => {
              // Pointer selection resumes typing; keyboard navigation stays on
              // the trigger so arrows/Home/End can continue through the list.
              // The frame swaps after this event, so focus once it attached.
              if (event.detail > 0 && session?.state.state.started)
                requestAnimationFrame(() => {
                  // A double-click opened a rename; never steal its focus.
                  if (
                    document.activeElement?.classList.contains(
                      "terminal-tab-input",
                    )
                  )
                    return;
                  session.term.focus();
                });
            }}
            onDoubleClick={onRename}
            onAuxClick={(event) => {
              if (event.button === 1 && canClose) {
                event.preventDefault();
                onClose();
              }
            }}
          >
            <span
              className={`status-dot ${statusTone(status, error)}`}
              aria-hidden="true"
            />
            <span className="terminal-tab-name">{name}</span>
          </TabsTrigger>
          {canClose && (
            <button
              type="button"
              className="terminal-tab-close"
              aria-label={`Close ${name}`}
              onClick={onClose}
            >
              <X size={11} aria-hidden="true" />
            </button>
          )}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent aria-label={`Actions for ${name}`}>
        <ContextMenuItem onSelect={onRename}>Rename</ContextMenuItem>
        {tab.custom && (
          <ContextMenuItem onSelect={onResetName}>Reset name</ContextMenuItem>
        )}
        <ContextMenuSeparator />
        <ContextMenuItem
          disabled={sessionActive}
          onSelect={() => void session?.connect()}
        >
          Reconnect
        </ContextMenuItem>
        <ContextMenuItem
          disabled={!sessionActive}
          title="Background jobs may continue after disconnecting."
          onSelect={() => session?.disconnect()}
        >
          Disconnect
        </ContextMenuItem>
        <ContextMenuItem
          variant="destructive"
          disabled={!canClose}
          onSelect={onClose}
        >
          Close
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

function TerminalView({
  session,
  server,
  immersive,
  layout,
  connection,
  onAdd,
  onClose,
  onRename,
  onImmersive,
}: {
  session: TerminalSession;
  server: Server;
  immersive: boolean;
  layout: TerminalLayout;
  connection: string;
  onAdd: () => void;
  onClose: (id: string) => void;
  onRename: (id: string, custom?: string) => void;
  onImmersive: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const { status, error, active, started } = useSelector(
    session.state,
    (state) => state,
  );
  useLayoutEffect(() => session.attach(host.current!), [session]);
  // Steady states live on the tab dots; only transitions and failures earn
  // space in the bar. The text stays in the tree for screen readers.
  const steady = status === "Connected" || status === "Disconnected";
  return (
    <>
      <div
        className="terminal-bar"
        {...(immersive ? { "data-tauri-drag-region": true } : {})}
      >
        <TerminalTabs
          connection={connection}
          layout={layout}
          dragRegion={immersive}
          onAdd={onAdd}
          onClose={onClose}
          onRename={onRename}
        />
        <div
          className="terminal-actions"
          {...(immersive ? { "data-tauri-drag-region": true } : {})}
        >
          <span
            className={`status terminal-state ${statusTone(status, error)}${steady ? " sr-only" : ""}`}
            role="status"
          >
            <span className="status-dot" aria-hidden="true" />
            <span className="terminal-status">{status}</span>
          </span>
          <Button
            size="icon-xs"
            variant="ghost"
            aria-label={
              immersive ? "Exit immersive mode" : "Enter immersive mode"
            }
            title={
              immersive
                ? "Show the sidebar and toolbar"
                : "Hide the sidebar and toolbar"
            }
            onClick={onImmersive}
          >
            {immersive ? <Minimize2 /> : <Maximize2 />}
          </Button>
          {status === "Connecting…" && (
            <Button
              size="xs"
              variant="outline"
              onClick={() => session.disconnect()}
            >
              Cancel connection
            </Button>
          )}
        </div>
      </div>
      {error && (
        <p className="terminal-error cockpit-error" role="alert">
          {error}
        </p>
      )}
      <TabsContent value={layout.active} className="terminal-frame">
        <div
          className="terminal-mount"
          ref={host}
          style={{ visibility: started ? "visible" : "hidden" }}
        />
        {!started && (
          <div className="terminal-welcome">
            <TerminalIcon
              className="terminal-mark"
              size={28}
              strokeWidth={1.4}
              aria-hidden="true"
            />
            <h3>
              {active
                ? "Connecting…"
                : error
                  ? "Connection failed"
                  : "Open a remote shell"}
            </h3>
            <p className="terminal-endpoint">
              {server.sshUser}@{server.sshHost}
            </p>
          </div>
        )}
      </TabsContent>
      {layout.tabs
        .filter((tab) => tab.id !== layout.active)
        .map((tab) => (
          // Radix hides these, but they give every inactive trigger an
          // aria-controls target that resolves.
          <TabsContent key={tab.id} value={tab.id} />
        ))}
    </>
  );
}
