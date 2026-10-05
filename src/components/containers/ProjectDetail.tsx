import { useState } from "react";
import { toast } from "sonner";
import {
  ArrowUpFromLine,
  ChevronDown,
  LoaderCircle,
  MoreHorizontal,
  Play,
  RotateCw,
  Square,
  Trash2,
} from "lucide-react";
import { cn } from "../../lib/utils";
import { Button } from "../controls";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import { Dot } from "../master/MasterList";
import { DetailTab, DetailTabs, enter } from "../master/Detail";
import { Confirm } from "./parts";
import { ContainerLogToolbar, ContainerLogs } from "./ContainerDetail";
import {
  ComposeActivity,
  ComposeFiles,
  DownDialog,
  ScaleDialog,
  useCompose,
} from "./compose";
import { bytes } from "../../overviewHealth";
import { useWorkspaceState } from "../../state/workspace";
import {
  useContainerActions,
  usePendingActions,
  type ContainerAction,
} from "../../hooks/useContainers";
import {
  composeWords,
  containerKey,
  formatCpu,
  isRunning,
  portLabel,
  projectState,
  publishedPorts,
  stateOf,
  type ComposeAction,
  type Container,
  type ProjectGroup,
  type ProjectVerb,
} from "../../domain/containers";

const toneText = {
  success: "text-(--success)",
  warning: "text-(--warning)",
  danger: "text-(--danger)",
  muted: "text-muted-foreground",
} as const;
const PROGRESS: Record<ProjectVerb, string> = {
  start: "Starting the project…",
  stop: "Stopping the project…",
  restart: "Restarting the project…",
};
const DONE: Record<ProjectVerb, string> = {
  start: "Started",
  stop: "Stopped",
  restart: "Restarted",
};
const up = (
  services: string[] = [],
  options: Partial<Extract<ComposeAction, { kind: "up" }>> = {},
): ComposeAction => ({
  kind: "up",
  services,
  build: false,
  pull: false,
  forceRecreate: false,
  ...options,
});

type Row =
  | { kind: "container"; service: string; container: Container }
  | { kind: "missing"; service: string; profiles: string[] };

/**
 * A Compose project as a whole: how many of its services run, Compose's own
 * actions (Up, Pull, Rebuild, Down, Scale) when the server has the CLI,
 * start/stop/restart of its containers either way, each service at a
 * glance -- created or not -- and its logs, command output and files.
 */
