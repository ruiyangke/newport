import type { ComponentProps } from "react";
import { cn } from "../lib/utils";

export function SectionEmpty({
  as: Tag = "p",
  className,
  ...props
}: ComponentProps<"p"> & { as?: "p" | "div" }) {
  return (
    <Tag
      data-slot="section-empty"
      className={cn(
        "rounded-[8px] bg-sidebar px-[18px] py-[30px] text-[12px] leading-[1.6] text-muted-foreground [&_small]:mt-[4px] [&_small]:block",
        className,
      )}
      {...props}
    />
  );
}
export function WorkspaceBanner({
  error,
  className,
  ...props
}: ComponentProps<"div"> & { error?: boolean }) {
  return (
    <div
      data-slot="banner"
      className={cn(
        "mx-[28px] mt-[16px] flex items-start gap-[10px] rounded-[8px] border border-border bg-sidebar p-[12px] text-[12px] leading-[1.6] wrap-anywhere [&>span]:flex-1 [&_button]:size-[22px]",
        error && "text-(--red)",
        className,
      )}
      {...props}
    />
  );
}
export function WorkspaceEmpty({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="empty"
      className={cn(
        "flex min-h-[calc(100%-70px)] flex-col items-center justify-center gap-[20px] p-[40px] text-center [&>svg]:text-muted-foreground [&_h1]:text-[26px] [&_p]:max-w-[42ch] [&_p]:text-[13px] [&_p]:leading-[1.7] [&_p]:text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}
