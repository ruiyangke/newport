import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import {
  ArrowLeft,
  Container as ContainerIcon,
  TerminalSquare,
} from "lucide-react";
import { PageHeader } from "../PageHeader";
import { SearchField } from "../SearchField";
import { Button, RefreshButton } from "../controls";
import { MasterListSkeleton } from "../master/MasterList";
import { DetailSkeleton, masterGrid } from "../master/Detail";
import { ContainerList } from "./ContainerList";
import { ContainerDetail } from "./ContainerDetail";
import { ProjectDetail } from "./ProjectDetail";
import { DiskUsage } from "./DiskUsage";
import { recordStats } from "./parts";
import { useComposeWatcher } from "./compose";
import { useContainersSnapshot } from "../../hooks/useContainers";
import { useWorkspaceState } from "../../state/workspace";
import { queueTerminalCommand } from "../../terminal/pending";
import { cn } from "../../lib/utils";
import {
  DISK,
  containerKey,
  groupContainers,
  isProblem,
  isRunning,
  projectKey,
  type Container,
} from "../../domain/containers";

function matches(needle: string, c: Container) {
  return `${c.name} ${c.image} ${c.compose?.project ?? ""} ${c.compose?.service ?? ""} ${c.state} ${c.health ?? ""}`
    .toLowerCase()
    .includes(needle);
}

/** Why the page is empty, and what would fix it. */
function Unavailable({
  id,
  error,
  retry,
  retrying,
}: {
  id: string;
  error: string;
  retry: () => void;
  retrying: boolean;
}) {
  const navigate = useNavigate();
  const denied = error.startsWith("This account can't use Docker");
  const missing = error.startsWith("Docker isn't installed");
  return (
    <div
      role="alert"
      className="mb-[22px] flex flex-1 animate-in flex-col items-center justify-center gap-[10px] text-center duration-200 fade-in motion-reduce:animate-none"
    >
      <ContainerIcon
        size={30}
        strokeWidth={1.4}
        aria-hidden="true"
        className="text-muted-foreground"
      />
      <h3 className="text-[14px]">
        {denied
          ? "This account can’t use Docker"
          : missing
            ? "No Docker on this server"
            : "Docker isn’t answering"}
      </h3>
      <p className="max-w-[460px] text-[12px] leading-[1.6] text-muted-foreground wrap-anywhere">
        {denied
          ? "Docker’s socket only lets root and the docker group in. Adding this account to the group gives it Docker access (which is root-equivalent); it takes effect at the next login."
          : error}
      </p>
      <div className="flex flex-wrap justify-center gap-[8px]">
        {denied && (
          <Button
            onClick={() => {
              queueTerminalCommand(id, "sudo usermod -aG docker $USER");
              void navigate(`/servers/${encodeURIComponent(id)}/terminal`);
            }}
          >
            <TerminalSquare size={13} /> Add me to the docker group…
          </Button>
        )}
        <Button loading={retrying} onClick={retry}>
          Try again
        </Button>
      </div>
    </div>
  );
}

/**
 * Containers, end to end: problems, Compose projects and standalone
 * containers on the left; on the right the chosen container's state, why
 * it is down, its controls, live readings, ports and logs -- or a project
 * as a whole, or Docker's disk usage. The page fits the window and each
 * side scrolls on its own.
 */
