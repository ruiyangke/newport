import { useEffect, useImperativeHandle, useRef, type Ref } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { terminalLogText } from "../git/logTerminal";
import "@xterm/xterm/css/xterm.css";

export type GitLogTerminalHandle = { text: () => string };
export function GitLogTerminal({
  text,
  follow,
  ref,
}: {
  text: string;
  follow: boolean;
  ref?: Ref<GitLogTerminalHandle>;
}) {
  const host = useRef<HTMLDivElement>(null);
  const instance = useRef<Terminal | null>(null);
  useImperativeHandle(
    ref,
    () => ({
      text: () => {
        const buffer = instance.current?.buffer.active;
        if (!buffer) return "";
        let output = "";
        for (let index = 0; index < buffer.length; index++) {
          const line = buffer.getLine(index);
          if (index && !line?.isWrapped) output += "\n";
          output +=
            line?.translateToString(!buffer.getLine(index + 1)?.isWrapped) ??
            "";
        }
        return output.trimEnd();
      },
    }),
    [],
  );
  useEffect(() => {
    const element = host.current!;
    const term = new Terminal({
      disableStdin: true,
      convertEol: true,
      cursorBlink: false,
      cursorInactiveStyle: "none",
      fontSize: 11,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      lineHeight: 1.4,
      scrollback: 20000,
      screenReaderMode: true,
      allowProposedApi: false,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(element);
    instance.current = term;
    const resize = () => {
      if (element.clientWidth && element.clientHeight) fit.fit();
    };
    const theme = () => {
      const dark = document.documentElement.classList.contains("dark");
      term.options.theme = dark
        ? {
            background: "#1e1f21",
            foreground: "#eeeeef",
            cursor: "#eeeeef",
            selectionBackground: "#454349",
          }
        : {
            background: "#ffffff",
            foreground: "#25262a",
            cursor: "#25262a",
            selectionBackground: "#dad9dd",
          };
    };
    theme();
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(element);
    const appearance = new MutationObserver(theme);
    appearance.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class", "style", "data-theme"],
    });
    return () => {
      observer.disconnect();
      appearance.disconnect();
      instance.current = null;
      term.dispose();
    };
  }, []);
  useEffect(() => {
    const term = instance.current;
    if (!term) return;
    const position = term.buffer.active.viewportY;
    let disposed = false;
    // Queue reset with the text so rapid event batches cannot interleave resets.
    term.write("\x1bc" + terminalLogText(text), () => {
      if (!disposed) {
        if (follow) term.scrollToBottom();
        else term.scrollToLine(position);
      }
    });
    return () => {
      disposed = true;
    };
  }, [text, follow]);
  return (
    <div
      ref={host}
      aria-label="Command output"
      className="min-h-0 flex-1 overflow-hidden bg-background text-foreground"
    />
  );
}
