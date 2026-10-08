import { StatusDot } from "./components/StatusDot";
import { WorkspaceBanner, WorkspaceEmpty } from "./components/Feedback";
import {
  AppShell,
  AppSidebar,
  SidebarResizer,
  WindowToolbar,
} from "./components/AppShell";
import {
  SidebarNavigation,
  SidebarTab,
  SidebarAction,
} from "./components/SidebarNavigation";
import { EditorBody, EditorFooter } from "./components/EditorLayout";
import {
  WorkspaceViewport,
  WorkspaceDetail,
  WorkspaceContent,
} from "./components/WorkspaceLayout";
import { serverScope } from "./query/keys";
import { WindowControls } from "./components/WindowControls";
import appIcon from "../src-tauri/icons/icon.png";
import { ServerScopeProvider } from "./query/keys";
import { hasOpenDialog } from "./overlays";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { SettingsPanel } from "./components/SettingsPanel";
import {
  useWorkspaceNavigation,
  type WorkspaceView,
} from "./hooks/useWorkspaceNavigation";
import { useDesktopShortcuts } from "./hooks/useDesktopShortcuts";
import { useRetention } from "./hooks/useRetention";
import { useTaskRunner } from "./hooks/useTaskRunner";
import { IntegrationPanel } from "./components/IntegrationPanel";
import { ConnectionsPanel } from "./components/ConnectionsPanel";
import { ProjectsPanel } from "./components/ProjectsPanel";
import { useSnapshot } from "./hooks/useSnapshot";
import { desktop } from "./api/desktop";
import { useSidebar } from "./useSidebar";
import { immersiveStore, setImmersive } from "./immersive";
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
  lazy,
  Suspense,
  type CSSProperties,
} from "react";
import { isTauri } from "@tauri-apps/api/core";
import { useSelector } from "@tanstack/react-store";
import {
  FolderGit2,
  FolderOpen,
  Plug,
  PanelLeft,
  Ellipsis,
  Activity,
  Boxes,
  Cable,
  Layers,
  Terminal,
  CircleAlert,
  Network,
  Plus,
  RefreshCw,
  Server as ServerIcon,
  Settings2,
  X,
  type LucideIcon,
} from "lucide-react";
import {
  displayName,
  newServer,
  type Server,
  type TunnelEditor,
} from "./types";

const FilesPanel = lazy(() =>
  import("./components/FilesPanel").then((module) => ({
    default: module.FilesPanel,
  })),
);

/** The sidebar and the command palette share one ordered view list. */
const viewItems: { value: WorkspaceView; label: string; icon: LucideIcon }[] = [
  { value: "overview", label: "Overview", icon: Activity },
  { value: "connections", label: "Connections", icon: Cable },
  { value: "services", label: "Services", icon: Layers },
  { value: "containers", label: "Containers", icon: Boxes },
  { value: "terminal", label: "Terminal", icon: Terminal },
  { value: "files", label: "Files", icon: FolderOpen },
  { value: "projects", label: "Projects", icon: FolderGit2 },
  { value: "integration", label: "Integration", icon: Plug },
];

