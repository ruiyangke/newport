// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, expect, it } from "vitest";
import GitBlobEditor from "./GitBlobEditor";
let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
});
it("preserves CRLF split across pages without adding an extra display line", async () => {
  const render = (text: string) =>
    root.render(<GitBlobEditor text={text} label="Content" />);
  await act(async () => render("one\r"));
  const view = EditorView.findFromDOM(
    host.querySelector<HTMLElement>(".cm-editor")!,
  )!;
  await act(async () => render("one\r\ntwo\r"));
  expect(view.state.doc.toString()).toBe("one\ntwo\n");
  expect(view.state.doc.lines).toBe(3);
  await act(async () => render("one\r\ntwo\r\nthree"));
  expect(view.state.doc.toString()).toBe("one\ntwo\nthree");
  expect(view.state.doc.lines).toBe(3);
  expect(
    EditorView.findFromDOM(host.querySelector<HTMLElement>(".cm-editor")!),
  ).toBe(view);
});
