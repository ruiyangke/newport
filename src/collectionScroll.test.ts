// @vitest-environment jsdom
import { expect, it } from "vitest";
import {
  captureCollectionScroll,
  restoreCollectionScroll,
} from "./collectionScroll";
it("anchors scoped service rows and leaves other workspaces alone", () => {
  const workspace = document.createElement("div");
  workspace.className = "workspace-scroll";
  const scope = document.createElement("div");
  const row = document.createElement("div");
  row.dataset.scrollAnchor = "";
  scope.append(row);
  workspace.append(scope);
  document.body.append(workspace);
  let top = 10;
  workspace.getBoundingClientRect = () => ({ top: 0, bottom: 100 }) as DOMRect;
  row.getBoundingClientRect = () => ({ top, bottom: top + 40 }) as DOMRect;
  workspace.scrollTop = 20;
  const saved = captureCollectionScroll(scope);
  top = 30;
  restoreCollectionScroll(saved);
  expect(workspace.scrollTop).toBe(40);
  scope.hidden = true;
  expect(captureCollectionScroll(scope)).toEqual([]);
  workspace.remove();
});
