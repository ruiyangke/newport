import { useEffect } from "react";
import { useLocation, useMatch, useNavigate } from "react-router";

export const workspaceViews = [
  "overview",
  "connections",
  "services",
  "containers",
  "commands",
  "files",
  "integration",
] as const;
export type WorkspaceView = (typeof workspaceViews)[number];

function isWorkspaceView(value: string | undefined): value is WorkspaceView {
  return workspaceViews.some((view) => view === value);
}

function workspacePath(serverId: string, view: WorkspaceView) {
  return `/servers/${encodeURIComponent(serverId)}/${view}`;
}

export function useWorkspaceNavigation(
  servers: readonly { id: string }[],
  ready: boolean,
) {
  const match = useMatch("/servers/:serverId/:view?");
  const location = useLocation();
  const navigate = useNavigate();
  const settings = location.pathname === "/settings";
  const requestedServer =
    match?.params.serverId ?? (settings ? location.state?.serverId : undefined);
  const requestedView =
    match?.params.view ?? (settings ? location.state?.view : undefined);
  const view = isWorkspaceView(requestedView) ? requestedView : "overview";
  const selected = servers.some((server) => server.id === requestedServer)
    ? requestedServer!
    : (servers[0]?.id ?? "");
  const canonicalPath = settings
    ? "/settings"
    : selected
      ? workspacePath(selected, view)
      : "/";

  // Wait for profiles before repairing missing/deleted servers. Replace invalid
  // entries so Back cannot get stuck bouncing through a stale destination.
  useEffect(() => {
    if (ready && location.pathname !== canonicalPath) {
      void navigate(canonicalPath, { replace: true });
    }
  }, [ready, location.pathname, canonicalPath, navigate]);

  const open = (serverId: string, nextView: WorkspaceView) => {
    const path = workspacePath(serverId, nextView);
    if (path !== location.pathname) void navigate(path);
  };

  return {
    selected,
    view,
    settings,
    openSettings: () => {
      if (!settings)
        void navigate("/settings", { state: { serverId: selected, view } });
    },
    selectServer: (id: string) => open(id, view),
    selectView: (nextView: string) => {
      if (selected && isWorkspaceView(nextView)) open(selected, nextView);
    },
  };
}
