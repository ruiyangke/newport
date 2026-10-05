import { ScrollText } from "lucide-react";
import {
  Badge,
  MasterGroup,
  MasterListSkeleton,
  MasterNav,
  MasterRow,
  rowClass,
  selectable,
} from "../master/MasterList";
import {
  duration,
  listNote,
  shortName,
  stateOf,
  unitKey,
  type ServiceGroups,
  type ServiceUnit,
  type TimerUnit,
} from "../../domain/services";

export const JOURNAL = "journal";

export function timerNote(timer: TimerUnit, now: number) {
  if (timer.next && timer.next > now) return `in ${duration(timer.next - now)}`;
  if (timer.last) return `${duration(now - timer.last)} ago`;
  return timer.active === "active" ? "waiting" : "off";
}

/**
 * The left pane: what needs attention, the person's own services, timers,
 * and -- folded away -- the operating system's services.
 */
export function ServiceList({
  groups,
  timers,
  selected,
  onSelect,
  systemOpen,
  onToggleSystem,
  searching,
  now,
}: {
  groups: ServiceGroups;
  timers: TimerUnit[];
  selected: string | null;
  onSelect: (key: string) => void;
  systemOpen: boolean;
  onToggleSystem: () => void;
  searching: boolean;
  now: number;
}) {
  const service = (unit: ServiceUnit) => {
    const state = stateOf(unit);
    return (
      <MasterRow
        key={unitKey(unit)}
        selected={selected === unitKey(unit)}
        onSelect={() => onSelect(unitKey(unit))}
        tone={state.tone}
        name={shortName(unit.name)}
        badge={unit.scope === "user" ? <Badge>user</Badge> : undefined}
        note={listNote(unit, now)}
        noteTone={
          state.tone === "danger" || state.tone === "warning"
            ? state.tone
            : undefined
        }
        label={`${unit.name}${unit.scope === "user" ? " (user)" : ""}: ${state.label}`}
      />
    );
  };
  return (
    <MasterNav label="Services" selected={selected}>
      <ul role="list" className="m-0 list-none p-0">
        <li className="mb-[8px]">
          <button
            {...selectable(selected === JOURNAL, () => onSelect(JOURNAL))}
            className={rowClass(selected === JOURNAL)}
          >
            <ScrollText
              size={13}
              aria-hidden="true"
              className="text-muted-foreground"
            />
            System journal
          </button>
        </li>
        <MasterGroup
          title="Problems"
          count={groups.problems.length}
          open
          tone="danger"
        >
          {groups.problems.map(service)}
        </MasterGroup>
        <MasterGroup title="Services" count={groups.owned.length} open>
          {groups.owned.map(service)}
        </MasterGroup>
        <MasterGroup title="Timers" count={timers.length} open>
          {timers.map((timer) => (
            <MasterRow
              key={unitKey(timer)}
              selected={selected === unitKey(timer)}
              onSelect={() => onSelect(unitKey(timer))}
              tone={timer.active === "active" ? "success" : "muted"}
              name={shortName(timer.name)}
              badge={timer.scope === "user" ? <Badge>user</Badge> : undefined}
              note={timerNote(timer, now)}
              label={`${timer.name}${timer.scope === "user" ? " (user)" : ""}: timer, ${timerNote(timer, now)}`}
            />
          ))}
        </MasterGroup>
        <MasterGroup
          title="System"
          count={groups.system.length}
          open={systemOpen || searching}
          onToggle={searching ? undefined : onToggleSystem}
        >
          {groups.system.map(service)}
        </MasterGroup>
      </ul>
      {searching &&
        !groups.problems.length &&
        !groups.owned.length &&
        !groups.system.length &&
        !timers.length && (
          <p
            role="status"
            className="px-[8px] text-[12px] text-muted-foreground"
          >
            No services or timers match.
          </p>
        )}
    </MasterNav>
  );
}

export function ServiceListSkeleton() {
  return <MasterListSkeleton label="Reading services" />;
}
