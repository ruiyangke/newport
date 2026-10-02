import { cn } from "../lib/utils";
import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { Button } from "./controls";

export function CodeBlock(props: {
  code: string;
  label: string;
  className?: string;
}) {
  return <CopyableCode key={props.code} {...props} />;
}
function CopyableCode({
  code,
  label,
  className,
}: {
  code: string;
  label: string;
  className?: string;
}) {
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState(false);
  return (
    <div
      data-slot="code-block"
      className={cn(
        "my-[10px] overflow-hidden rounded-[6px] border border-border",
        className,
      )}
    >
      <div className="flex items-center justify-between bg-(--native-toolbar) px-[10px] py-[4px] text-[11px] text-muted-foreground">
        <span>{label}</span>
        <Button
          variant="ghost"
          onClick={async () => {
            try {
              await navigator.clipboard.writeText(code);
              setCopied(true);
              setError(false);
            } catch {
              setCopied(false);
              setError(true);
            }
          }}
        >
          {copied ? <Check size={14} /> : <Copy size={14} />}
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
      <pre
        className="m-0 overflow-x-auto p-[12px] text-[12px] select-text"
        tabIndex={0}
        aria-label={label}
      >
        <code>{code}</code>
      </pre>
      {error && (
        <p className="px-[12px] text-[12px]" role="alert">
          Couldn’t copy. Select and copy the command above.
        </p>
      )}
    </div>
  );
}
