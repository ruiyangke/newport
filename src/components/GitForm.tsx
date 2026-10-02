import type { ComponentProps } from "react";
import { cn } from "../lib/utils";

const formLayout =
  "grid gap-[16px] pt-[16px] group-data-[full-page=true]/repository:max-w-[640px]";
const dialogLayout =
  "min-h-0 overflow-y-auto px-[20px] pt-[16px] pb-[20px] wrap-anywhere";

export function GitForm({
  dialog = false,
  className,
  ...props
}: ComponentProps<"form"> & { dialog?: boolean }) {
  return (
    <form
      data-slot="git-project-form"
      className={cn(
        formLayout,
        dialog && dialogLayout,
        dialog && "flex flex-col",
        className,
      )}
      {...props}
    />
  );
}
export function GitFormFields({
  dialog = false,
  className,
  ...props
}: ComponentProps<"div"> & { dialog?: boolean }) {
  return (
    <div
      data-slot="git-project-form"
      className={cn(formLayout, dialog && dialogLayout, className)}
      {...props}
    />
  );
}
export function GitField({ className, ...props }: ComponentProps<"label">) {
  return <label className={cn("grid gap-[6px]", className)} {...props} />;
}
export function GitFieldHint({ className, ...props }: ComponentProps<"small">) {
  return (
    <small
      className={cn("text-[12px] text-muted-foreground", className)}
      {...props}
    />
  );
}
export function GitFormActions({
  className,
  ...props
}: ComponentProps<"footer">) {
  return (
    <footer
      className={cn("flex justify-end gap-[8px]", className)}
      {...props}
    />
  );
}
export function GitCheckboxField({
  compact = false,
  className,
  ...props
}: ComponentProps<"label"> & { compact?: boolean }) {
  return (
    <label
      className={cn(
        "flex items-center gap-[8px]",
        compact ? "text-[12px]" : "text-[13px]",
        className,
      )}
      {...props}
    />
  );
}
export function GitPathField({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      className={cn(
        "flex items-center gap-[8px] [&_input]:min-w-0 [&_input]:flex-1",
        className,
      )}
      {...props}
    />
  );
}
export function GitConfirmationBody({
  className,
  ...props
}: ComponentProps<"div">) {
  return (
    <div
      data-slot="git-integration-confirm"
      className={cn(
        "grid gap-[14px] px-[24px] pb-[24px] text-[13px] wrap-anywhere",
        className,
      )}
      {...props}
    />
  );
}
export function GitError({ className, ...props }: ComponentProps<"p">) {
  return (
    <p
      role="alert"
      className={cn(
        "m-0 flex flex-wrap items-center gap-[8px] text-(--danger) wrap-anywhere",
        className,
      )}
      {...props}
    />
  );
}
