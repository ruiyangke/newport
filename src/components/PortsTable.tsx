import {
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
  type SortingState,
} from "@tanstack/react-table";
import { ArrowDown, ArrowUp, ArrowUpDown, GitBranch } from "lucide-react";
import { cn } from "../lib/utils";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./MonitoringTable";
import {
  Identity,
  PortNumber,
  StateDot,
  listeningModel,
  savedModel,
  statusText,
  type RowModel,
  type Shared,
} from "./PortRow";
import { OTHER, type PortEntry, type PortProject } from "../portProjects";
import { direction, type Tunnel } from "../types";

/** One row of the table: a row model plus where it belongs. */
interface Item {
  model: RowModel;
  project: PortProject;
  branch: string | null;
  reverse: boolean;
}

// Problems first, then forwards coming up, up, off, and plain listeners.
function statusRank({ state, problem }: RowModel) {
  if (problem) return 0;
  if (state?.status === "connecting" || state?.status === "reconnecting")
    return 1;
  if (state?.status === "connected") return 2;
  return state ? 3 : 4;
}
function Status({ model }: { model: RowModel }) {
  const rank = statusRank(model);
  const text =
    rank === 0
      ? model.state?.status === "error"
        ? "Failed"
        : "Unreachable"
      : rank === 3
        ? "Off"
        : rank === 4
          ? "—"
          : statusText(model.state);
  return (
    <span
      className={cn(
        "text-[12px] whitespace-nowrap",
        rank === 0 && "font-medium text-(--danger)",
        rank === 1 && "text-(--warning)",
        rank === 2 && "text-(--success)",
        rank >= 3 && "text-muted-foreground",
      )}
    >
      {rank === 4 ? <span aria-label="Not forwarded">—</span> : text}
    </span>
  );
}

const column = createColumnHelper<Item>();
const columns = [
  column.accessor((item) => item.model.portNumber, {
    id: "port",
    header: "Port",
    cell: ({ row }) => (
      <span className="flex items-center gap-[9px] whitespace-nowrap">
        <StateDot state={row.original.model.state} />
        <PortNumber model={row.original.model} />
      </span>
    ),
  }),
  column.accessor((item) => item.model.title.toLowerCase(), {
    id: "service",
    header: "Service",
    cell: ({ row }) => (
      <>
        <Identity model={row.original.model} />
        {/* Narrow panels drop the address column; it moves under the name. */}
        {row.original.model.route && (
          <div className="mt-[3px] -ml-[8px] hidden @max-[640px]/ports:block">
            {row.original.model.route}
          </div>
        )}
      </>
    ),
  }),
  column.accessor(
    (item) =>
      `${item.project.key === OTHER ? "￿" : item.project.name.toLowerCase()} ${item.branch ?? ""}`,
    {
      id: "project",
      header: "Project",
      cell: ({ row }) => {
        const { project, branch, reverse } = row.original;
        return (
          <span
            className="flex min-w-0 flex-col text-[12px] leading-[1.4]"
            title={[project.path ?? project.name, branch]
              .filter(Boolean)
              .join("\n")}
          >
            <span
              className={cn(
                "truncate",
                project.key === OTHER && "text-muted-foreground",
              )}
            >
              {project.key === OTHER ? "Other" : project.name}
            </span>
            {(branch || reverse) && (
              <span className="inline-flex min-w-0 items-center gap-[3px] text-[11px] text-muted-foreground">
                {branch ? (
                  <>
                    <GitBranch size={10} aria-hidden="true" />
                    <code className="truncate text-[11px]">{branch}</code>
                  </>
                ) : (
                  "From this Mac"
                )}
              </span>
            )}
          </span>
        );
      },
    },
  ),
  column.accessor((item) => statusRank(item.model), {
    id: "status",
    header: "Status",
    cell: ({ row }) => <Status model={row.original.model} />,
  }),
  column.display({
    id: "address",
    header: "Local address",
    cell: ({ row }) => row.original.model.route,
  }),
  column.display({
    id: "actions",
    header: () => <span className="sr-only">Actions</span>,
    cell: ({ row }) => (
      <span className="flex items-center justify-end gap-[6px]">
        {row.original.model.menu}
        {row.original.model.control}
      </span>
    ),
  }),
];

