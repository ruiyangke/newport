import type { ComponentProps } from "react";
import { cn } from "../lib/utils";
import {
  Table as BaseTable,
  TableHead as BaseHead,
  TableCell as BaseCell,
  TableRow as BaseRow,
} from "./ui/table";
export { TableBody, TableHeader } from "./ui/table";

const numericStyle =
  "text-right font-[ui-monospace,SFMono-Regular,Menlo,monospace] whitespace-nowrap tabular-nums";
export function Table({
  className,
  kind,
  ...props
}: ComponentProps<typeof BaseTable> & { kind?: "history" }) {
  return (
    <BaseTable
      data-slot="cockpit-table"
      className={cn(
        "w-full border-separate border-spacing-0 text-[12px]",
        kind === "history" &&
          "[&_td]:whitespace-nowrap [&_td:not(:first-child)]:text-right [&_td:not(:first-child)]:tabular-nums",
        className,
      )}
      {...props}
    />
  );
}
export function TableHead({
  className,
  numeric,
  ...props
}: ComponentProps<typeof BaseHead> & { numeric?: boolean }) {
  return (
    <BaseHead
      className={cn(
        "sticky top-0 z-[2] h-[28px] bg-(--native-toolbar) px-[10px] py-[8px] text-[11px] font-medium text-muted-foreground [box-shadow:0_1px_0_var(--border)]",
        numeric && numericStyle,
        className,
      )}
      {...props}
    />
  );
}
export function TableCell({
  className,
  numeric,
  ...props
}: ComponentProps<typeof BaseCell> & { numeric?: boolean }) {
  return (
    <BaseCell
      className={cn(
        "max-w-[220px] px-[10px] py-[8px] text-[12px] whitespace-normal wrap-anywhere [&_small]:mt-[4px] [&_small]:block",
        numeric && numericStyle,
        className,
      )}
      {...props}
    />
  );
}
export function TableRow({
  className,
  ...props
}: ComponentProps<typeof BaseRow>) {
  return (
    <BaseRow
      className={cn(
        "even:bg-[color-mix(in_srgb,var(--foreground)_2%,transparent)]",
        className,
      )}
      {...props}
    />
  );
}