export function ProjectDetail({
  id,
  project: p,
  onSelect,
  now,
}: {
  id: string;
  project: ProjectGroup;
  onSelect: (key: string) => void;
  now: number;
}) {
  const action = useContainerActions(id);
  const pending = usePendingActions(id);
  const compose = useCompose(id, p.name);
  const [tab, setTab] = useWorkspaceState(id, "containers.projectTab");
  const [confirming, setConfirming] = useState<
    "stop" | "restart" | "jobs" | "down" | null
  >(null);
  const [scaling, setScaling] = useState<string | null>(null);
  const running = p.services.filter(isRunning);
  const state = projectState(p);
  const busyVerb =
    pending.find(
      (a): a is Extract<ContainerAction, { kind: "project" }> =>
        a.kind === "project" && a.project === p.name,
    )?.verb ?? null;
  const finishedJobs = p.jobs.filter((c) => !isRunning(c));
  const removingJobs = pending.some(
    (a) =>
      a.kind === "remove" &&
      a.containers.some((x) => finishedJobs.some((j) => j.id === x)),
  );
  const mine =
    action.variables &&
    ((action.variables.kind === "project" &&
      action.variables.project === p.name) ||
      (action.variables.kind === "remove" &&
        action.variables.containers.some((x) =>
          finishedJobs.some((j) => j.id === x),
        )));
  const error = mine && action.error ? String(action.error) : null;
  const run = (verb: ProjectVerb) =>
    action.mutate(
      {
        kind: "project",
        project: p.name,
        verb,
        expectedIds: p.services.map((c) => c.id),
      },
      {
        onSuccess: () => {
          setConfirming(null);
          toast(`${DONE[verb]} ${p.name}`);
        },
      },
    );
  const busy = !!busyVerb || removingJobs || compose.busy;
  const cli = compose.cli;
  const defined = compose.info.data?.services ?? null;
  const builds = new Set(
    (defined ?? []).filter((s) => s.build).map((s) => s.name),
  );
  // Every service the files define, created or not, then any container
  // whose service the files no longer mention.
  const rows: Row[] = [];
  for (const s of defined ?? []) {
    const members = p.services.filter((c) => c.compose?.service === s.name);
    if (members.length)
      for (const c of members)
        rows.push({ kind: "container", service: s.name, container: c });
    else rows.push({ kind: "missing", service: s.name, profiles: s.profiles });
  }
  for (const c of p.services)
    if (!defined?.some((s) => s.name === c.compose?.service))
      rows.push({
        kind: "container",
        service: c.compose?.service ?? c.name,
        container: c,
      });
  const missing = rows.filter((r) => r.kind === "missing").length;
  const replicas = (service: string) =>
    p.services.filter((c) => c.compose?.service === service && isRunning(c))
      .length;
  const start = (a: ComposeAction, show = true) => {
    compose.start(a, show);
  };
  return (
    <article
      aria-label={`${p.name} project`}
      className={cn("flex min-w-0 flex-1 flex-col gap-[14px]", enter)}
    >
      <header>
        <h2 className="flex-wrap text-[17px] wrap-anywhere">
          {p.name}
          <span className="rounded-[4px] bg-muted px-[5px] text-[10.5px]">
            Compose project
          </span>
        </h2>
        {p.workingDir && (
          <p className="mt-[2px] font-mono text-[11.5px] text-muted-foreground wrap-anywhere">
            {p.workingDir}
          </p>
        )}
        <p
          role="status"
          className={cn(
            "mt-[8px] flex flex-wrap items-center gap-x-[8px] text-[12.5px]",
            busyVerb || compose.running
              ? "text-muted-foreground"
              : toneText[state.tone],
          )}
        >
          {busyVerb || compose.running ? (
            <span className="inline-flex items-center gap-[6px] font-semibold">
              <LoaderCircle
                size={13}
                aria-hidden="true"
                className="animate-spin motion-reduce:animate-none"
              />
              {busyVerb ? PROGRESS[busyVerb] : "Running a Compose command…"}
            </span>
          ) : (
            <span className="font-semibold">{state.label}</span>
          )}
          <span className="text-muted-foreground">
            {running.length} of {p.services.length}{" "}
            {p.services.length === 1 ? "service" : "services"} running
            {missing > 0 && ` · ${missing} not created`}
          </span>
          {compose.running && tab !== "activity" && (
            <button
              type="button"
              onClick={() => setTab("activity")}
              className="text-primary underline-offset-2 hover:underline"
            >
              View output
            </button>
          )}
        </p>
      </header>
      <div className="flex flex-col gap-[8px]">
        <div className="flex flex-wrap items-center gap-[6px]">
          {cli ? (
            <Button
              variant={
                running.length < p.services.length || missing
                  ? "default"
                  : "outline"
              }
              disabled={busy}
              title="docker compose up -d: create what’s missing, recreate what changed"
              onClick={() => start(up())}
            >
              <ArrowUpFromLine size={13} /> Up
            </Button>
          ) : (
            running.length < p.services.length && (
              <Button
                variant="default"
                loading={busyVerb === "start"}
                disabled={busy || !p.services.length}
                onClick={() => run("start")}
              >
                <Play size={13} />
                {running.length ? "Start the rest" : "Start all"}
              </Button>
            )
          )}
          {running.length > 0 && (
            <>
              <Button
                variant={
                  !cli && running.length === p.services.length
                    ? "default"
                    : "outline"
                }
                loading={busyVerb === "restart"}
                disabled={busy}
                onClick={() => setConfirming("restart")}
              >
                <RotateCw size={13} /> Restart all
              </Button>
              <Button
                loading={busyVerb === "stop"}
                disabled={busy}
                onClick={() => setConfirming("stop")}
              >
                <Square size={12} /> Stop all
              </Button>
            </>
          )}
          {cli && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="ghost" disabled={busy}>
                  More <ChevronDown size={12} />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start">
                {running.length < p.services.length && (
                  <>
                    <DropdownMenuItem onSelect={() => run("start")}>
                      Start existing containers
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                  </>
                )}
                <DropdownMenuItem
                  onSelect={() => start(up([], { pull: true }))}
                >
                  Pull images & up
                </DropdownMenuItem>
                {builds.size > 0 && (
                  <DropdownMenuItem
                    onSelect={() => start(up([], { build: true }))}
                  >
                    Rebuild & up
                  </DropdownMenuItem>
                )}
                <DropdownMenuItem
                  onSelect={() => start({ kind: "pull", services: [] })}
                >
                  Pull images only
                </DropdownMenuItem>
                {builds.size > 0 && (
                  <DropdownMenuItem
                    onSelect={() =>
                      start({ kind: "build", services: [], noCache: false })
                    }
                  >
                    Build images only
                  </DropdownMenuItem>
                )}
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  variant="destructive"
                  onSelect={() => setConfirming("down")}
                >
                  Down…
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          )}
          {finishedJobs.length > 0 && (
            <Button
              variant="ghost"
              loading={removingJobs}
              disabled={busy}
              onClick={() => setConfirming("jobs")}
            >
              <Trash2 size={13} /> Remove {finishedJobs.length} finished{" "}
              {finishedJobs.length === 1 ? "job" : "jobs"}
            </Button>
          )}
        </div>
        <p className="text-[11.5px] text-muted-foreground">
          {cli ? (
            <>
              <strong className="font-medium">Up</strong> applies the compose
              files: it creates what’s missing and recreates what changed.
              Restart and Stop act on the existing containers. Compose{" "}
              {cli.version}.
            </>
          ) : compose.info.isPending ? (
            "Checking for the Compose CLI…"
          ) : (
            <>
              These act on the project’s existing containers. Up, Pull, Rebuild
              and Down need the Compose CLI on the server
              {compose.info.data?.cliError
                ? `: ${compose.info.data.cliError}`
                : compose.info.error
                  ? `: ${String(compose.info.error)}`
                  : "."}
            </>
          )}
        </p>
        {(error ?? compose.error) && !confirming && (
          <p
            role="alert"
            className="text-[12px] leading-[1.6] text-(--danger) wrap-anywhere"
          >
            {error ?? compose.error}
          </p>
        )}
      </div>
      <table
        aria-label={`${p.name} services`}
        className="w-full border-collapse text-[12px]"
      >
        <thead>
          <tr className="text-left text-[11px] text-muted-foreground">
            <th className="pb-[4px] font-normal">Service</th>
            <th className="pb-[4px] font-normal">State</th>
            <th className="pb-[4px] text-right font-normal">CPU</th>
            <th className="pb-[4px] text-right font-normal">Memory</th>
            <th className="pb-[4px] pl-[14px] font-normal">Ports</th>
            {cli && (
              <th className="w-[1%] pb-[4px]">
                <span className="sr-only">Actions</span>
              </th>
            )}
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => {
            if (row.kind === "missing")
              return (
                <tr
                  key={`missing:${row.service}`}
                  className="border-t border-border"
                >
                  <td className="py-[5px] pr-[10px]">
                    <span className="inline-flex items-center gap-[7px] text-muted-foreground">
                      <Dot tone="muted" />
                      {row.service}
                    </span>
                  </td>
                  <td
                    className="py-[5px] pr-[10px] text-muted-foreground"
                    colSpan={4}
                  >
                    Not created
                    {row.profiles.length > 0 &&
                      ` · profile ${row.profiles.join(", ")}`}
                  </td>
                  {cli && (
                    <td className="py-[3px] text-right">
                      <Button
                        className="h-[24px] px-[8px] text-[11.5px]"
                        disabled={busy}
                        aria-label={`Up ${row.service}`}
                        onClick={() => start(up([row.service]))}
                      >
                        Up
                      </Button>
                    </td>
                  )}
                </tr>
              );
            const c = row.container;
            const s = stateOf(c);
            return (
              <tr key={c.id} className="border-t border-border">
                <td className="py-[5px] pr-[10px]">
                  <button
                    type="button"
                    onClick={() => onSelect(containerKey(c))}
                    className="inline-flex items-center gap-[7px] rounded-[4px] text-left outline-none hover:underline focus-visible:outline-2 focus-visible:outline-ring"
                  >
                    <Dot tone={s.tone} />
                    {row.service}
                    {(c.compose?.number ?? 1) > 1 && (
                      <span className="text-muted-foreground">
                        #{c.compose?.number}
                      </span>
                    )}
                  </button>
                </td>
                <td className={cn("py-[5px] pr-[10px]", toneText[s.tone])}>
                  {s.label}
                </td>
                <td className="py-[5px] text-right tabular-nums">
                  {c.cpu !== null ? formatCpu(c.cpu) : "—"}
                </td>
                <td className="py-[5px] text-right tabular-nums">
                  {c.memory !== null ? bytes(c.memory) : "—"}
                </td>
                <td className="py-[5px] pl-[14px] font-mono text-[11.5px] text-muted-foreground">
                  {publishedPorts(c).map(portLabel).join(", ") || "—"}
                </td>
                {cli && (
                  <td className="py-[3px] text-right">
                    <ServiceMenu
                      service={row.service}
                      build={builds.has(row.service)}
                      disabled={busy}
                      onRun={start}
                      onScale={() => setScaling(row.service)}
                    />
                  </td>
                )}
              </tr>
            );
          })}
        </tbody>
      </table>
      <DetailTabs
        value={tab}
        onValueChange={setTab}
        tabs={[
          { value: "logs", label: "Logs" },
          { value: "activity", label: "Activity" },
          { value: "file", label: "Compose file" },
        ]}
        side={tab === "logs" ? <ContainerLogToolbar id={id} /> : null}
      >
        <DetailTab value="logs">
          <ContainerLogs
            id={id}
            target={{ project: p.name }}
            label={`${p.name} logs`}
          />
        </DetailTab>
        <DetailTab value="activity">
          <ComposeActivity id={id} project={p.name} now={now} />
        </DetailTab>
        <DetailTab value="file">
          <ComposeFiles id={id} project={p.name} />
        </DetailTab>
      </DetailTabs>
      {(confirming === "stop" || confirming === "restart") && (
        <Confirm
          title={`${confirming === "stop" ? "Stop" : "Restart"} ${p.name}?`}
          action={confirming === "stop" ? "Stop all" : "Restart all"}
          busy={!!busyVerb}
          error={error}
          onConfirm={() => run(confirming)}
          onClose={() => setConfirming(null)}
        >
          <p>
            {confirming === "stop"
              ? "These containers stop; anything using them stops working until they start again:"
              : "These containers restart; connections to them drop for a moment:"}
          </p>
          <ul className="m-0 max-h-[160px] list-disc overflow-auto pl-[20px] leading-[1.7]">
            {running.map((c) => (
              <li key={c.id}>
                {c.compose?.service ?? c.name}{" "}
                <span className="text-muted-foreground">({c.name})</span>
              </li>
            ))}
          </ul>
        </Confirm>
      )}
      {confirming === "down" && (
        <DownDialog
          project={p.name}
          busy={compose.busy}
          error={compose.error}
          onConfirm={(options) => {
            setConfirming(null);
            start({ kind: "down", ...options });
          }}
          onClose={() => setConfirming(null)}
        />
      )}
      {scaling && (
        <ScaleDialog
          service={scaling}
          current={replicas(scaling)}
          busy={compose.busy}
          error={compose.error}
          onConfirm={(n) => {
            const service = scaling;
            setScaling(null);
            start({ kind: "scale", service, replicas: n });
          }}
          onClose={() => setScaling(null)}
        />
      )}
      {confirming === "jobs" && (
        <Confirm
          title={`Remove ${finishedJobs.length} finished ${finishedJobs.length === 1 ? "job" : "jobs"}?`}
          action="Remove"
          busy={removingJobs}
          error={error}
          onConfirm={() =>
            action.mutate(
              {
                kind: "remove",
                containers: finishedJobs.map((c) => c.id),
                names: finishedJobs.map((c) => c.name),
              },
              {
                onSuccess: (result) => {
                  setConfirming(null);
                  toast(
                    `Removed ${(result as { removed: number } | null)?.removed ?? finishedJobs.length} jobs from ${p.name}`,
                  );
                },
              },
            )
          }
          onClose={() => setConfirming(null)}
        >
          <p>
            The stopped one-off containers from <code>docker compose run</code>{" "}
            and their logs are deleted. Services, images and volumes stay.
          </p>
        </Confirm>
      )}
    </article>
  );
}

/** One service's Compose actions, at the end of its row. */
export function ServiceMenu({
  service,
  build,
  disabled,
  onRun,
  onScale,
}: {
  service: string;
  build: boolean;
  disabled: boolean;
  onRun: (action: ComposeAction) => void;
  onScale?: () => void;
}) {
  const items: ComposeAction[] = [
    up([service], { forceRecreate: true }),
    up([service], { forceRecreate: true, pull: true }),
    ...(build ? [up([service], { forceRecreate: true, build: true })] : []),
  ];
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          disabled={disabled}
          aria-label={`Compose actions for ${service}`}
        >
          <MoreHorizontal size={15} />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {items.map((a) => (
          <DropdownMenuItem
            key={composeWords(a).label}
            onSelect={() => onRun(a)}
          >
            {composeWords(a).label}
          </DropdownMenuItem>
        ))}
        {onScale && (
          <DropdownMenuItem onSelect={onScale}>Scale…</DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
