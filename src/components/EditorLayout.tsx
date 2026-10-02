import type { ComponentProps } from "react";
import { cn } from "../lib/utils";

export function EditorBody({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="form-body"
      className={cn(
        "flex min-h-0 flex-col gap-[20px] overflow-y-auto px-[24px] pt-[8px] pb-[20px] [&>p]:text-[12px] [&>p]:leading-[1.65] [@media(max-height:560px),(max-width:540px)]:gap-[16px] [@media(max-height:560px),(max-width:540px)]:px-[18px] [@media(max-height:560px),(max-width:540px)]:pb-[14px]",
        className,
      )}
      {...props}
    />
  );
}
export function EditorFooter({
  className,
  ...props
}: ComponentProps<"footer">) {
  return (
    <footer
      data-slot="dialog-footer"
      className={cn(
        "flex shrink-0 flex-wrap justify-end gap-[8px] px-[24px] pt-[12px] pb-[20px] [&>button]:min-h-[32px] [&>button]:min-w-[76px] [&>button]:rounded-[7px] [@media(max-height:560px),(max-width:540px)]:px-[18px] [@media(max-height:560px),(max-width:540px)]:pt-[10px] [@media(max-height:560px),(max-width:540px)]:pb-[14px]",
        className,
      )}
      {...props}
    />
  );
}
export function EditorSection({
  className,
  ...props
}: ComponentProps<"fieldset">) {
  return (
    <fieldset
      data-slot="form-section"
      className={cn(
        "m-0 grid min-w-0 gap-[14px] border-0 p-0 [&>legend]:mb-[12px] [&>legend]:p-0 [&>legend]:text-[11px] [&>legend]:font-medium [&>legend]:text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}
export function EditorCheckboxField({
  className,
  ...props
}: ComponentProps<"label">) {
  return (
    <label
      data-slot="check-field"
      className={cn(
        "flex items-start gap-[9px] text-[12px] leading-[1.6] [&_[data-slot=checkbox]]:mt-[3px] [&_small]:mt-[3px] [&_small]:block",
        className,
      )}
      {...props}
    />
  );
}
