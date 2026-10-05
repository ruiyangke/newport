import {
  ChevronRight,
  Container,
  Cpu,
  Folder,
  FolderGit2,
  GitBranch,
  Laptop,
  MoreHorizontal,
  Plus,
  Square,
  Zap,
} from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "../lib/utils";
import { Button } from "./controls";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "./ui/collapsible";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import { ListeningPortRow, SavedForwardRow } from "./PortRow";
import {
  OTHER,
  planForwards,
  projectTunnels,
  tildify,
  worktreeSections,
  type PortEntry,
  type PortProject,
  type Worktree,
} from "../portProjects";
import type { TunnelActions } from "../hooks/useTunnelActions";
import {
  active,
  direction,
  newTunnel,
  tunnelState,
  type Snapshot,
  type Tunnel,
  type TunnelDeletion,
  type TunnelEditor,
} from "../types";

// What each root marker says about the project, in a word.
const markers: Record<string, string> = {
  ".git": "Git",
  ".hg": "Mercurial",
  ".jj": "Jujutsu",
  ".sl": "Sapling",
  ".svn": "Subversion",
  ".bzr": "Bazaar",
  ".fslckout": "Fossil",
  _FOSSIL_: "Fossil",
  _darcs: "Darcs",
  ".projectile": "Projectile",
  "package.json": "Node",
  "deno.json": "Deno",
  "deno.jsonc": "Deno",
  "Cargo.toml": "Rust",
  "go.mod": "Go",
  "pyproject.toml": "Python",
  "setup.py": "Python",
  "requirements.txt": "Python",
  Gemfile: "Ruby",
  "composer.json": "PHP",
  "pom.xml": "Maven",
  "build.gradle": "Gradle",
  "build.gradle.kts": "Gradle",
  "build.sbt": "sbt",
  "mix.exs": "Elixir",
  "flake.nix": "Nix",
  "stack.yaml": "Haskell",
  "dune-project": "OCaml",
  "project.clj": "Clojure",
  "deps.edn": "Clojure",
  "pubspec.yaml": "Dart",
  "Package.swift": "Swift",
  "CMakeLists.txt": "CMake",
  Makefile: "Make",
  compose: "Compose",
  "compose.yaml": "Compose",
  "compose.yml": "Compose",
  "docker-compose.yml": "Compose",
  "docker-compose.yaml": "Compose",
};

type Props = {
  project: PortProject;
  /** Rows the page filter keeps; the project's totals still count all. */
  ports: PortEntry[];
  forwards: Tunnel[];
  serverId: string;
  tunnels: Tunnel[];
  runtime: Snapshot["runtime"];
  actions: TunnelActions;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  edit: (editor: Omit<TunnelEditor, "kind" | "projects">) => void;
  setDeleting: (target: TunnelDeletion) => void;
};

/**
 * Forward-all for a set of rows: forwards every port none reaches and
 * reconnects saved forwards that are off; once all are up, it stops them.
 * One row needs no batch -- its own switch does the same.
 */
function batchFor(
  project: PortProject,
  entries: PortEntry[],
  saved: Tunnel[],
  {
    tunnels,
    serverId,
    runtime,
    actions,
  }: Pick<Props, "tunnels" | "serverId" | "runtime" | "actions">,
  target: string,
) {
  const all = projectTunnels(project, entries, saved);
  const live = all.filter((t) => active(tunnelState(runtime, t.id).status));
  const idle = all.filter((t) => !active(tunnelState(runtime, t.id).status));
  const plan =
    project.key === OTHER
      ? []
      : planForwards(project, tunnels, serverId, entries);
  const rows = entries.length + saved.length;
  const worth = project.key !== OTHER && rows > 1;
  return {
    plan,
    live,
    canForward: worth && plan.length + idle.length > 0,
    canStop: worth && live.length > 0,
    busy: actions.pendingTargets.has(target) || actions.allPending,
    forwardAll: () =>
      actions.forward(
        plan,
        idle.map((t) => t.id),
        target,
      ),
    stopAll: () => actions.connectAll(live, false, target),
  };
}

