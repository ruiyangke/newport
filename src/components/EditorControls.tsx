import type { ComponentProps } from "react";
import { cn } from "../lib/utils";
import { Input, Select } from "./controls";

const editorControl =
  "h-[34px] rounded-[7px] border-border bg-[color-mix(in_srgb,var(--native-toolbar)_45%,var(--background))] text-[12px] font-normal shadow-none focus-visible:border-ring focus-visible:shadow-[0_0_0_2px_color-mix(in_srgb,var(--ring)_25%,transparent)]";

export function EditorInput({
  className,
  ...props
}: ComponentProps<typeof Input>) {
  return <Input className={cn(editorControl, className)} {...props} />;
}
export function EditorSelect({
  className,
  ...props
}: ComponentProps<typeof Select>) {
  return (
    <Select className={cn(editorControl, "w-full", className)} {...props} />
  );
}
