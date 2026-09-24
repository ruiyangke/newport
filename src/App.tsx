import { WindowControls } from "./components/WindowControls";
import appIcon from "../src-tauri/icons/icon.png";
import { isCancelledError } from "@tanstack/react-query";
import { ServerScopeProvider } from "./query/keys";
import { hasOpenDialog } from "./overlays";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { SettingsPanel } from "./components/SettingsPanel";
import {
  useWorkspaceNavigation,
  workspaceViews,
} from "./hooks/useWorkspaceNavigation";
import { IntegrationPanel } from "./components/IntegrationPanel";
import { ConnectionsPanel } from "./components/ConnectionsPanel";
import { useSnapshot } from "./hooks/useSnapshot";
import { desktop } from "./api/desktop";
import { useSidebar } from "./useSidebar";
import { listen } from "@tauri-apps/api/event";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "./components/ui/dropdown-menu";
import { toast } from "sonner";
import { ServerPicker } from "./components/ServerPicker";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "./components/ui/tabs";
import {
  Command,
  CommandDialog,
  CommandInput,
  CommandList,
  CommandEmpty,
  CommandGroup,
  CommandItem,
  CommandSeparator,
  CommandShortcut,
} from "./components/ui/command";
import { Button } from "./components/controls";
import { Cockpit } from "./components/Cockpit";
import { Modal, ServerForm, TunnelForm } from "./components/Editors";
import {
  useEffect,
  useState,
  useRef,
  lazy,
  Suspense,
  type CSSProperties,
} from "react";
import { isTauri } from "@tauri-apps/api/core";
import {
  FolderOpen,
  Plug,
  PanelLeft,
  Ellipsis,
  Activity,
  Boxes,
  Cable,
  Command as CommandIcon,
  Layers,
  Terminal,
  CircleAlert,
  Network,
  Plus,
  RefreshCw,
  Server as ServerIcon,
  Settings2,
  X,
} from "lucide-react";
import {
  displayName,
  serverConnectionKey,
  newServer,
  type Server,
  type Tunnel,
} from "./types";

const FilesPanel = lazy(() =>
  import("./components/FilesPanel").then((module) => ({
    default: module.FilesPanel,
  })),
);

type Editor =
  | { kind: "server"; value: Server; existing: boolean }
  | { kind: "tunnel"; value: Tunnel; existing: boolean };

