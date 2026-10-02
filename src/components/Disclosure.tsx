import type { ComponentProps, ReactNode } from "react";
import { cn } from "../lib/utils";

export function Disclosure({
  title,
  className,
  children,
  ...props
}: Omit<ComponentProps<"details">, "title"> & { title: ReactNode }) {
  return (
    <details
      data-slot="system-details"
      className={cn("mt-[10px] text-[11px]", className)}
      {...props}
    >
      <summary className="w-fit cursor-pointer text-muted-foreground hover:text-foreground focus-visible:rounded-[2px] focus-visible:outline-2 focus-visible:outline-offset-[4px] focus-visible:outline-ring">
        {title}
      </summary>
      {children}
    </details>
  );
}
