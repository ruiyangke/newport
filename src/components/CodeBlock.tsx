import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { Button } from "./controls";

export function CodeBlock({ code, label }: { code: string; label: string }) {
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState(false);
  return (
    <div className="setup-code-block">
      <div className="setup-code-heading">
        <span>{label}</span>
        <Button
          variant="ghost"
          onClick={async () => {
            try {
              await navigator.clipboard.writeText(code);
              setCopied(true);
              setError(false);
            } catch {
              setError(true);
            }
          }}
        >
          {copied ? <Check size={14} /> : <Copy size={14} />}
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
      <pre tabIndex={0} aria-label={label}>
        <code>{code}</code>
      </pre>
      {error && (
        <p role="alert">Couldn’t copy. Select and copy the command above.</p>
      )}
    </div>
  );
}