function BatchButton({
  batch,
  name,
  quiet = false,
}: {
  batch: ReturnType<typeof batchFor>;
  name: string;
  quiet?: boolean;
}) {
  const reveal =
    quiet &&
    !batch.busy &&
    "opacity-0 transition-opacity group-hover/heading:opacity-100 group-focus-within/heading:opacity-100 motion-reduce:transition-none";
  const style = cn(
    "h-[26px] px-[8px] text-[12px] text-muted-foreground hover:text-foreground",
    reveal,
  );
  if (batch.canForward)
    return (
      <Button
        variant="ghost"
        className={style}
        loading={batch.busy}
        aria-label={`Forward all ports in ${name}`}
        title={
          batch.plan.length
            ? `Add ${batch.plan.length} ${batch.plan.length === 1 ? "forward" : "forwards"} and connect`
            : "Connect every saved forward"
        }
        onClick={batch.forwardAll}
      >
        <Zap size={13} />
        Forward all
      </Button>
    );
  if (batch.canStop)
    return (
      <Button
        variant="ghost"
        className={style}
        loading={batch.busy}
        aria-label={`Stop all forwards in ${name}`}
        onClick={batch.stopAll}
      >
        <Square size={11} />
        Stop all
      </Button>
    );
  return null;
}

/** A small label inside a project; rows below keep the page's columns. */
function Subheading({
  icon,
  children,
  action,
}: {
  icon: ReactNode;
  children: ReactNode;
  action?: ReactNode;
}) {
  return (
    <li
      role="presentation"
      data-slot="worktree-heading"
      className="group/heading flex min-h-[28px] items-center gap-[6px] px-[12px] pt-[8px] text-[11.5px] text-muted-foreground"
    >
      {icon}
      {children}
      <span className="flex-1" />
      {action}
    </li>
  );
}

function worktreeWhere(worktree: Worktree, root: string | null) {
  // Worktrees usually live inside the repository or beside it.
  const parent = root?.replace(/\/[^/]+$/, "");
  if (root && worktree.path.startsWith(`${root}/`))
    return `${worktree.path.slice(root.length + 1)}/`;
  if (parent && worktree.path.startsWith(`${parent}/`))
    return `../${worktree.path.slice(parent.length + 1)}/`;
  return worktree.path;
}

function Section({
  worktree,
  entries,
  project,
  showHeading,
  props,
}: {
  worktree: Worktree | null;
  entries: PortEntry[];
  project: PortProject;
  showHeading: boolean;
  props: Props;
}) {
  const batch = batchFor(
    project,
    entries,
    [],
    props,
    `${project.key}#${worktree?.path ?? ""}`,
  );
  const name = worktree?.branch ?? "";
  return (
    <>
      {showHeading && worktree && (
        <Subheading
          icon={<GitBranch size={12} aria-hidden="true" />}
          action={<BatchButton batch={batch} name={name} quiet />}
        >
          <code className="text-[11.5px] font-medium text-foreground/85">
            {name || worktreeWhere(worktree, project.path)}
          </code>
          {!worktree.main && (
            <span className="min-w-0 truncate" title={worktree.path}>
              worktree · {worktreeWhere(worktree, project.path)}
            </span>
          )}
        </Subheading>
      )}
      {entries.map((entry) => (
        <ListeningPortRow
          key={entry.key}
          entry={entry}
          serverId={props.serverId}
          projectKey={project.key || null}
          base={worktree?.path ?? project.path}
          runtime={props.runtime}
          actions={props.actions}
          edit={props.edit}
          setDeleting={props.setDeleting}
        />
      ))}
    </>
  );
}

function ProjectIcon({ project }: { project: PortProject }) {
  const Icon =
    project.key === OTHER
      ? Cpu
      : project.source === "compose"
        ? Container
        : project.source === "saved" || project.source === "vcs"
          ? FolderGit2
          : Folder;
  return (
    <Icon
      size={15}
      aria-hidden="true"
      className="shrink-0 text-muted-foreground"
    />
  );
}