export default function App() {
  const sidebar = useSidebar();
  useEffect(() => {
    if (!isTauri()) return;
    const stop = listen<string>("preference-error", (event) =>
      toast.error(event.payload),
    ).catch(() => () => {});
    return () => {
      void stop.then((unlisten) => unlisten()).catch(() => {});
    };
  }, []);
  const [serverMenuOpen, setServerMenuOpen] = useState(false);
  const desktopAction = useRef<(action: string) => void>(() => {});
  const packagedDesktop =
    location.protocol === "tauri:" && !navigator.platform.startsWith("Win");
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    if (isTauri())
      void listen<string>("workspace-action", (event) =>
        desktopAction.current(event.payload),
      )
        .then((stop) => {
          if (disposed) stop();
          else unlisten = stop;
        })
        .catch(() => {});
    const keydown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || packagedDesktop) return;
      const action =
        event.key === ","
          ? "app-settings"
          : event.key === "n"
            ? "server-new"
            : event.shiftKey && event.key.toLowerCase() === "e"
              ? "server-edit"
              : event.shiftKey && event.key.toLowerCase() === "t"
                ? "server-test"
                : event.shiftKey && event.key.toLowerCase() === "l"
                  ? "sidebar-toggle"
                  : /^[1-6]$/.test(event.key)
                    ? `view-${workspaceViews[Number(event.key) - 1]}`
                    : "";
      if (action) {
        event.preventDefault();
        desktopAction.current(action);
      }
    };
    window.addEventListener("keydown", keydown);
    return () => {
      disposed = true;
      unlisten?.();
      window.removeEventListener("keydown", keydown);
    };
  }, [packagedDesktop]);
  const live = isTauri();
  const {
    data,
    loaded,
    error: snapshotError,
    refresh,
    dismissError: dismissSnapshotError,
  } = useSnapshot(live);
  const { selected, view, settings, openSettings, selectServer, selectView } =
    useWorkspaceNavigation(
      data.config.servers,
      loaded && !snapshotError && !data.loadError,
    );
  const [commandOpen, setCommandOpen] = useState(false);
  useEffect(() => {
    const listener = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setCommandOpen((open) => (open ? false : !hasOpenDialog()));
      }
    };
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, []);

  const [actionError, setError] = useState("");
  const error = actionError || snapshotError;
  const [editor, setEditor] = useState<Editor | null>(null);
  const inFlight = useRef(new Set<string>());
  const [pending, setPending] = useState<Set<string>>(new Set());
  const [deleting, setDeleting] = useState<{
    kind: "server" | "tunnel";
    id: string;
    name: string;
  } | null>(null);
  const act = async (
    key: string,
    task: () => Promise<unknown>,
    message?: string,
  ) => {
    if (inFlight.current.has(key)) return;
    inFlight.current.add(key);
    setPending((p) => new Set(p).add(key));
    setError("");
    try {
      try {
        await task();
      } finally {
        await refresh();
      }
      if (message)
        toast.success(message, { id: "ssh-verified", duration: 3500 });
    } catch (e) {
      if (!isCancelledError(e)) setError(String(e));
    } finally {
      inFlight.current.delete(key);
      setPending((p) => {
        const next = new Set(p);
        next.delete(key);
        return next;
      });
    }
  };
  const server = data.config.servers.find((s) => s.id === selected);
  const tunnels = data.config.tunnels.filter((t) => t.serverId === selected);
  const connected = Object.values(data.runtime.tunnels).filter(
    (t) => t.status === "connected",
  ).length;
  const blocked = !live || !!data.loadError || !loaded;
  const addServer = () =>
    setEditor({ kind: "server", value: newServer(), existing: false });
  const health = server
    ? (data.runtime.health[server.id] ?? "unknown")
    : "unknown";
  desktopAction.current = (action) => {
    if (editor || deleting || commandOpen || hasOpenDialog()) return;
    if (action === "app-settings") openSettings();
    else if (action === "sidebar-toggle") sidebar.toggle();
    else if (action === "server-new" && !blocked) addServer();
    else if (action.startsWith("view-") && server) selectView(action.slice(5));
    else if (server && action === "server-edit")
      setEditor({ kind: "server", value: server, existing: true });
    else if (server && action === "server-delete")
      setDeleting({ kind: "server", id: server.id, name: displayName(server) });
    else if (
      server &&
      action === "server-test" &&
      !pending.has(`test-${selected}`)
    )
      void act(
        `test-${selected}`,
        () => desktop("test_connection", { id: selected }),
        "SSH connection verified.",
      );
  };
  const openServerMenu = () => setServerMenuOpen(true);
  return (
    <Tabs
      value={settings ? "settings" : view}
      onValueChange={selectView}
      orientation="vertical"
      className={`app-shell native-shell mac-shell ${navigator.platform.startsWith("Win") ? "windows-shell" : ""} ${sidebar.hidden ? "sidebar-hidden" : ""}`}
      style={{ "--source-width": `${sidebar.width}px` } as CSSProperties}
    >
      <aside
        className="sidebar"
        hidden={sidebar.hidden}
        onContextMenu={(event) => {
          event.preventDefault();
          openServerMenu();
        }}
      >
        <div className="sidebar-titlebar" data-tauri-drag-region />
        <div className="app-brand" data-tauri-drag-region>
          <img src={appIcon} alt="" width={22} height={22} draggable={false} />
          <span data-tauri-drag-region>Porthop</span>
        </div>
        <ServerPicker
          servers={data.config.servers}
          selected={selected}
          loaded={loaded}
          blocked={blocked}
          onSelect={(id) => {
            selectServer(id);
            setError("");
          }}
          onAdd={addServer}
        />

        <TabsList className="source-list" aria-label="Server views">
          {[
            { value: "overview", label: "Overview", icon: Activity },
            { value: "connections", label: "Connections", icon: Cable },
            { value: "services", label: "Services", icon: Layers },
            { value: "containers", label: "Containers", icon: Boxes },
            { value: "commands", label: "Commands", icon: Terminal },
            { value: "files", label: "Files", icon: FolderOpen },
            { value: "integration", label: "Integration", icon: Plug },
          ].map(({ value, label, icon: Icon }) => (
            <TabsTrigger
              key={value}
              value={value}
              aria-label={label}
              disabled={!server}
            >
              <Icon size={17} />
              <span>{label}</span>
              {value === "connections" && tunnels.length > 0 && (
                <span className="source-count">{tunnels.length}</span>
              )}
            </TabsTrigger>
          ))}
        </TabsList>
        <div className="sidebar-secondary">
          <Button
            variant="ghost"
            onClick={openSettings}
            aria-current={settings ? "page" : undefined}
          >
            <Settings2 size={16} /> Settings
          </Button>
          <Button
            variant="ghost"
            onClick={addServer}
            disabled={blocked}
            aria-label="Add server"
          >
            <Plus size={16} />
            Add Server
          </Button>
        </div>
        <div className="sidebar-footer" role="status">
          <span
            aria-hidden="true"
            className={`status-dot ${connected ? "online" : ""}`}
          />
          <span>
            {connected
              ? `${connected} active tunnel${connected === 1 ? "" : "s"}`
              : "No active tunnels"}
          </span>
        </div>
      </aside>
      {!sidebar.hidden && (
        <div
          className="sidebar-resizer"
          role="separator"
          aria-label="Sidebar width"
          aria-orientation="vertical"
          aria-valuemin={176}
          aria-valuemax={sidebar.maximum}
          aria-valuenow={sidebar.width}
          tabIndex={0}
          onPointerDown={(event) => {
            if (event.button !== 0) return;
            event.preventDefault();
            event.currentTarget.setPointerCapture(event.pointerId);
          }}
          onPointerMove={(event) => {
            if (event.currentTarget.hasPointerCapture(event.pointerId))
              sidebar.resize(event.clientX);
          }}
          onPointerUp={(event) => {
            event.currentTarget.releasePointerCapture(event.pointerId);
          }}
          onDoubleClick={() => sidebar.resize(204)}
          onKeyDown={(event) => {
            if (
              ["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)
            ) {
              event.preventDefault();
              sidebar.resize(
                event.key === "Home"
                  ? 176
                  : event.key === "End"
                    ? sidebar.maximum
                    : sidebar.width + (event.key === "ArrowLeft" ? -10 : 10),
              );
            }
          }}
        />
      )}
      <main>
        <header className="window-toolbar" data-tauri-drag-region>
          <Button
            variant="ghost"
            size="icon"
            className="icon sidebar-toggle"
            data-workspace-focus
            aria-label={sidebar.hidden ? "Show sidebar" : "Hide sidebar"}
            title={sidebar.hidden ? "Show sidebar" : "Hide sidebar"}
            onClick={sidebar.toggle}
          >
            <PanelLeft size={18} />
          </Button>
          <div className="toolbar-title" data-tauri-drag-region>
            <div className="toolbar-heading">
              <h1>
                {settings
                  ? "Settings"
                  : server
                    ? view[0].toUpperCase() + view.slice(1)
                    : "Servers"}
              </h1>
              {server && !settings && (
                <span
                  title={
                    data.runtime.connectivity?.[server.id]?.error ?? undefined
                  }
                  className={`status toolbar-health ${health === "reachable" ? "connected" : health === "unreachable" || health === "error" ? "error" : ""}`}
                >
                  <span className="status-dot" />
                  {health === "unknown"
                    ? "Connection unknown"
                    : health === "error"
                      ? "Connection failed"
                      : health[0].toUpperCase() + health.slice(1)}
                </span>
              )}
            </div>

            <span>
              {settings
                ? "Porthop"
                : server
                  ? `${displayName(server)} · ${server.sshUser}@${server.sshHost}`
                  : "Your SSH workspace"}
            </span>
          </div>
          {server && !settings && (
            <div className="toolbar-actions">
              <Button
                variant="ghost"
                size="icon"
                className="icon"
                title="Test connection"
                aria-label="Test connection"
                loading={pending.has(`test-${selected}`)}
                onClick={() =>
                  void act(
                    `test-${selected}`,
                    () => desktop("test_connection", { id: selected }),
                    "SSH connection verified.",
                  )
                }
              >
                <Cable size={16} />
              </Button>
              <span className="toolbar-divider" />
              <Button
                variant="ghost"
                size="icon"
                className="icon"
                title="Edit server"
                aria-label="Edit server"
                onClick={() =>
                  setEditor({ kind: "server", value: server, existing: true })
                }
              >
                <Settings2 size={17} />
              </Button>
              <DropdownMenu
                open={serverMenuOpen}
                onOpenChange={setServerMenuOpen}
              >
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="icon"
                    aria-label="Server actions"
                  >
                    <Ellipsis size={18} />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem
                    onSelect={() => desktopAction.current("server-test")}
                  >
                    Test connection
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    onSelect={() => desktopAction.current("server-edit")}
                  >
                    Edit server…
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem
                    onSelect={() => desktopAction.current("server-new")}
                  >
                    Add server…
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    variant="destructive"
                    onSelect={() => desktopAction.current("server-delete")}
                  >
                    Delete server
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          )}
          <WindowControls />
        </header>
        <div
          className={`workspace-scroll${!settings && view === "commands" ? " terminal-workspace" : !settings && view === "files" ? " files-pane" : ""}`}
          key={selected}
        >
          {!live && (
            <div className="banner">
              Interface preview. Open the desktop app to manage your servers.
            </div>
          )}
          {data.loadError && (
            <div className="banner error" role="alert">
              Profiles could not be loaded. Your original files are unchanged.{" "}
              {data.loadError}
            </div>
          )}
          {error && (
            <div className="banner error" role="alert">
              <CircleAlert size={16} />
              <span>{error}</span>
              <Button
                variant="ghost"
                size="icon"
                className="icon"
                aria-label="Dismiss error"
                onClick={() => {
                  setError("");
                  dismissSnapshotError();
                }}
              >
                <X size={16} />
              </Button>
            </div>
          )}
          <ErrorBoundary
            key={`${selected}:${settings ? "settings" : view}`}
            scope="workspace"
          >
            <Suspense
              fallback={
                <p className="workspace-loading" role="status">
                  Loading workspace…
                </p>
              }
            >
              {settings ? (
                <div className="detail">
                  <SettingsPanel />
                </div>
              ) : !loaded ? (
                <div className="empty">
                  <RefreshCw size={26} />
                  <h1>Loading your servers…</h1>
                </div>
              ) : !server ? (
                <div className="empty">
                  <Network size={42} strokeWidth={1.25} />
                  <h1>Your servers, one hop away.</h1>
                  <p>
                    Connect a server to manage its services, files, and SSH
                    tunnels.
                  </p>
                  <Button
                    variant="default"
                    className="primary"
                    onClick={addServer}
                    disabled={blocked}
                  >
                    <Plus size={16} />
                    Add your first server
                  </Button>
                </div>
              ) : (
                <div className="detail">
                  <ServerScopeProvider
                    server={server}
                    revision={
                      data.runtime.connectionRevisions?.[server.id] ?? 0
                    }
                  >
                    <TabsContent value={view} className="workspace-content">
                      {view === "files" ? (
                        <FilesPanel
                          key={`${serverConnectionKey(server)}:${data.runtime.connectionRevisions?.[server.id] ?? 0}`}
                          server={server}
                        />
                      ) : view === "integration" ? (
                        <IntegrationPanel
                          server={server}
                          runtime={data.runtime}
                          pending={pending}
                          act={act}
                        />
                      ) : view !== "connections" ? (
                        <Cockpit
                          key={`${serverConnectionKey(server)}:${data.runtime.connectionRevisions?.[server.id] ?? 0}`}
                          server={server}
                          tab={view}
                        />
                      ) : (
                        <>
                          <ConnectionsPanel
                            server={server}
                            tunnels={tunnels}
                            runtime={data.runtime}
                            pending={pending}
                            act={act}
                            setEditor={setEditor}
                            setDeleting={setDeleting}
                          />
                        </>
                      )}
                    </TabsContent>
                  </ServerScopeProvider>
                </div>
              )}
            </Suspense>
          </ErrorBoundary>
        </div>
      </main>
      <CommandDialog
        open={commandOpen}
        onOpenChange={setCommandOpen}
        title="Go to"
        description="Find a server or workspace action."
        className="workspace-command"
      >
        <Command>
          <CommandInput placeholder="Search servers and actions…" />
          <CommandList>
            <CommandEmpty>No matching servers or actions.</CommandEmpty>
            <CommandGroup heading="Servers">
              {data.config.servers.map((entry) => (
                <CommandItem
                  key={entry.id}
                  value={`server:${entry.id}`}
                  keywords={[
                    displayName(entry),
                    entry.sshHost,
                    entry.sshUser,
                    String(entry.sshPort),
                  ]}
                  onSelect={() => {
                    selectServer(entry.id);
                    setCommandOpen(false);
                  }}
                >
                  <ServerIcon />
                  <span>{displayName(entry)}</span>
                  <CommandShortcut>
                    {entry.sshUser}@{entry.sshHost}:{entry.sshPort}
                  </CommandShortcut>
                </CommandItem>
              ))}
            </CommandGroup>
            <CommandSeparator />
            <CommandGroup heading="Workspace">
              <CommandItem
                onSelect={() => {
                  openSettings();
                  setCommandOpen(false);
                }}
              >
                <Settings2 /> Settings
              </CommandItem>
              {workspaceViews.map((tab) => (
                <CommandItem
                  key={tab}
                  disabled={!server}
                  onSelect={() => {
                    selectView(tab);
                    setCommandOpen(false);
                  }}
                >
                  <CommandIcon />
                  <span>Open {tab}</span>
                </CommandItem>
              ))}
              <CommandItem
                disabled={blocked}
                onSelect={() => {
                  setCommandOpen(false);
                  addServer();
                }}
              >
                <Plus />
                Add server
              </CommandItem>
            </CommandGroup>
          </CommandList>
        </Command>
      </CommandDialog>
      {editor?.kind === "server" && (
        <ServerForm
          value={editor.value}
          existing={editor.existing}
          onClose={() => setEditor(null)}
          onSave={async (s, password) => {
            try {
              await desktop("save_server", { server: s, password });
            } finally {
              await refresh();
            }
            selectServer(s.id);
            setEditor(null);
            toast.success("Server saved", {
              description:
                "Choose Commands to open a shell, or Connections to forward a port.",
            });
          }}
        />
      )}
      {editor?.kind === "tunnel" && (
        <TunnelForm
          value={editor.value}
          existing={editor.existing}
          onClose={() => setEditor(null)}
          onSave={async (t) => {
            try {
              await desktop("save_tunnel", { tunnel: t });
            } finally {
              await refresh();
            }
            setEditor(null);
          }}
        />
      )}
      {deleting && (
        <Modal
          title={`Delete ${deleting.kind}?`}
          busy={pending.has("delete")}
          describedBy="delete-description"
          onClose={() => setDeleting(null)}
        >
          <div className="form-body">
            {error && (
              <p role="alert" className="error">
                {error}
              </p>
            )}
            <p id="delete-description">
              Delete <strong>{deleting.name}</strong>?{" "}
              {deleting.kind === "server"
                ? "Its tunnels will disconnect and be removed, along with its saved password."
                : "The tunnel will disconnect and its configuration will be removed."}
            </p>
          </div>
          <footer className="dialog-footer">
            <Button
              disabled={pending.has("delete")}
              onClick={() => setDeleting(null)}
            >
              Cancel
            </Button>
            <Button
              variant="destructive"
              loading={pending.has("delete")}
              onClick={() =>
                void act("delete", async () => {
                  await desktop(`delete_${deleting.kind}`, { id: deleting.id });
                  setDeleting(null);
                })
              }
            >
              {pending.has("delete") ? "Deleting…" : `Delete ${deleting.kind}`}
            </Button>
          </footer>
        </Modal>
      )}
    </Tabs>
  );
}
