import { useEffect, useRef } from "react";
import { EditorState } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import { diffTheme } from "./diff/diffExtensions";

/** Read-only, viewport-rendered text. Appending pages preserves scroll position. */
export default function GitBlobEditor({
  text,
  label,
}: {
  text: string;
  label: string;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const previous = useRef("");
  useEffect(() => {
    const editor = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        extensions: [
          EditorState.readOnly.of(true),
          EditorView.editable.of(false),
          lineNumbers(),
          diffTheme,
          EditorView.contentAttributes.of({
            "aria-label": label,
            "aria-readonly": "true",
          }),
        ],
      }),
    });
    view.current = editor;
    previous.current = "";
    return () => {
      view.current = null;
      editor.destroy();
    };
  }, [label]);
  useEffect(() => {
    const editor = view.current;
    if (!editor) return;
    const append = text.startsWith(previous.current);
    let insert = append ? text.slice(previous.current.length) : text;
    // CodeMirror already normalized the previous page's trailing CR. A
    // following LF completes that same newline rather than adding another.
    if (append && previous.current.endsWith("\r") && insert.startsWith("\n"))
      insert = insert.slice(1);
    editor.dispatch({
      changes: {
        from: append ? editor.state.doc.length : 0,
        to: editor.state.doc.length,
        insert,
      },
    });
    previous.current = text;
  }, [text, label]);
  return <div ref={host} />;
}