export function PortProjectSection(props: Props) {
  const {
    project,
    ports,
    forwards,
    serverId,
    runtime,
    open,
    onOpenChange,
    edit,
    setDeleting,
    actions,
  } = props;
  const isOther = project.key === OTHER;
  const batch = batchFor(
    project,
    project.ports,
    project.forwards.filter((t) => direction(t) === "local"),
    props,
    project.key,
  );
  const sections = worktreeSections(ports);
  // One checkout needs no headings; its branch joins the project's.
  const single =
    project.worktrees.length === 1 && project.worktrees[0].main
      ? project.worktrees[0]
      : null;
  const showHeadings = !single && project.worktrees.length > 0;
  const idle = forwards.filter((t) => direction(t) === "local");
  const reverse = forwards.filter((t) => direction(t) === "remote");
  const total = project.ports.length + project.forwards.length;
  // Rows with a saved forward, whether or not it is up right now.
  const forwarded =
    project.ports.filter((entry) => entry.forwards.length).length +
    project.forwards.length;
  const live = projectTunnels(project).some((t) =>
    active(tunnelState(runtime, t.id).status),
  );
  const marker = project.marker ? markers[project.marker] : null;
  const where = isOther
    ? null
    : project.path
      ? tildify(
          project.path,
          project.ports.map((entry) => entry.port.user),
        )
      : project.source === "compose"
        ? "Docker Compose"
        : null;
  const savedRow = (t: Tunnel) => (
    <SavedForwardRow
      key={t.id}
      tunnel={t}
      serverId={serverId}
      projectKey={project.key || null}
      base={project.path}
      runtime={runtime}
      actions={actions}
      edit={edit}
      setDeleting={setDeleting}
    />
  );
  return (
    <Collapsible open={open} onOpenChange={onOpenChange} asChild>
      <section
        data-slot="port-project"
        aria-label={`${project.name} project`}
        className="border-t border-border/60 py-[10px] first:border-t-0 first:pt-[2px]"
      >
        <header className="group/heading flex min-h-[34px] items-center gap-[8px] pr-[12px] pl-[4px]">
          <CollapsibleTrigger asChild>
            <button
              type="button"
              className="group/trigger flex min-w-0 flex-1 items-center gap-[8px] rounded-[6px] py-[4px] text-left outline-none focus-visible:outline-2 focus-visible:outline-ring"
              aria-label={`${open ? "Collapse" : "Expand"} ${project.name}`}
            >
              <ChevronRight
                size={13}
                aria-hidden="true"
                className="shrink-0 text-muted-foreground/70 transition-transform group-data-[state=open]/trigger:rotate-90 motion-reduce:transition-none"
              />
              <ProjectIcon project={project} />
              <span className="shrink-0 truncate text-[14px] font-semibold tracking-[-0.01em]">
                {project.name}
              </span>
              <span className="flex min-w-0 items-center gap-[8px] text-[11.5px] text-muted-foreground">
                {single?.branch && (
                  <span className="inline-flex shrink-0 items-center gap-[3px]">
                    <GitBranch size={11} aria-hidden="true" />
                    <code className="text-[11px]">{single.branch}</code>
                  </span>
                )}
                {where && (
                  <span
                    className="truncate"
                    title={[
                      project.path,
                      marker && `Identified by ${project.marker}`,
                    ]
                      .filter(Boolean)
                      .join("\n")}
                  >
                    {where}
                  </span>
                )}
                {!marker || isOther ? null : (
                  <span className="sr-only">{marker}</span>
                )}
              </span>
            </button>
          </CollapsibleTrigger>
          <span
            className={cn(
              "shrink-0 text-[11.5px] tabular-nums",
              // A narrow header keeps the project's name over its tally.
              (batch.canForward || batch.canStop) &&
                "@max-[560px]/ports:hidden",
              live ? "text-foreground/80" : "text-muted-foreground",
            )}
          >
            {forwarded
              ? `${forwarded} of ${total} forwarded`
              : `${total} ${isOther ? "services" : total === 1 ? "port" : "ports"}`}
          </span>
          <BatchButton batch={batch} name={project.name} />
          {!isOther && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className="size-[26px] opacity-0 transition-opacity group-hover/heading:opacity-100 group-focus-within/heading:opacity-100 data-[state=open]:opacity-100 motion-reduce:transition-none"
                  aria-label={`Actions for ${project.name}`}
                >
                  <MoreHorizontal size={15} />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem
                  onSelect={() =>
                    edit({
                      value: { ...newTunnel(serverId), project: project.key },
                      existing: false,
                    })
                  }
                >
                  <Plus size={14} /> New forward here
                </DropdownMenuItem>
                {batch.canForward && batch.canStop && (
                  <DropdownMenuItem onSelect={batch.stopAll}>
                    <Square size={12} /> Stop all
                  </DropdownMenuItem>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        </header>
        <CollapsibleContent>
          <ul
            role="list"
            aria-label={`${project.name} ports`}
            className="m-0 mt-[2px] list-none p-0"
          >
            {sections.map(({ worktree, entries }) => (
              <Section
                key={worktree?.path ?? ""}
                worktree={worktree}
                entries={entries}
                project={project}
                showHeading={showHeadings}
                props={props}
              />
            ))}
            {idle.map(savedRow)}
            {reverse.length > 0 && (
              <Subheading icon={<Laptop size={12} aria-hidden="true" />}>
                <span className="font-medium text-foreground/85">
                  From this Mac
                </span>
                <span>shared with the server</span>
              </Subheading>
            )}
            {reverse.map(savedRow)}
          </ul>
        </CollapsibleContent>
      </section>
    </Collapsible>
  );
}
