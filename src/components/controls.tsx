import {
  Select as BaseSelect,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectGroup,
} from "./ui/select";
import type { ComponentProps } from "react";
import { LoaderCircle, RefreshCw } from "lucide-react";
import { Button as BaseButton } from "./ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";

// Keep action semantics and feedback consistent across pages and dialogs.
export function Button({
  className = "",
  title,
  loading = false,
  disabled,
  type = "button",
  children,
  ...props
}: ComponentProps<typeof BaseButton> & { loading?: boolean }) {
  const iconOnly = props.size?.startsWith("icon");
  const help = title ?? (iconOnly ? props["aria-label"] : undefined);
  const button = (
    <BaseButton
      {...props}
      type={type}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      data-loading={loading || undefined}
      variant={props.variant ?? "outline"}
      size={props.size ?? "default"}
      className={className}
    >
      {loading && (
        <LoaderCircle className="button-spinner" aria-hidden="true" />
      )}
      {children}
    </BaseButton>
  );
  return help ? (
    <Tooltip>
      <TooltipTrigger asChild>{button}</TooltipTrigger>
      <TooltipContent>{help}</TooltipContent>
    </Tooltip>
  ) : (
    button
  );
}

export function RefreshButton({
  busy,
  onClick,
  logs = false,
}: {
  busy: boolean;
  onClick: () => void;
  logs?: boolean;
}) {
  return (
    <Button loading={busy} onClick={onClick}>
      <RefreshCw size={14} aria-hidden="true" />
      {busy ? "Refreshing…" : logs ? "Refresh logs" : "Refresh"}
    </Button>
  );
}
export { Input } from "./ui/input";
export function Select({
  value,
  onValueChange,
  children,
  disabled,
  name,
  ...triggerProps
}: Omit<ComponentProps<typeof SelectTrigger>, "onChange" | "value"> & {
  value: string;
  onValueChange: (value: string) => void;
  name?: string;
}) {
  return (
    <BaseSelect
      value={value}
      onValueChange={onValueChange}
      disabled={disabled}
      name={name}
    >
      <SelectTrigger {...triggerProps}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent position="popper" align="start">
        <SelectGroup>{children}</SelectGroup>
      </SelectContent>
    </BaseSelect>
  );
}
export { SelectItem } from "./ui/select";
export { Textarea } from "./ui/textarea";
export { Checkbox } from "./ui/checkbox";
