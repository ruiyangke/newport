import { useState } from "react";
import { ChevronRight, HardDrive } from "lucide-react";
import { cn } from "../../lib/utils";
import {
  Badge,
  Dot,
  MasterGroup,
  MasterNav,
  MasterRow,
  noteTone,
  rowClass,
  selectable,
} from "../master/MasterList";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "../ui/collapsible";
import {
  DISK,
  containerKey,
  displayName,
  listNote,
  projectKey,
  projectState,
  stateOf,
  type Container,
  type ContainerGroups,
  type ProjectGroup,
} from "../../domain/containers";

function ContainerRow({
  container: c,
  selected,
  onSelect,
  now,
  context,
  className,
}: {
  container: Container;
  selected: string | null;
  onSelect: (key: string) => void;
  now: number;
  /** Shown beside the name where the project isn't otherwise clear. */
  context?: string;
  className?: string;
}) {
  const state = stateOf(c);
  const key = containerKey(c);
  return (
    <MasterRow
      selected={selected === key}
      onSelect={() => onSelect(key)}
      tone={state.tone}
      name={displayName(c)}
      badge={
        c.compose?.oneoff ? (
          <Badge>job</Badge>
        ) : context ? (
          <span className="max-w-[45%] shrink truncate text-[11px] text-muted-foreground">
            {context}
          </span>
        ) : undefined
      }
      note={listNote(c, now)}
      noteTone={
        state.tone === "danger" || state.tone === "warning"
          ? state.tone
          : undefined
      }
      label={`${c.name}: ${state.label}`}
      className={className}
    />
  );
}

/**
 * A Compose project: its header row opens the project (state, controls,
 * merged logs) and its chevron folds the members away.
 */
function Project({
  project: p,
  open,
  onToggle,
  selected,
  onSelect,
  now,
}: {
  project: ProjectGroup;
  open: boolean;
  onToggle: () => void;
  selected: string | null;
  onSelect: (key: string) => void;
  now: number;
}) {
  const [jobsOpen, setJobsOpen] = useState(false);
  const state = projectState(p);
  const key = projectKey(p.name);
  return (
    <Collapsible asChild open={open} onOpenChange={onToggle}>
      <li role="presentation" className="mt-[2px]">
        <div className="flex items-center">
          <CollapsibleTrigger
            aria-label={`${open ? "Fold" : "Unfold"} ${p.name}`}
            className="grid size-[22px] shrink-0 place-items-center rounded-[5px] text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
          >
            <ChevronRight
              size={12}
              aria-hidden="true"
              className={cn(
                "transition-transform duration-200 motion-reduce:transition-none",
                open && "rotate-90",
              )}
            />
          </CollapsibleTrigger>
          <button
            {...selectable(selected === key, () => onSelect(key))}
            aria-label={`${p.name}: Compose project, ${state.label}`}
            className={cn(rowClass(selected === key), "pl-[6px]")}
          >
            <Dot tone={state.tone} />
            <span className="min-w-0 flex-1 truncate font-medium">
              {p.name}
            </span>
            <span
              className={cn(
                "shrink-0 text-[11px] tabular-nums",
                noteTone(p.problems ? "danger" : undefined),
              )}
            >
              {p.problems
                ? `${p.problems} ${p.problems === 1 ? "issue" : "issues"}`
                : p.services.length
                  ? `${p.running}/${p.services.length}`
                  : ""}
            </span>
          </button>
        </div>
        <CollapsibleContent className="overflow-hidden data-[state=closed]:animate-collapsible-up data-[state=open]:animate-collapsible-down motion-reduce:animate-none">
          <ul
            role="list"
            aria-label={`${p.name} containers`}
            className="m-0 list-none p-0"
          >
            {p.services.map((c) => (
              <ContainerRow
                key={c.id}
                container={c}
                selected={selected}
                onSelect={onSelect}
                now={now}
                className="pl-[30px]"
              />
            ))}
            {p.jobs.length > 0 && (
              <li>
                <button
                  type="button"
                  aria-expanded={jobsOpen}
                  onClick={() => setJobsOpen(!jobsOpen)}
                  className="flex w-full items-center gap-[5px] rounded-[6px] py-[4px] pr-[8px] pl-[30px] text-left text-[11.5px] text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
                >
                  <ChevronRight
                    size={11}
                    aria-hidden="true"
                    className={cn(
                      "transition-transform duration-200 motion-reduce:transition-none",
                      jobsOpen && "rotate-90",
                    )}
                  />
                  {p.jobs.length} one-off {p.jobs.length === 1 ? "job" : "jobs"}
                </button>
                {jobsOpen && (
                  <ul
                    role="list"
                    aria-label={`${p.name} jobs`}
                    className="m-0 list-none p-0"
                  >
                    {p.jobs.map((c) => (
                      <ContainerRow
                        key={c.id}
                        container={c}
                        selected={selected}
                        onSelect={onSelect}
                        now={now}
                        className="pl-[44px]"
                      />
                    ))}
                  </ul>
                )}
              </li>
            )}
          </ul>
        </CollapsibleContent>
      </li>
    </Collapsible>
  );
}

/**
 * The left pane: Compose projects (those needing attention first) with
 * their containers, standalone containers, and disk usage. A container in
 * trouble stays with its project, marked, rather than listed twice.
 */
export function ContainerList({
  groups,
  selected,
  onSelect,
  collapsed,
  onToggle,
  searching,
  now,
}: {
  groups: ContainerGroups;
  selected: string | null;
  onSelect: (key: string) => void;
  collapsed: ReadonlySet<string>;
  onToggle: (project: string) => void;
  searching: boolean;
  now: number;
}) {
  const empty = !groups.projects.length && !groups.standalone.length;
  return (
    <MasterNav label="Containers" selected={selected}>
      <ul role="list" className="m-0 list-none p-0">
        <li className="mb-[8px]">
          <button
            {...selectable(selected === DISK, () => onSelect(DISK))}
            className={rowClass(selected === DISK)}
          >
            <HardDrive
              size={13}
              aria-hidden="true"
              className="text-muted-foreground"
            />
            Disk usage
          </button>
        </li>
        <MasterGroup title="Projects" count={groups.projects.length} open>
          {groups.projects.map((p) => (
            <Project
              key={p.name}
              project={p}
              open={searching || !collapsed.has(p.name)}
              onToggle={() => onToggle(p.name)}
              selected={selected}
              onSelect={onSelect}
              now={now}
            />
          ))}
        </MasterGroup>
        <MasterGroup title="Standalone" count={groups.standalone.length} open>
          {groups.standalone.map((c) => (
            <ContainerRow
              key={c.id}
              container={c}
              selected={selected}
              onSelect={onSelect}
              now={now}
            />
          ))}
        </MasterGroup>
      </ul>
      {empty && (
        <p
          role="status"
          className="px-[8px] pt-[4px] text-[12px] text-muted-foreground"
        >
          {searching
            ? "No projects or containers match."
            : "No containers on this server yet."}
        </p>
      )}
    </MasterNav>
  );
}
