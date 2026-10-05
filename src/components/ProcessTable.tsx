import {
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
} from "@tanstack/react-table";
import { ArrowDown, ArrowUp, ArrowUpDown } from "lucide-react";
import { cn } from "../lib/utils";
import { SectionEmpty } from "./Feedback";
import { SearchField } from "./SearchField";
import { Button } from "./controls";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./MonitoringTable";
import { useWorkspaceState } from "../state/workspace";
import { bytes } from "../overviewHealth";
import type { Overview, Process } from "../domain/cockpit";

/** Rows shown before "Show all"; the agent sends up to a hundred. */
const FIRST_ROWS = 20;

// Only states worth a reader's attention; sleeping is what processes do.
const STATES: Record<string, { label: string; tone: string }> = {
  D: { label: "Waiting on I/O", tone: "text-(--warning)" },
  Z: { label: "Zombie", tone: "text-(--danger)" },
  T: { label: "Stopped", tone: "text-muted-foreground" },
  t: { label: "Traced", tone: "text-muted-foreground" },
};

/** A thin magnitude bar beside a figure; one hue, the number carries it. */
function Bar({ share }: { share: number }) {
  const width = Math.min(100, Math.max(0, share));
  return (
    <span
      aria-hidden="true"
      className="relative h-[4px] w-[44px] shrink-0 overflow-hidden rounded-full @max-[560px]/overview:hidden"
    >
      <span className="absolute inset-0 bg-(--series-1) opacity-15" />
      <span
        className="absolute inset-y-0 left-0 rounded-full bg-(--series-1)"
        style={{ width: `${width}%` }}
      />
    </span>
  );
}

const column = createColumnHelper<Process>();

function columnsFor(memoryTotal: number) {
  return [
    column.accessor((p) => p.name.toLowerCase(), {
      id: "name",
      header: "Process",
      sortDescFirst: false,
      cell: ({ row }) => {
        const p = row.original;
        const state = STATES[p.state];
        return (
          <div className="min-w-0" title={p.command ?? p.name}>
            <div className="flex min-w-0 items-center gap-[7px]">
              <span className="truncate font-medium">{p.name}</span>
              {state && (
                <span className={cn("shrink-0 text-[11px]", state.tone)}>
                  {state.label}
                </span>
              )}
            </div>
            <div className="truncate text-[11px] text-muted-foreground">
              {p.command ?? (p.memory === 0 ? "Kernel thread" : "")}
            </div>
          </div>
        );
      },
    }),
    column.accessor("pid", {
      header: "PID",
      sortDescFirst: false,
      cell: ({ getValue }) => (
        <code className="text-muted-foreground">{getValue()}</code>
      ),
    }),
    column.accessor((p) => p.user.toLowerCase(), {
      id: "user",
      header: "User",
      sortDescFirst: false,
      cell: ({ row }) => (
        <span className="truncate text-muted-foreground">
          {row.original.user}
        </span>
      ),
    }),
    column.accessor("cpu", {
      header: "CPU",
      sortDescFirst: true,
      cell: ({ getValue }) => (
        <span className="flex items-center justify-end gap-[8px]">
          <Bar share={getValue()} />
          <span className="w-[6ch] text-right">{getValue().toFixed(1)}%</span>
        </span>
      ),
    }),
    column.accessor("memory", {
      header: "Memory",
      sortDescFirst: true,
      cell: ({ getValue }) => {
        const share = memoryTotal ? (getValue() / memoryTotal) * 100 : 0;
        return (
          <span
            className="flex items-center justify-end gap-[8px]"
            title={`${share.toFixed(1)}% of memory`}
          >
            <Bar share={share} />
            <span className="w-[9ch] text-right">{bytes(getValue())}</span>
          </span>
        );
      },
    }),
  ];
}

const widths: Record<string, string> = {
  name: "",
  pid: "w-[80px]",
  user: "w-[120px] @max-[560px]/overview:hidden",
  cpu: "w-[140px] @max-[560px]/overview:w-[80px]",
  memory: "w-[160px] @max-[560px]/overview:w-[96px]",
};

/**
 * What runs on the server: the busiest by CPU and the largest by memory, so
 * either sort shows the real leaders. CPU is each process's share of one
 * core over the agent's last second, as htop shows it.
 */