type Editor =
  { kind: "server"; value: Server; existing: boolean } | TunnelEditor;

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
  const packagedDesktop =
    location.protocol === "tauri:" && !navigator.platform.startsWith("Win");
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
  const windows = navigator.platform.startsWith("Win");
  const immersive = useSelector(immersiveStore, (state) => state.active);
  // Immersive mode belongs to the terminal page; leaving it restores the shell.
  useEffect(() => {
    if (immersive && (settings || view !== "terminal")) setImmersive(false);
  }, [immersive, settings, view]);
  // Retention fans out through one call; see useRetention for the key
  // spaces. UI choices survive reconnects, native handles reset with them.
  useRetention({
    loaded,
    snapshotError,
    loadError: data.loadError,
    servers: data.config.servers,
    connectionRevisions: data.runtime.connectionRevisions,
    instanceId: data.instanceId,
  });
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

  // App-level desktop actions; failures stay in this channel, separate
  // from snapshot failures (see the banner and the delete dialog below).
  const { act, pending, actionError, clearActionError } =
    useTaskRunner(refresh);
  const [editor, setEditor] = useState<Editor | null>(null);
  const [deleting, setDeleting] = useState<{
    kind: "server" | "tunnel";
    id: string;
    name: string;
  } | null>(null);
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
  const desktopAction = useDesktopShortcuts({
    packagedDesktop,
    dispatch: (action) => {
      if (editor || deleting || commandOpen || hasOpenDialog()) return;
      if (action === "app-settings") openSettings();
      else if (action === "sidebar-toggle") sidebar.toggle();
      else if (action === "server-new" && !blocked) addServer();
      else if (action.startsWith("view-") && server)
        selectView(action.slice(5));
      else if (server && action === "server-edit")
        setEditor({ kind: "server", value: server, existing: true });
      else if (server && action === "server-delete")
        setDeleting({
          kind: "server",
          id: server.id,
          name: displayName(server),
        });
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
    },
  });
  const openServerMenu = () => setServerMenuOpen(true);
  return (
    <AppShell
      value={settings ? "settings" : view}
      onValueChange={selectView}
      orientation="vertical"
      collapsed={sidebar.hidden}
      immersive={immersive}
      style={{ "--source-width": `${sidebar.width}px` } as CSSProperties}
    >
      <AppSidebar
        windows={windows}
        hidden={sidebar.hidden || immersive}
        onContextMenu={(event) => {
          event.preventDefault();
          openServerMenu();
        }}
      >
        <div
          data-slot="sidebar-titlebar"
          className={
            windows ? "hidden" : "-mx-[10px] h-[52px] min-h-[52px] select-none"
          }
          data-tauri-drag-region
        />
        <div
          data-slot="app-brand"
          className={`${windows ? "h-[52px]" : "h-[30px]"} mb-[6px] flex shrink-0 select-none items-center gap-[8px] px-[9px] text-[14px] font-semibold text-foreground [&_img]:pointer-events-none [&_img]:rounded-[5px]`}
          data-tauri-drag-region
        >
          <img src={appIcon} alt="" width={22} height={22} draggable={false} />
          <span data-tauri-drag-region>Newport</span>
        </div>
        <ServerPicker
          servers={data.config.servers}
          selected={selected}
          loaded={loaded}
          blocked={blocked}
          onSelect={(id) => {
            selectServer(id);
            clearActionError();
          }}
          onAdd={addServer}
        />

        <SidebarNavigation aria-label="Server views">
          {viewItems.map(({ value, label, icon: Icon }) => (
            <SidebarTab
              key={value}
              value={value}
              aria-label={label}
              title={!server ? "Add a server to open this view" : undefined}
              disabled={!server}
            >
              <Icon size={17} />
              <span>{label}</span>
              {value === "connections" && tunnels.length > 0 && (
                <span
                  data-slot="source-count"
                  className="ml-auto text-[11px] text-muted-foreground tabular-nums group-data-[state=active]/navigation:text-(--native-selection-foreground)"
                >
                  {tunnels.length}
                </span>
              )}
            </SidebarTab>
          ))}
        </SidebarNavigation>
        <div
          data-slot="sidebar-secondary"
          className="mt-auto flex flex-col gap-[3px] border-0 border-current pt-[16px] pb-[8px] [@media(max-height:560px)]:pt-[4px]"
        >
          <SidebarAction
            variant="ghost"
            className="aria-[current=page]:bg-(--native-selection)"
            onClick={openSettings}
            aria-current={settings ? "page" : undefined}
          >
            <Settings2 size={16} /> Settings
          </SidebarAction>
          <SidebarAction
            variant="ghost"
            onClick={addServer}
            disabled={blocked}
            aria-label="Add server"
          >
            <Plus size={16} />
            Add Server
          </SidebarAction>
        </div>
        <div
          data-slot="sidebar-footer"
          className="m-0 grid grid-cols-[6px_1fr] items-center gap-x-[8px] gap-y-[5px] border-t border-border px-[8px] pt-[10px] pb-[14px] text-[11px]"
          role="status"
        >
          <StatusDot className={connected ? "bg-(--green)" : undefined} />
          <span>
            {connected
              ? `${connected} active tunnel${connected === 1 ? "" : "s"}`
              : "No active tunnels"}
          </span>
        </div>
      </AppSidebar>
      {!sidebar.hidden && !immersive && (
        <SidebarResizer
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
      <main className="flex min-h-0 min-w-0 flex-col overflow-hidden bg-background">
        {(!immersive || windows) && (
          <WindowToolbar
            windows={windows}
            collapsed={sidebar.hidden}
            immersive={immersive}
            data-tauri-drag-region
          >
            {!immersive && (
              <>
                <Button
                  variant="ghost"
                  size="icon"
                  data-slot="sidebar-toggle"
                  className="size-[28px] rounded-[5px]"
                  data-workspace-focus
                  aria-label={sidebar.hidden ? "Show sidebar" : "Hide sidebar"}
                  title={sidebar.hidden ? "Show sidebar" : "Hide sidebar"}
                  onClick={sidebar.toggle}
                >
                  <PanelLeft size={18} />
                </Button>
                <div
                  data-slot="toolbar-title"
                  className="min-w-0 flex-1 [&_*]:pointer-events-none"
                  data-tauri-drag-region
                >
                  <div
                    data-slot="toolbar-heading"
                    className="flex flex-wrap items-center gap-x-[10px] gap-y-[5px]"
                  >
                    <h1 className="text-[13px] leading-[1.3] font-semibold tracking-normal">
                      {settings
                        ? "Settings"
                        : server
                          ? view[0].toUpperCase() + view.slice(1)
                          : "Servers"}
                    </h1>
                    {server && !settings && (
                      <span
                        title={
                          data.runtime.connectivity?.[server.id]?.error ??
                          undefined
                        }
                        data-slot="toolbar-health"
                        className={`inline-flex items-center gap-[6px] text-[10px] whitespace-nowrap ${health === "reachable" ? "text-(--green)" : health === "unreachable" || health === "error" ? "text-(--red)" : "text-muted-foreground"}`}
                      >
                        <span className="inline-block size-[5px] shrink-0 rounded-full bg-current" />
                        {health === "unknown"
                          ? "Connection unknown"
                          : health === "error"
                            ? "Connection failed"
                            : health[0].toUpperCase() + health.slice(1)}
                      </span>
                    )}
                  </div>

                  <span className="mt-[2px] block truncate text-[11px] text-muted-foreground max-[700px]:max-w-[210px]">
                    {settings
                      ? "Application preferences"
                      : server
                        ? `${displayName(server)} · ${server.sshUser}@${server.sshHost}`
                        : "Your SSH workspace"}
                  </span>
                </div>
                {server && !settings && (
                  <div
                    data-slot="toolbar-actions"
                    className="flex shrink-0 items-center gap-[2px] text-muted-foreground [&_button]:size-[28px] [&_button]:rounded-[5px]"
                  >
                    <Button
                      variant="ghost"
                      size="icon"

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
                    <span
                      data-slot="toolbar-divider"
                      className="mx-[5px] h-[16px] w-px bg-border"
                    />
                    <Button
                      variant="ghost"
                      size="icon"

                      title="Edit server"
                      aria-label="Edit server"
                      onClick={() =>
                        setEditor({
                          kind: "server",
                          value: server,
                          existing: true,
                        })
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
                          onSelect={() =>
                            desktopAction.current("server-delete")
                          }
                        >
                          Delete server
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                )}
              </>
            )}
            <WindowControls />
          </WindowToolbar>
        )}
        <WorkspaceViewport mode={settings ? undefined : view} key={selected}>
          {!live && (
            <WorkspaceBanner>
              Interface preview. Open the desktop app to manage your servers.
            </WorkspaceBanner>
          )}
          {data.loadError && (
            <WorkspaceBanner error role="alert">
              Profiles could not be loaded. Your original files are unchanged.{" "}
              {data.loadError}
            </WorkspaceBanner>
          )}
          {actionError && (
            <WorkspaceBanner error role="alert">
              <CircleAlert size={16} />
              <span>{actionError}</span>
              <Button
                variant="ghost"
                size="icon"

                aria-label="Dismiss error"
                onClick={() => {
                  clearActionError();
                }}
              >
                <X size={16} />
              </Button>
            </WorkspaceBanner>
          )}
          {snapshotError && (
            <WorkspaceBanner error role="alert">
              <CircleAlert size={16} />
              <span>{snapshotError}</span>
              <Button
                variant="ghost"
                size="icon"

                aria-label="Dismiss error"
                onClick={() => {
                  dismissSnapshotError();
                }}
              >
                <X size={16} />
              </Button>
            </WorkspaceBanner>
          )}
          <ErrorBoundary
            key={`${selected}:${settings ? "settings" : view}`}
            scope="workspace"
          >
            <Suspense
              fallback={
                <p
                  data-slot="workspace-loading"
                  className="p-[24px] text-muted-foreground"
                  role="status"
                >
                  Loading workspace…
                </p>
              }
            >
              {settings ? (
                <WorkspaceDetail mode={settings ? undefined : view}>
                  <SettingsPanel />
                </WorkspaceDetail>
              ) : !loaded ? (
                <WorkspaceEmpty>
                  <RefreshCw size={26} />
                  <h1>Loading your servers…</h1>
                </WorkspaceEmpty>
              ) : !server ? (
                <WorkspaceEmpty>
                  <Network size={42} strokeWidth={1.25} />
                  <h1>Your servers, one hop away.</h1>
                  <p>
                    Connect a server to manage its services, files, and SSH
                    tunnels.
                  </p>
                  <Button
                    variant="default"
                    onClick={addServer}
                    disabled={blocked}
                  >
                    <Plus size={16} />
                    Add your first server
                  </Button>
                </WorkspaceEmpty>
              ) : (
                <WorkspaceDetail mode={settings ? undefined : view}>
                  <ServerScopeProvider
                    server={server}
                    instanceId={data.instanceId}
                    revision={
                      data.runtime.connectionRevisions?.[server.id] ?? 0
                    }
                  >
                    <WorkspaceContent value={view} mode={view}>
                      {view === "projects" ? (
                        <ProjectsPanel
                          key={
                            serverScope(
                              server,
                              data.runtime.connectionRevisions?.[server.id] ??
                                0,
                              data.instanceId,
                            ).connection
                          }
                          server={server}
                        />
                      ) : view === "files" ? (
                        <FilesPanel
                          key={
                            serverScope(
                              server,
                              data.runtime.connectionRevisions?.[server.id] ??
                                0,
                              data.instanceId,
                            ).connection
                          }
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
                          key={
                            serverScope(
                              server,
                              data.runtime.connectionRevisions?.[server.id] ??
                                0,
                              data.instanceId,
                            ).connection
                          }
                          server={server}
                          tab={view}
                        />
                      ) : (
                        <ConnectionsPanel
                          server={server}
                          tunnels={tunnels}
                          runtime={data.runtime}
                          setEditor={setEditor}
                          setDeleting={setDeleting}
                        />
                      )}
                    </WorkspaceContent>
                  </ServerScopeProvider>
                </WorkspaceDetail>
              )}
            </Suspense>
          </ErrorBoundary>
        </WorkspaceViewport>
      </main>
      <CommandDialog
        open={commandOpen}
        onOpenChange={setCommandOpen}
        title="Go to"
        description="Find a server or workspace action."
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
              {viewItems.map(({ value, icon: Icon }, index) => (
                <CommandItem
                  key={value}
                  disabled={!server}
                  onSelect={() => {
                    selectView(value);
                    setCommandOpen(false);
                  }}
                >
                  <Icon />
                  <span>Open {value}</span>
                  <CommandShortcut>
                    {windows ? `Ctrl+${index + 1}` : `⌘${index + 1}`}
                  </CommandShortcut>
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
                "Choose Terminal to open a shell, or Connections to forward a port.",
            });
          }}
        />
      )}
      {editor?.kind === "tunnel" && (
        <TunnelForm
          value={editor.value}
          existing={editor.existing}
          projects={editor.projects}
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
          <EditorBody>
            {actionError && (
              <p role="alert" className="text-(--red)">
                {actionError}
              </p>
            )}
            <p id="delete-description">
              Delete <strong>{deleting.name}</strong>?{" "}
              {deleting.kind === "server"
                ? "Its tunnels will disconnect and be removed, along with its saved password."
                : "The tunnel will disconnect and its configuration will be removed."}
            </p>
          </EditorBody>
          <EditorFooter>
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
          </EditorFooter>
        </Modal>
      )}
    </AppShell>
  );
}
