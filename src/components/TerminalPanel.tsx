import { StatusIndicator } from "./StatusIndicator";
import { StatusDot } from "./StatusDot";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createStore, useSelector } from "@tanstack/react-store";
import type { ISearchOptions } from "@xterm/addon-search";
import {
  CaseSensitive,
  ChevronDown,
  ChevronUp,
  Maximize2,
  Minimize2,
  Plus,
  Terminal as TerminalIcon,
  X,
} from "lucide-react";
import { type Server } from "../types";
import { cn } from "../lib/utils";
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
import {
  pendingTerminalCommands,
  takeTerminalCommand,
} from "../terminal/pending";
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
import { TerminalTabFrame } from "./TerminalTabFrame";

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
  // Another page handed over a command: open it in a fresh tab, typed, and
  // run only when that page asked (never for a password prompt).
  const pending = useSelector(
    pendingTerminalCommands,
    (state) => state[server.id],
  );
  useEffect(() => {
    if (pending === undefined) return;
    const handed = takeTerminalCommand(server.id);
    if (handed === undefined) return;
    const id = crypto.randomUUID();
    const session = getTerminalSession(connection, id, server.id);
    session.type(handed.run ? `${handed.command}\r` : handed.command);
    void session.connect();
    update({
      tabs: [
        ...layout.tabs,
        { id, name: handed.title ?? `Terminal ${layout.next}` },
      ],
      active: id,
      next: layout.next + 1,
    });
    // `layout`/`update` are read at hand-over time only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pending, connection, server.id]);
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
      data-slot="remote-terminal"
      className="mt-0 flex min-h-0 min-w-0 flex-1 flex-col gap-0 p-0"
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

function statusTone(status: string, error: string, reconnecting: boolean) {
  if (error) return "danger";
  if (reconnecting || status === "Connecting…") return "warning";
  if (status === "Connected") return "success";
  return "muted";
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
    <div
      data-slot="terminal-tabs"
      className="flex min-w-0 items-center gap-[2px] overflow-x-auto scrollbar-none [&::-webkit-scrollbar]:hidden"
    >
      <TabsList
        data-slot="terminal-tablist"
        className="flex min-w-0 flex-row items-center justify-start gap-[2px] bg-transparent p-0 group-data-vertical/tabs:flex-row"
        aria-label="Terminal sessions"
        {...(dragRegion ? { "data-tauri-drag-region": true } : {})}
      >
        {layout.tabs.map((tab) =>
          editing === tab.id ? (
            <TerminalTabFrame key={tab.id} data-editing="">
              <input
                data-slot="terminal-tab-input"
                className="h-[26px] w-[132px] min-w-0 select-text rounded-[5px] border-0 border-current bg-transparent px-[8px] py-0 text-[12px] text-foreground focus:outline-2 focus:-outline-offset-1 focus:outline-ring"
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
            </TerminalTabFrame>
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
  const { title, active: sessionActive } = useSelector(
    session?.state ?? emptyTabState,
    (state) => state,
  );
  const name = tab.custom || title || tab.name;
  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <TerminalTabFrame data-active={active ? "" : undefined}>
          <TabsTrigger
            value={tab.id}
            data-slot="terminal-tab-select"
            className="flex h-[26px] w-auto min-w-0 flex-initial items-center gap-[6px] rounded-[5px] border-0 border-current bg-transparent! py-0 pr-[4px] pl-[8px] text-[12px] text-muted-foreground [box-shadow:none]! group-data-vertical/tabs:w-auto group-data-[active]/terminal-tab:font-medium group-data-[active]/terminal-tab:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring"
            title={name}
            onClick={(event) => {
              // Pointer selection resumes typing; keyboard navigation stays on
              // the trigger so arrows/Home/End can continue through the list.
              // The frame swaps after this event, so focus once it attached.
              if (event.detail > 0 && session?.state.state.started)
                requestAnimationFrame(() => {
                  // A double-click opened a rename; never steal its focus.
                  if (
                    document.activeElement?.getAttribute("data-slot") ===
                    "terminal-tab-input"
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
            <StatusDot />
            <span data-slot="terminal-tab-name" className="truncate">
              {name}
            </span>
          </TabsTrigger>
          {canClose && (
            <button
              type="button"
              data-slot="terminal-tab-close"
              className="mr-[3px] grid size-[16px] place-items-center rounded-[4px] border-0 border-current bg-transparent p-0 text-muted-foreground opacity-0 group-hover/terminal-tab:opacity-100 group-data-[active]/terminal-tab:opacity-100 hover:bg-[color-mix(in_srgb,var(--foreground)_12%,transparent)] hover:text-foreground focus-visible:opacity-100 focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring"
              aria-label={`Close ${name}`}
              onClick={onClose}
            >
              <X size={11} aria-hidden="true" />
            </button>
          )}
        </TerminalTabFrame>
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
  const { status, error, active, started, reconnecting } = useSelector(
    session.state,
    (state) => state,
  );
  useLayoutEffect(() => session.attach(host.current!), [session]);
  const [findOpen, setFindOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [results, setResults] = useState<{
    index: number;
    count: number;
  } | null>(null);
  const findInput = useRef<HTMLInputElement>(null);

  // A tab switch ends the previous session's search.
  useEffect(() => {
    setFindOpen(false);
    setQuery("");
    setResults(null);
  }, [session]);

  // Cmd/Ctrl+F belongs to the terminal, never to the remote shell.
  useLayoutEffect(() => {
    session.term.attachCustomKeyEventHandler((event) => {
      if (
        event.type === "keydown" &&
        (event.metaKey || event.ctrlKey) &&
        event.key.toLowerCase() === "f"
      ) {
        event.preventDefault();
        setFindOpen(true);
        return false;
      }
      return true;
    });
    return () => session.term.attachCustomKeyEventHandler(() => true);
  }, [session]);

  useEffect(() => {
    if (!findOpen) return;
    findInput.current?.focus();
    findInput.current?.select();
  }, [findOpen]);

  useEffect(() => {
    if (!findOpen) return;
    const subscription = session.search.onDidChangeResults(
      ({ resultIndex, resultCount }) =>
        setResults({ index: resultIndex, count: resultCount }),
    );
    return () => subscription.dispose();
  }, [findOpen, session]);

  const find = (
    direction: "next" | "previous",
    value: { query: string; caseSensitive: boolean; incremental: boolean },
  ) => {
    if (!value.query) {
      session.search.clearDecorations();
      setResults(null);
      return;
    }
    const dark = document.documentElement.classList.contains("dark");
    const options: ISearchOptions = {
      caseSensitive: value.caseSensitive,
      incremental: value.incremental,
      decorations: {
        // Backgrounds must be solid #RRGGBB (no alpha), so pick tones that
        // stay legible behind the terminal text on each theme.
        matchBackground: dark ? "#8a7a2a" : "#f6e3a1",
        matchBorder: dark ? "#c9b458" : "#d8b43a",
        matchOverviewRuler: dark ? "#c9b458" : "#d8b43a",
        activeMatchBackground: dark ? "#c77800" : "#ffb74d",
        activeMatchBorder: dark ? "#ffb74d" : "#f57c00",
        activeMatchColorOverviewRuler: dark ? "#ffb74d" : "#f57c00",
      },
    };
    if (direction === "next") session.search.findNext(value.query, options);
    else session.search.findPrevious(value.query, options);
  };
  const runFind = (direction: "next" | "previous", incremental = false) =>
    find(direction, { query, caseSensitive, incremental });
  const closeFind = () => {
    session.search.clearDecorations();
    setFindOpen(false);
    setResults(null);
    session.term.focus();
  };
  // Steady states live on the tab dots; only transitions and failures earn
  // space in the bar. The text stays in the tree for screen readers.
  const steady = status === "Connected" || status === "Disconnected";
  return (
    <>
      <div
        data-slot="terminal-bar"
        className={`flex h-[38px] shrink-0 select-none items-center justify-between gap-[8px] border-b border-border pr-[8px] ${immersive ? "pl-[96px]" : "pl-[8px]"}`}
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
          data-slot="terminal-actions"
          className="flex shrink-0 items-center gap-[8px]"
          {...(immersive ? { "data-tauri-drag-region": true } : {})}
        >
          <StatusIndicator
            data-slot="terminal-state"
            tone={statusTone(status, error, reconnecting)}
            className={steady ? "sr-only" : "whitespace-nowrap"}
            role="status"
            title={
              reconnecting
                ? "The SSH connection dropped. Reconnecting automatically."
                : undefined
            }
          >
            <span className="terminal-status">{status}</span>
          </StatusIndicator>
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
          {(status === "Connecting…" || reconnecting) && (
            <Button
              size="xs"
              variant="outline"
              onClick={() => session.disconnect()}
            >
              {reconnecting ? "Stop reconnecting" : "Cancel connection"}
            </Button>
          )}
        </div>
      </div>
      {error && (
        <p
          data-slot="terminal-error"
          className="m-0 shrink-0 border-b border-border bg-[color-mix(in_srgb,var(--red)_6%,transparent)] px-[12px] py-[9px] text-[12px] leading-[1.6] text-(--red) wrap-anywhere"
          role="alert"
        >
          {error}
        </p>
      )}
      <TabsContent
        value={layout.active}
        data-slot="terminal-frame"
        className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden bg-background"
      >
        <div
          data-slot="terminal-mount"
          className={cn(
            "flex min-h-0 min-w-0 flex-1 flex-col",
            started ? "visible" : "invisible",
          )}
          ref={host}
        />
        {!started && (
          <div
            data-slot="terminal-welcome"
            className="absolute inset-0 flex flex-col items-center justify-center gap-[6px] bg-background p-[24px] text-center"
          >
            <TerminalIcon
              data-slot="terminal-mark"
              className="mb-[8px] text-muted-foreground opacity-80"
              size={28}
              strokeWidth={1.4}
              aria-hidden="true"
            />
            <h3 className="m-0 text-[15px] font-semibold">
              {active
                ? "Connecting…"
                : error
                  ? "Connection failed"
                  : "Open a remote shell"}
            </h3>
            <p
              data-slot="terminal-endpoint"
              className="font-[ui-monospace,SFMono-Regular,Menlo,monospace] text-[12px] leading-[1.6] text-muted-foreground wrap-anywhere"
            >
              {server.sshUser}@{server.sshHost}
            </p>
          </div>
        )}
        {findOpen && (
          <div
            data-slot="terminal-find"
            className="absolute top-[8px] right-[12px] z-[2] flex items-center gap-[2px] rounded-[8px] border border-border bg-popover p-[3px] [box-shadow:0_6px_20px_rgb(0_0_0/12%)]"
            role="search"
            aria-label="Find"
          >
            <input
              ref={findInput}
              data-slot="terminal-find-input"
              className="h-[24px] w-[180px] rounded-[5px] border-0 border-current bg-transparent px-[6px] py-0 text-[12px] text-foreground placeholder:text-muted-foreground focus:outline-2 focus:-outline-offset-2 focus:outline-ring"
              value={query}
              placeholder="Find"
              aria-label="Find in terminal"
              spellCheck={false}
              autoComplete="off"
              onChange={(event) => {
                setQuery(event.target.value);
                find("next", {
                  query: event.target.value,
                  caseSensitive,
                  incremental: true,
                });
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  runFind(event.shiftKey ? "previous" : "next");
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  closeFind();
                }
              }}
            />
            <span
              data-slot="terminal-find-count"
              className="min-w-[52px] pr-[4px] text-right text-[11px] text-muted-foreground tabular-nums"
              aria-live="polite"
            >
              {query && results
                ? results.count
                  ? `${results.index + 1}/${results.count}`
                  : "No results"
                : ""}
            </span>
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label="Previous match"
              onClick={() => runFind("previous")}
            >
              <ChevronUp />
            </Button>
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label="Next match"
              onClick={() => runFind("next")}
            >
              <ChevronDown />
            </Button>
            <Button
              size="icon-xs"
              variant="ghost"
              aria-pressed={caseSensitive}
              aria-label="Match case"
              onClick={() => {
                const next = !caseSensitive;
                setCaseSensitive(next);
                find("next", {
                  query,
                  caseSensitive: next,
                  incremental: false,
                });
              }}
            >
              <CaseSensitive />
            </Button>
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label="Close find"
              onClick={closeFind}
            >
              <X />
            </Button>
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
