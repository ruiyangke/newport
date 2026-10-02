import type { ComponentProps } from "react";
import { Search } from "lucide-react";
import { Input } from "./controls";
import { cn } from "../lib/utils";

export function SearchField({
  className,
  ...props
}: ComponentProps<typeof Input>) {
  return (
    <div
      className={cn(
        "flex items-center gap-[6px] rounded-lg border border-input bg-background px-[9px] text-muted-foreground focus-within:outline-2 focus-within:outline-offset-1 focus-within:outline-ring",
        className,
      )}
    >
      <Search size={14} aria-hidden="true" />
      <Input
        className="h-[31px] border-0 bg-transparent p-0 text-[12px] [box-shadow:none] focus-visible:[box-shadow:none] dark:bg-transparent"
        {...props}
      />
    </div>
  );
}