export function ProcessTable({ id, d }: { id: string; d: Overview }) {
  const [filter, setFilter] = useWorkspaceState(id, "overview.filter");
  const [sorting, setSorting] = useWorkspaceState(id, "overview.processSort");
  const [all, setAll] = useWorkspaceState(id, "overview.processesAll");
  const needle = filter.trim().toLowerCase();
  const data = needle
    ? d.processes.filter((p) =>
        `${p.name} ${p.user} ${p.pid} ${p.command ?? ""}`
          .toLowerCase()
          .includes(needle),
      )
    : d.processes;
  const table = useReactTable({
    data,
    columns: columnsFor(d.memoryTotal),
    state: { sorting },
    onSortingChange: (updater) =>
      setSorting(typeof updater === "function" ? updater(sorting) : updater),
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getRowId: (p) => String(p.pid),
    enableSortingRemoval: false,
    // Rows are rebuilt from every reading; nothing pages.
    autoResetAll: false,
  });
  const rows = table.getRowModel().rows;
  const shown = needle || all ? rows : rows.slice(0, FIRST_ROWS);
  return (
    <section aria-label="Processes" className="min-w-0">
      <header className="mb-[6px] flex min-h-[28px] flex-wrap items-center justify-between gap-[10px]">
        <h2>
          Processes <span>{d.processCount}</span>
        </h2>
        <SearchField
          className="w-[260px]"
          aria-label="Filter processes"
          placeholder="Find a process, command, user, or PID"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
      </header>
      <p className="mb-[8px] text-[11px] text-muted-foreground">
        {needle
          ? `Showing ${rows.length} of ${d.processes.length} collected processes`
          : `The busiest and largest ${d.processes.length} of ${d.processCount} · CPU over the last second, as a share of one core`}
      </p>
      {shown.length ? (
        <Table
          aria-label="Processes"
          className="table-fixed"
          containerClassName="max-h-none overflow-visible"
        >
          <TableHeader>
            {table.getHeaderGroups().map((group) => (
              <TableRow key={group.id}>
                {group.headers.map((header) => {
                  const sorted = header.column.getIsSorted();
                  const Icon =
                    sorted === "asc"
                      ? ArrowUp
                      : sorted === "desc"
                        ? ArrowDown
                        : ArrowUpDown;
                  const numeric = ["cpu", "memory"].includes(header.id);
                  return (
                    <TableHead
                      key={header.id}
                      scope="col"
                      numeric={numeric}
                      aria-sort={
                        sorted === "asc"
                          ? "ascending"
                          : sorted === "desc"
                            ? "descending"
                            : undefined
                      }
                      className={cn(widths[header.id], "font-sans")}
                    >
                      <button
                        type="button"
                        onClick={header.column.getToggleSortingHandler()}
                        className={cn(
                          "group/sort -mx-[4px] inline-flex items-center gap-[4px] rounded-[4px] px-[4px] outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring",
                          numeric && "flex-row-reverse",
                        )}
                      >
                        {flexRender(
                          header.column.columnDef.header,
                          header.getContext(),
                        )}
                        <Icon
                          size={11}
                          aria-hidden="true"
                          className={cn(
                            !sorted &&
                              "opacity-0 group-hover/sort:opacity-60 group-focus-visible/sort:opacity-60",
                          )}
                        />
                      </button>
                    </TableHead>
                  );
                })}
              </TableRow>
            ))}
          </TableHeader>
          <TableBody>
            {shown.map((row) => (
              <TableRow key={row.id}>
                {row.getVisibleCells().map((cell) => (
                  <TableCell
                    key={cell.id}
                    numeric={["cpu", "memory"].includes(cell.column.id)}
                    className={cn(
                      "max-w-none py-[6px] align-middle",
                      widths[cell.column.id],
                    )}
                  >
                    {flexRender(cell.column.columnDef.cell, cell.getContext())}
                  </TableCell>
                ))}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : (
        <SectionEmpty>
          {needle
            ? "No matches in the collected processes."
            : "No process information available."}
        </SectionEmpty>
      )}
      {!needle && rows.length > FIRST_ROWS && (
        <Button
          variant="ghost"
          className="mt-[6px] h-[26px] px-[8px] text-[12px] text-muted-foreground hover:text-foreground"
          aria-expanded={all}
          onClick={() => setAll(!all)}
        >
          {all ? "Show fewer" : `Show all ${rows.length}`}
        </Button>
      )}
    </section>
  );
}