export function ContainersPage({ id }: { id: string }) {
  const snapshot = useContainersSnapshot(id);
  const [query, setQuery] = useWorkspaceState(id, "containers.query");
  const [selected, setSelected] = useWorkspaceState(id, "containers.selected");
  const [collapsed, setCollapsed] = useWorkspaceState(
    id,
    "containers.collapsed",
  );
  const [refreshing, setRefreshing] = useState(false);
  const refresh = () => {
    setRefreshing(true);
    void snapshot.refetch().finally(() => setRefreshing(false));
  };
  useComposeWatcher(id);
  const data = snapshot.data;
  const now = snapshot.dataUpdatedAt || Date.now();
  useEffect(
    () => recordStats(id, data, snapshot.dataUpdatedAt),
    [id, data, snapshot.dataUpdatedAt],
  );
  const all = data?.containers ?? [];
  const needle = query.trim().toLowerCase();
  const groups = groupContainers(
    needle ? all.filter((c) => matches(needle, c)) : all,
  );
  // The first problem, else the first project, else the first container.
  const fallback = groups.problems[0]
    ? containerKey(groups.problems[0])
    : groups.projects[0]
      ? projectKey(groups.projects[0].name)
      : groups.standalone[0]
        ? containerKey(groups.standalone[0])
        : DISK;
  const key = selected ?? fallback;
  const container = key.startsWith("c:")
    ? all.find((c) => containerKey(c) === key)
    : undefined;
  const project = key.startsWith("p:")
    ? groupContainers(all).projects.find((p) => projectKey(p.name) === key)
    : undefined;
  const services = all.filter((c) => !c.compose?.oneoff);
  const running = services.filter(isRunning).length;
  const problems = all.filter(isProblem).length;
  const summary = data
    ? [
        `${running} running`,
        services.length - running && `${services.length - running} stopped`,
        problems &&
          `${problems} ${problems === 1 ? "needs" : "need"} attention`,
      ]
        .filter(Boolean)
        .join(" · ")
    : snapshot.error
      ? "Containers unavailable"
      : "Reading containers…";
  const engine = data?.engine;
  return (
    <div
      data-slot="containers-page"
      className="@container/page flex min-h-0 flex-1 flex-col px-[22px] pt-[16px] max-[700px]:px-[16px] max-[700px]:pt-[14px]"
    >
      <PageHeader
        className="mb-[14px]"
        title="Containers"
        subtitle={summary}
        subtitleRole="status"
        actions={<RefreshButton busy={refreshing} onClick={refresh} />}
        meta={
          snapshot.dataUpdatedAt
            ? `${engine ? `${engine.podman ? "Podman" : "Docker"} ${engine.version}${engine.rootless ? " (rootless)" : ""} · ` : ""}Updated ${new Date(snapshot.dataUpdatedAt).toLocaleTimeString()}`
            : " "
        }
      />
      {snapshot.error && data && (
        <p
          role="alert"
          className="mb-[12px] text-[12px] leading-[1.6] text-(--danger) wrap-anywhere"
        >
          {String(snapshot.error)} Showing the last reading.
        </p>
      )}
      {data?.truncated && (
        <p className="mb-[12px] text-[11.5px] text-muted-foreground">
          Showing the newest 500 containers.
        </p>
      )}
      {snapshot.error && !data ? (
        <Unavailable
          id={id}
          error={String(snapshot.error)}
          retry={refresh}
          retrying={refreshing}
        />
      ) : (
        <div className={masterGrid}>
          <aside
            aria-label="Container list"
            className={cn(
              "flex min-h-0 min-w-0 flex-col",
              selected && "@max-[600px]/page:hidden",
            )}
          >
            <SearchField
              className="mb-[8px] shrink-0"
              aria-label="Find a container"
              placeholder="Find a container, project or image"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <div
              data-slot="containers-list-scroll"
              className="-mx-[4px] min-h-0 flex-1 overflow-auto overscroll-contain px-[4px] pb-[16px]"
            >
              {data ? (
                <ContainerList
                  groups={groups}
                  selected={key}
                  onSelect={setSelected}
                  collapsed={collapsed}
                  onToggle={(name) =>
                    setCollapsed((previous) => {
                      const next = new Set(previous);
                      if (next.has(name)) next.delete(name);
                      else next.add(name);
                      return next;
                    })
                  }
                  searching={!!needle}
                  now={now}
                />
              ) : (
                <MasterListSkeleton label="Reading containers" />
              )}
            </div>
          </aside>
          <section
            aria-label="Container detail"
            data-slot="containers-detail-scroll"
            className={cn(
              "flex min-h-0 min-w-0 flex-col overflow-auto overscroll-contain pb-[16px]",
              !selected && "@max-[600px]/page:hidden",
            )}
          >
            <Button
              variant="ghost"
              className="mb-[10px] hidden h-[26px] self-start px-[6px] text-[12px] @max-[600px]/page:inline-flex"
              onClick={() => setSelected(null)}
            >
              <ArrowLeft size={13} /> All containers
            </Button>
            {!data ? (
              <DetailSkeleton />
            ) : container ? (
              <ContainerDetail
                key={container.id}
                id={id}
                container={container}
                onSelect={setSelected}
                now={now}
                podman={!!engine?.podman}
              />
            ) : project ? (
              <ProjectDetail
                key={key}
                id={id}
                project={project}
                onSelect={setSelected}
                now={now}
              />
            ) : key === DISK ? (
              <DiskUsage key={key} id={id} now={now} />
            ) : (
              <p className="text-[12px] text-muted-foreground">
                {selected
                  ? "That container is gone. It may have been removed or recreated under another name."
                  : "Choose a container."}
              </p>
            )}
          </section>
        </div>
      )}
    </div>
  );
}
