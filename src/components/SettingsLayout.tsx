import type { ComponentProps } from "react";
import { cn } from "../lib/utils";

export function SettingsSection({
  className,
  ...props
}: ComponentProps<"section">) {
  return (
    <section
      className={cn(
        "border-b border-border py-[22px] first:pt-[4px]",
        className,
      )}
      {...props}
    />
  );
}
export function SettingsHeading({ className, ...props }: ComponentProps<"h2">) {
  return (
    <h2
      className={cn("m-0 mb-[16px] text-[13px] font-semibold", className)}
      {...props}
    />
  );
}
export function SettingsRow({
  className,
  wrap,
  ...props
}: ComponentProps<"div"> & { wrap?: boolean }) {
  return (
    <div
      data-slot="settings-row"
      className={cn(
        "flex items-center justify-between gap-[24px] text-[12px]",
        wrap && "flex-wrap gap-y-[12px]",
        className,
      )}
      {...props}
    />
  );
}
export function SettingsHelp({ className, ...props }: ComponentProps<"small">) {
  return (
    <small
      className={cn(
        "mt-[4px] block text-[12px] leading-[1.6] text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}
export function SettingsMessage({ className, ...props }: ComponentProps<"p">) {
  return (
    <p
      className={cn(
        "text-[12px] leading-[1.6] text-muted-foreground [&+p]:mt-[8px]",
        className,
      )}
      {...props}
    />
  );
}
