import type { ComponentProps } from "react";
import { Switch as SwitchPrimitive } from "radix-ui";
import { cn } from "../../lib/utils";

export function Switch({
  className,
  ...props
}: ComponentProps<typeof SwitchPrimitive.Root>) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn(
        "h-[22px] w-[36px] shrink-0 cursor-pointer rounded-full bg-border p-[2px] outline-none focus-visible:outline-2 focus-visible:outline-offset-3 focus-visible:outline-ring disabled:cursor-wait disabled:opacity-50 data-[state=checked]:bg-primary",
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className="block size-[18px] rounded-full bg-white shadow-[0_1px_3px_#0002] transition-transform duration-[120ms] ease-out data-[state=checked]:translate-x-[14px] motion-reduce:transition-none"
      />
    </SwitchPrimitive.Root>
  );
}
