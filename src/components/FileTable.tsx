import type { ComponentProps } from "react";
import { cn } from "../lib/utils";
import { TableCell, TableHead } from "./ui/table";

type Column = { column?: "size" | "modified" | "actions" };
function columnStyle(column: Column["column"]) {
  return column === "size"
    ? "w-[76px] text-right tabular-nums"
    : column === "modified"
      ? "w-[94px] text-[11px] text-muted-foreground max-[800px]:hidden"
      : column === "actions"
        ? "w-[72px] px-[3px]"
        : "pl-[12px]";
}
export function FileTableHead({
  column,
  className,
  ...props
}: ComponentProps<typeof TableHead> & Column) {
  return (
    <TableHead
      className={cn(
        "sticky top-0 z-[1] h-[30px] bg-(--native-toolbar) text-[11px]",
        columnStyle(column),
        className,
      )}
      {...props}
    />
  );
}
export function FileTableCell({
  column,
  className,
  ...props
}: ComponentProps<typeof TableCell> & Column) {
  return (
    <TableCell
      className={cn(
        "h-[36px] py-[2px] text-[12px]",
        columnStyle(column),
        className,
      )}
      {...props}
    />
  );
}