// Narrow panels keep port, service, address and the switch.
const hideNarrow: Record<string, string> = {
  project: "@max-[640px]/ports:hidden",
  status: "@max-[640px]/ports:hidden",
  address: "@max-[640px]/ports:hidden",
};
// A fixed layout lets long names and paths truncate instead of pushing the
// columns after them off the page; the service column takes what is left.
const widths: Record<string, string> = {
  port: "w-[84px] @max-[640px]/ports:w-[64px]",
  project: "w-[20%]",
  status: "w-[124px]",
  address: "w-[184px]",
  actions: "w-[84px]",
};

/**
 * The Ports view: every listening port and saved forward in one sortable
 * table, for finding a port by number, scanning status across projects, and
 * comparing worktrees side by side. Rows are the Projects view's own models.
 */
export function PortsTable({
  groups,
  sorting,
  onSortingChange,
  shared,
}: {
  groups: { project: PortProject; ports: PortEntry[]; forwards: Tunnel[] }[];
  sorting: SortingState;
  onSortingChange: (sorting: SortingState) => void;
  shared: Omit<Shared, "projectKey" | "base">;
}) {
  const data = groups.flatMap(({ project, ports, forwards }): Item[] => [
    ...ports.map((entry) => ({
      model: listeningModel({
        ...shared,
        entry,
        projectKey: project.key || null,
        base: entry.worktree?.path ?? project.path,
      }),
      project,
      // A single main checkout needs no branch beside its project.
      branch:
        project.worktrees.length > 1 || entry.worktree?.main === false
          ? (entry.worktree?.branch ?? null)
          : null,
      reverse: false,
    })),
    ...forwards.map((tunnel) => ({
      model: savedModel({
        ...shared,
        tunnel,
        projectKey: project.key || null,
        base: project.path,
      }),
      project,
      branch: null,
      reverse: direction(tunnel) === "remote",
    })),
  ]);
  const table = useReactTable({
    data,
    columns,
    state: { sorting },
    onSortingChange: (updater) =>
      onSortingChange(
        typeof updater === "function" ? updater(sorting) : updater,
      ),
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getRowId: (item) => item.model.key,
    enableSortingRemoval: false,
    // A first click reads top-down: lowest port, A–Z, problems first.
    sortDescFirst: false,
    // Rows are rebuilt each render from live runtime state. Nothing here
    // pages, and an automatic page reset on new data would re-render forever.
    autoResetAll: false,
  });
  return (
    <Table
      aria-label="Ports"
      className="table-fixed"
      containerClassName="max-h-none overflow-visible"
    >
      <TableHeader>
        {table.getHeaderGroups().map((group) => (
          <TableRow key={group.id}>
            {group.headers.map((header) => {
              const sorted = header.column.getIsSorted();
              const sortable = header.column.getCanSort();
              const Icon =
                sorted === "asc"
                  ? ArrowUp
                  : sorted === "desc"
                    ? ArrowDown
                    : ArrowUpDown;
              return (
                <TableHead
                  key={header.id}
                  scope="col"
                  aria-sort={
                    sorted === "asc"
                      ? "ascending"
                      : sorted === "desc"
                        ? "descending"
                        : undefined
                  }
                  className={cn(
                    widths[header.id],
                    hideNarrow[header.id],
                    header.id === "address" && "text-right",
                  )}
                >
                  {sortable ? (
                    <button
                      type="button"
                      onClick={header.column.getToggleSortingHandler()}
                      className="group/sort -mx-[4px] inline-flex items-center gap-[4px] rounded-[4px] px-[4px] outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
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
                  ) : (
                    flexRender(
                      header.column.columnDef.header,
                      header.getContext(),
                    )
                  )}
                </TableHead>
              );
            })}
          </TableRow>
        ))}
      </TableHeader>
      <TableBody>
        {table.getRowModel().rows.map((row) => (
          <TableRow
            key={row.id}
            aria-label={row.original.model.label}
            className="group/row hover:bg-accent/50 focus-within:bg-accent/50"
          >
            {row.getVisibleCells().map((cell) => (
              <TableCell
                key={cell.id}
                className={cn(
                  "max-w-none py-[7px] align-middle",
                  hideNarrow[cell.column.id],
                  cell.column.id === "address" &&
                    "text-right whitespace-nowrap",
                )}
              >
                {flexRender(cell.column.columnDef.cell, cell.getContext())}
              </TableCell>
            ))}
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
