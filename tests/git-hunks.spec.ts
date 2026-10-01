import { test, expect } from "@playwright/test";
import { mkdirSync } from "node:fs";

const fixture = (options: {
  lines: boolean;
  truncated?: boolean;
  longPath?: boolean;
  longBranch?: boolean;
  longRepo?: boolean;
  connectError?: boolean;
  statusErrorOnRefresh?: boolean;
  longLine?: boolean;
  manyFiles?: boolean;
  branchesError?: boolean;
  // A second bookmark whose repository opens under its own id, so switching
  // between the two is observable.
  twoProjects?: boolean;
}) => {
  {
    const path = (display: string) => ({ display, bytesB64: btoa(display) });
    const oid = { algorithm: "sha1", hex: "a".repeat(40) };
    const state = {
      revision: 0,
      staged: [] as string[],
      actions: [] as any[],
      contexts: [] as number[],
      receipts: [] as any[],
      unknown: false,
      // Every Git request, as "method repoId", in the order the agent saw them.
      requests: [] as string[],
    };
    const head = {
      name: path(
        options.longBranch
          ? "refs/heads/feature/PROJ-2481-rework-the-repository-status-pipeline"
          : "refs/heads/main",
      ),
      oid,
      unborn: false,
      detached: false,
    };
    Object.assign(window, {
      hunkFixture: state,
      isTauri: true,
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: "main" } },
        transformCallback: () => 1,
        invoke: async (cmd: string, args: any) => {
          if (cmd === "snapshot")
            return {
              config: {
                servers: [
                  {
                    id: "server",
                    name: "Development",
                    sshUser: "dev",
                    sshHost: "example.test",
                    sshPort: 22,
                    authMethod: "publicKey",
                  },
                ],
                tunnels: [],
              },
              runtime: { tunnels: {}, clipboard: {}, health: {} },
              loadError: null,
            };
          if (cmd === "get_sidebar_width") return 200;
          if (cmd === "git_projects_list")
            return [
              {
                id: "project",
                serverId: "server",
                name: options.longRepo
                  ? "Newport Workspace Platform — Repository Services"
                  : "Application",
                path: path("/repo"),
              },
              ...(options.twoProjects
                ? [
                    {
                      id: "second",
                      serverId: "server",
                      name: "Second",
                      path: path("/second"),
                    },
                  ]
                : []),
            ];
          if (cmd === "git_pending_operations") return state.receipts;
          if (cmd === "git_acknowledge_operation") {
            state.receipts = state.receipts.filter(
              (r) => r.operationId !== args.operationId,
            );
            return;
          }
          if (cmd === "git_connect" && options.connectError)
            throw new Error("Close an unused Git session first.");
          if (cmd === "git_connect")
            return {
              sessionId: "session",
              serverId: "server",
              info: {
                capabilities: {
                  methods: [
                    "repo.open",
                    "repo.status",
                    "repo.branches",
                    "repo.diff_page",
                    "repo.close",
                    "operation.start",
                    "operation.get",
                  ],
                  actions: ["stage", "unstage"],
                  features: options.lines
                    ? ["index.hunks", "index.lines"]
                    : ["index.hunks"],
                },
              },
            };
          if (cmd !== "git_request") return;
          const { method, params } = args.request;
          state.requests.push(`${method} ${params?.repoId ?? ""}`.trim());
          // The panel's failure above an OPEN repository: the first read lands,
          // a later one fails, so the workspace is showing and the alert is on
          // top of it.
          if (method === "repo.status" && options.statusErrorOnRefresh) {
            state.revision += 1;
            if (state.revision > 1)
              throw new Error("Close an unused Git session first.");
          }
          if (method === "repo.branches") {
            if (options.branchesError)
              throw new Error(
                "Repository read exceeds the configured resource limit.",
              );
            return {
              snapshot: "branches",
              nextCursor: null,
              entries: [
                {
                  reference: path("refs/heads/main"),
                  name: path("main"),
                  oid,
                  current: true,
                  remote: false,
                  upstream: null,
                },
              ],
            };
          }
          if (method === "repo.open")
            return {
              repoId:
                params.path?.display === "/second" ? "repo-second" : "repo",
              commonRepoId: "common",
              root: path("/repo"),
              bare: false,
              objectFormat: "sha1",
              head,
              operationState: "Clean",
              integration: null,
              capabilities: { readOnly: false, workingTree: true },
            };
          if (method === "repo.status")
            return {
              snapshot: `s${state.revision}`,
              entries: [
                {
                  entryId: `file${state.revision}`,
                  path: path("src/app.ts"),
                  oldPath: null,
                  flags: 256,
                  staged: state.staged.length > 0,
                  unstaged: state.staged.length < 2,
                  untracked: false,
                  conflicted: false,
                  conflict: null,
                },
                // A large working tree, to test the claim that the composer
                // stays reachable however many files are listed.
                ...(options.manyFiles
                  ? Array.from({ length: 120 }, (_, i) => ({
                      entryId: `bulk${i}`,
                      path: path(`src/module-${i}/index.ts`),
                      oldPath: null,
                      flags: 256,
                      staged: false,
                      unstaged: true,
                      untracked: false,
                      conflicted: false,
                      conflict: null,
                    }))
                  : []),
                // A path far wider than the 250px column, so the check for
                // silently clipped text has something that actually overflows.
                // Opt-in: every other test's counts and staging assertions are
                // written against a single entry.
                ...(options.longPath
                  ? [
                      {
                        entryId: "long",
                        path: path(
                          "packages/workspace/src/features/repository/components/VeryLongDirectoryName/AnotherNestedDirectory/component-with-a-long-name.tsx",
                        ),
                        oldPath: null,
                        flags: 256,
                        staged: false,
                        unstaged: true,
                        untracked: false,
                        conflicted: false,
                        conflict: null,
                      },
                    ]
                  : []),
              ],
              nextCursor: null,
              metadata: {
                head,
                operationState: "Clean",
                integration: null,
                ahead: null,
                behind: null,
                upstreamRef: null,
                basis: "stored_refs",
                ...(options.truncated
                  ? { truncated: true, totalEntries: 48211, entryLimit: 10000 }
                  : {}),
              },
            };
          if (method === "repo.diff_page") {
            state.contexts.push(params.contextLines);
            const ids = ["a".repeat(64), "b".repeat(64)].filter((id) =>
              params.side === "head_to_index"
                ? state.staged.includes(id)
                : !state.staged.includes(id),
            );
            const response = {
              snapshot: params.snapshot,
              diff: {
                truncated: false,
                readOnly: true,
                files: [
                  {
                    oldPath: path("src/app.ts"),
                    newPath: path("src/app.ts"),
                    oldOid: oid,
                    newOid: oid,
                    oldMode: 33188,
                    newMode: 33188,
                    status: "Modified",
                    binary: false,
                    additions: ids.length,
                    deletions: ids.length,
                    hunks: ids.map((id) => {
                      const start = id.startsWith("a") ? 2 : 32;
                      const removed = (id.startsWith("a") ? "c" : "d").repeat(
                        64,
                      );
                      const added = (id.startsWith("a") ? "e" : "f").repeat(64);
                      return {
                        id,
                        oldStart: start,
                        oldLines: 3,
                        newStart: start,
                        newLines: 3,
                        lines: [
                          {
                            origin: " ",
                            oldLine: start,
                            newLine: start,
                            content: path(
                              options.longLine
                                ? `export const config = { ${Array.from(
                                    { length: 24 },
                                    (_, i) => `optionNumber${i}: "value-${i}"`,
                                  ).join(", ")} };\n`
                                : "export function start() {\n",
                            ),
                          },
                          {
                            ...(options.lines ? { id: removed } : null),
                            origin: "-",
                            oldLine: start + 1,
                            newLine: null,
                            content: path("  const interval = 1000;\n"),
                          },
                          {
                            ...(options.lines ? { id: added } : null),
                            origin: "+",
                            oldLine: null,
                            newLine: start + 1,
                            content: path("  const interval = 500;\n"),
                          },
                          {
                            origin: " ",
                            oldLine: start + 2,
                            newLine: start + 2,
                            content: path("}\n"),
                          },
                        ],
                      };
                    }),
                  },
                ],
              },
            };
            const entries = response.diff.files.map((file, fileIndex) => ({
              ...file,
              fileIndex,
              omissionReason: null,
              hunks: file.hunks.map((hunk, index) => ({
                ...hunk,
                index,
                totalLines: hunk.lines.length,
                lines: hunk.lines.map((line, lineIndex) => ({
                  ...line,
                  lineIndex,
                  byteOffset: 0,
                  lineComplete: true,
                  contentBytesB64: line.content.bytesB64,
                  id:
                    line.origin === "+" || line.origin === "-"
                      ? (line.id ??
                        (line.origin === "+" ? "e" : "c").repeat(64))
                      : null,
                })),
              })),
            }));
            return {
              snapshot: params.snapshot,
              entries,
              nextCursor: null,
              metadata: {
                sourceSnapshot: params.snapshot,
                entryId: params.entryId,
                side: params.side,
                contextLines: params.contextLines,
                readOnly: false,
                hasOmissions: false,
                totalFiles: entries.length,
                totalUnits: entries.reduce(
                  (sum, f) =>
                    sum + f.hunks.reduce((n, h) => n + h.lines.length, 0),
                  0,
                ),
              },
            };
          }
          if (method === "operation.get") {
            // Mirrors the native boundary: the agent journals before executing,
            // so a missing record resolves the receipt instead of blocking.
            const receipt = state.receipts.find(
              (r: any) => r.operationId === params.operationId,
            );
            if (receipt) receipt.state = "rejected";
            throw {
              code: "OPERATION_NOT_FOUND",
              message:
                "No operation record exists. Do not automatically replay a write.",
            };
          }
          if (method === "operation.start") {
            const { action } = params;
            if (
              params.expectedSnapshot !== `s${state.revision}` ||
              action.entryIds[0] !== `file${state.revision}` ||
              action.hunks.contextLines !==
                state.contexts[state.contexts.length - 1] ||
              action.hunks.ids.length !== 1
            )
              throw new Error("Wrong hunk guard");
            state.actions.push(action);
            if (state.unknown) {
              state.receipts.push({
                operationId: params.operationId,
                serverId: "server",
                action: action.kind,
                state: "outcome_unknown",
              });
              throw {
                code: "OUTCOME_UNKNOWN",
                message:
                  "The operation outcome is unknown. Check the saved outcome before retrying.",
              };
            }
            if (action.kind === "stage") state.staged.push(action.hunks.ids[0]);
            else
              state.staged = state.staged.filter(
                (id) => id !== action.hunks.ids[0],
              );
            state.revision++;
            state.receipts.push({
              operationId: params.operationId,
              serverId: "server",
              action: action.kind,
              state: "succeeded",
            });
            return {
              operationId: params.operationId,
              repository: "common",
              payloadHash: "hash",
              seq: 1,
              state: "succeeded",
              result: {},
              error: null,
            };
          }
          throw new Error(`Unexpected ${method}`);
        },
      },
    });
  }
};

test("stages and unstages a selected hunk while preserving the selected file", async ({
  page,
}, info) => {
  await page.addInitScript(fixture, { lines: false });
  await page.goto("/");
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await page
    .getByRole("region", { name: "Unstaged", exact: true })
    .getByRole("button", { name: "src/app.ts", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Stage hunk at line 2", exact: true }),
  ).toBeVisible();
  mkdirSync(".impeccable/screenshots", { recursive: true });
  // The Changes surface carries most of the custom CSS; check it in both themes.
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await page.screenshot({
    path: `.impeccable/screenshots/projects-hunks-dark-${info.project.name}.png`,
    animations: "disabled",
  });
  await page.emulateMedia({ colorScheme: "light" });
  // The single-pane flow only appears when the layout column is genuinely narrow.
  await page.setViewportSize({ width: 700, height: 680 });
  await page.screenshot({
    path: `.impeccable/screenshots/projects-hunks-narrow-${info.project.name}.png`,
    animations: "disabled",
  });
  // The measurements in this session were all taken at 1400x900; capture that
  // width too, so the surface is looked at where it was measured.
  await page.setViewportSize({ width: 1400, height: 900 });
  await expect(page.locator(".git-changes-files")).toBeVisible();
  await page.waitForFunction(
    () =>
      (document.querySelector(".git-changes-files")?.getBoundingClientRect()
        .width ?? 0) > 0,
  );
  await page.screenshot({
    path: `.impeccable/screenshots/projects-hunks-wide-${info.project.name}.png`,
    animations: "disabled",
  });
  // The filter's empty state had no coverage at all. The design puts three
  // things in it — a count that keeps the total in view so a filter cannot look
  // like an empty working tree, a heading over an explanation, and a way out —
  // so all three are asserted, including that the way out actually works.
  await page.setViewportSize({ width: 1400, height: 900 });
  await page
    .getByRole("textbox", { name: "Filter changed files" })
    .fill("zzzznomatch");
  await expect(page.locator(".git-projects-summary")).toHaveText(
    /^1 changed file 0 matching loaded files\./,
  );
  await expect(
    page.getByText("No files match your current filters", { exact: true }),
  ).toBeVisible();
  await expect(
    page.locator(".git-changes-files li > button[aria-pressed]"),
  ).toHaveCount(0);
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-filter-empty-${info.project.name}.png`,
  });
  await page
    .getByRole("button", { name: "Clear filters", exact: true })
    .click();
  await expect(
    page.locator(".git-changes-files li > button[aria-pressed]"),
  ).toHaveCount(1);
  // Unfiltered, the "of" line goes away and the total is on the Changes tab.
  await expect(page.locator(".git-projects-summary")).toHaveCount(0);
  await expect(
    page.getByRole("tab", { name: "Changes", exact: true }),
  ).toHaveText("Changes1");
  // The summary-length hint, at the threshold measured on the prototype: 50
  // characters silent, 51 shows it. Advisory only — it must never change
  // whether the commit button is usable.
  const hint = page.getByText(/Summaries under 50 characters/);
  const summaryField = page.getByRole("textbox", { name: "Commit summary" });
  await summaryField.fill("x".repeat(50));
  await expect(hint).toHaveCount(0);
  await summaryField.fill("x".repeat(51));
  await expect(hint).toBeVisible();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-summary-hint-${info.project.name}.png`,
  });
  await summaryField.fill("");
  await expect(hint).toHaveCount(0);
  // No control may sit on top of another at any width. The toolbar has twice
  // collapsed into items that intercepted each other's clicks, which a
  // screenshot does not catch -- the overlap is only visible to a hit test.
  // The check carries its own positive control: a button is dropped over a real
  // control and the detector must report it, so a clean run cannot mean the
  // detector quietly did nothing.
  for (const width of [1400, 900, 760]) {
    await page.setViewportSize({ width, height: 800 });
    await page.waitForFunction(
      (w) => Math.abs(document.documentElement.clientWidth - w) <= 20,
      width,
    );
    const report = await page.evaluate(() => {
      const scope = document.querySelector(".git-projects")!;
      const items = [
        ...scope.querySelectorAll(
          'button, a[href], input, select, [role="combobox"], [role="tab"]',
        ),
      ].filter((el) => {
        const r = el.getBoundingClientRect();
        return r.width > 0 && r.height > 0;
      });
      const name = (el: Element) =>
        (el.getAttribute("aria-label") || el.textContent || el.tagName)
          .trim()
          .replace(/\s+/g, " ")
          .slice(0, 24) || el.tagName;
      const obscured: string[] = [];
      for (const el of items) {
        const r = el.getBoundingClientRect();
        const hit = document.elementFromPoint(
          r.left + r.width / 2,
          r.top + r.height / 2,
        );
        if (!hit || hit === el || el.contains(hit) || hit.contains(el))
          continue;
        const culprit = hit.closest(
          'button, a[href], input, [role="combobox"], [role="tab"]',
        );
        if (culprit && culprit !== el && !el.contains(culprit)) {
          obscured.push(`${name(el)} under ${name(culprit)}`);
        }
      }
      let control = "DEAD";
      if (items.length > 0) {
        const victim = items[0];
        const r = victim.getBoundingClientRect();
        const spoiler = document.createElement("button");
        spoiler.style.cssText = `position:fixed;left:${r.left}px;top:${r.top}px;width:${r.width}px;height:${r.height}px;z-index:99999`;
        document.body.append(spoiler);
        const hit = document.elementFromPoint(
          r.left + r.width / 2,
          r.top + r.height / 2,
        );
        if (
          hit &&
          hit !== victim &&
          !victim.contains(hit) &&
          hit.closest("button")
        )
          control = "fires";
        spoiler.remove();
      }
      // Text cut off with nothing to say it was cut. The prototype clips its
      // paths; this app deliberately ellipses instead, so a regression here
      // would silently lose the end of a filename. Elements that are meant to
      // scroll are skipped, and screen-reader text is clipped to 1x1 on
      // purpose. Not leaf-only: the change list's path element wraps its
      // directory in a span, so the element doing the clipping has children.
      const clipped: string[] = [];
      for (const el of scope.querySelectorAll("*")) {
        if (!(el.textContent || "").trim()) continue;
        const box = el.getBoundingClientRect();
        if (box.width < 4 || box.height < 4) continue;
        const cs = getComputedStyle(el);
        const x = cs.overflowX;
        if (x === "visible" || x === "auto" || x === "scroll") continue;
        if (cs.textOverflow === "ellipsis") continue;
        if (cs.whiteSpace !== "nowrap" && cs.whiteSpace !== "pre") continue;
        if (el.scrollWidth > el.clientWidth + 1)
          clipped.push(`${name(el)} ${el.scrollWidth}>${el.clientWidth}`);
      }
      const probe = document.createElement("div");
      probe.style.cssText =
        "width:10px;overflow:hidden;white-space:nowrap;text-overflow:clip";
      probe.textContent = "a very long string that cannot possibly fit";
      scope.appendChild(probe);
      const clipControl = probe.scrollWidth > probe.clientWidth + 1;
      probe.remove();
      return {
        control,
        obscured,
        checked: items.length,
        clipped,
        clipControl,
      };
    });
    expect(report.clipControl).toBe(true);
    expect(report.clipped).toEqual([]);
    expect(report.control).toBe("fires");
    expect(report.checked).toBeGreaterThan(8);
    expect(report.obscured).toEqual([]);
  }
  await page.setViewportSize({ width: 960, height: 680 });
  await expect(page.locator(".git-changes-files")).toBeVisible();
  await page.screenshot({
    path: `.impeccable/screenshots/projects-hunks-${info.project.name}.png`,
    animations: "disabled",
  });
  // The grouped actions menu is the only way to reach the repository dialogs.
  await page.getByRole("button", { name: "Git actions", exact: true }).click();
  await page.screenshot({
    path: `.impeccable/screenshots/projects-git-actions-${info.project.name}.png`,
    animations: "disabled",
  });
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "Stage hunk at line 32", exact: true })
    .focus();
  await page.keyboard.press("Enter");
  await expect(page.getByText("Hunk staged.", { exact: true })).toBeVisible();
  // A write keeps the comparison the user chose rather than switching it, so
  // the staged hunk simply leaves the unstaged side.
  await expect(
    page.getByRole("button", { name: "Stage hunk at line 32", exact: true }),
  ).toHaveCount(0);
  await expect(
    page
      .getByRole("region", { name: "Unstaged", exact: true })
      .getByRole("button", { name: "src/app.ts", exact: true }),
  ).toHaveAttribute("aria-pressed", "true");
  await page.screenshot({
    path: `.impeccable/screenshots/projects-hunks-staged-${info.project.name}.png`,
    animations: "disabled",
  });
  // The same file now also appears under Staged; opening it there shows the
  // other comparison, where the hunk can be unstaged.
  await page
    .getByRole("region", { name: "Staged", exact: true })
    .getByRole("button", { name: "src/app.ts", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Unstage hunk at line 32", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Stage hunk at line 2", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Stage hunk at line 32", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() =>
      (window as any).hunkFixture.actions.map((action: any) => [
        action.kind,
        action.hunks.ids[0],
      ]),
    ),
  ).toEqual([
    ["stage", "b".repeat(64)],
    ["unstage", "b".repeat(64)],
  ]);
  await page.evaluate(() => {
    (window as any).hunkFixture.unknown = true;
  });
  await page
    .getByRole("button", { name: "Stage hunk at line 2", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Stage hunk at line 32", exact: true }),
  ).toBeEnabled();
  await expect(
    page.getByRole("region", { name: "Git operation recovery", exact: true }),
  ).toBeVisible();
  // An operation the agent never journaled must not block writing forever.
  await page
    .getByRole("button", { name: "Check outcome", exact: true })
    .click();
  await expect(
    page.getByText("The operation never started on the server", {
      exact: false,
    }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Dismiss outcome", exact: true })
    .click();
  await expect(
    page.getByRole("region", { name: "Git operation recovery", exact: true }),
  ).toBeHidden();
  await expect(
    page.getByRole("button", { name: "Stage hunk at line 32", exact: true }),
  ).toBeEnabled();
});

test("stages exactly the selected lines and warns about unselected deletions", async ({
  page,
}, info) => {
  await page.addInitScript(fixture, { lines: true });
  await page.goto("/");
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await page
    .getByRole("region", { name: "Unstaged", exact: true })
    .getByRole("button", { name: "src/app.ts", exact: true })
    .click();
  // Narrowing context re-reads the comparison and rebinds every identifier.
  // Context is a diff setting, in the diff header's settings popover.
  await page
    .getByRole("button", { name: "Diff Settings", exact: true })
    .click();
  await page
    .getByRole("combobox", { name: "Context lines", exact: true })
    .click();
  await page
    .getByRole("option", { name: "0 context lines", exact: true })
    .click();
  await expect
    .poll(async () =>
      page.evaluate(() => (window as any).hunkFixture.contexts.at(-1)),
    )
    .toBe(0);
  const added = page.getByRole("checkbox", {
    name: "Stage added line 3",
    exact: true,
  });
  const removed = page.getByRole("checkbox", {
    name: "Stage removed line 3",
    exact: true,
  });
  await added.check();
  await expect(
    page.getByText("1 line selected", { exact: true }),
  ).toBeVisible();
  // Keeping the paired deletion unselected leaves both lines in the file.
  await expect(
    page.getByText("Deletions you did not select stay in the file.", {
      exact: true,
    }),
  ).toBeVisible();
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    path: `.impeccable/screenshots/projects-lines-${info.project.name}.png`,
    animations: "disabled",
  });
  await removed.check();
  await expect(
    page.getByText("Deletions you did not select stay in the file"),
  ).toHaveCount(0);
  await expect(
    page.getByText("2 lines selected", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Stage 2 lines", exact: true })
    .click();
  await expect(page.getByText("Lines staged.", { exact: true })).toBeVisible();
  expect(
    await page.evaluate(() =>
      (window as any).hunkFixture.actions.map((action: any) => [
        action.kind,
        action.hunks.ids,
        action.hunks.lines,
        action.hunks.contextLines,
      ]),
    ),
  ).toEqual([["stage", ["a".repeat(64)], ["e".repeat(64), "c".repeat(64)], 0]]);
});

test("explains a working tree too large to read in one page", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false, truncated: true });
  await page.goto("/");
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  // The count, the cap and the likely cause are all stated.
  const notice = page.getByRole("status").filter({ hasText: "48,211" });
  await expect(notice).toBeVisible();
  await expect(notice).toContainText("10,000");
  await expect(notice).toContainText("not ignored");
  // The view still works rather than failing outright.
  await expect(
    page
      .getByRole("region", { name: "Unstaged", exact: true })
      .getByRole("button", { name: "src/app.ts", exact: true }),
  ).toBeVisible();
});

test("a path too long for its column is ellipsed, not silently cut", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false, longPath: true });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  const row = page
    .getByRole("button", { name: /component-with-a-long-name\.tsx$/ })
    .first();
  await expect(row).toBeVisible();
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-long-path.png",
  });
  const report = await page.evaluate(() => {
    const scope = document.querySelector(".git-projects")!;
    const name = (el: Element) =>
      el.tagName.toLowerCase() +
      (typeof el.className === "string" && el.className
        ? "." + el.className.trim().split(/\s+/)[0]
        : "");
    const clipped: string[] = [];
    let overflowing = 0;
    for (const el of scope.querySelectorAll("*")) {
      if (!(el.textContent || "").trim()) continue;
      const box = el.getBoundingClientRect();
      if (box.width < 4 || box.height < 4) continue;
      const cs = getComputedStyle(el);
      const x = cs.overflowX;
      if (x === "visible" || x === "auto" || x === "scroll") continue;
      if (cs.whiteSpace !== "nowrap" && cs.whiteSpace !== "pre") continue;
      if (el.scrollWidth > el.clientWidth + 1) {
        overflowing += 1;
        if (cs.textOverflow !== "ellipsis")
          clipped.push(`${name(el)} ${el.scrollWidth}>${el.clientWidth}`);
      }
    }
    return { clipped, overflowing };
  });
  // The point of the fixture: something really does overflow here, so a
  // passing `clipped` list means the ellipsis is doing its job rather than
  // that nothing was long enough to test.
  expect(report.overflowing).toBeGreaterThan(0);
  expect(report.clipped).toEqual([]);
  // The whole path stays recoverable on the element that carries it.
  await expect(row.locator(".git-change-path")).toHaveAttribute(
    "title",
    /packages\/workspace/,
  );
  // And the truncation falls on the directory, never on the file's own name:
  // an ellipsis that eats the filename hides exactly what is being looked for.
  const halves = await page.evaluate(() => {
    const path = [...document.querySelectorAll(".git-change-path")].find((el) =>
      (el.textContent || "").includes("component-with-a-long-name.tsx"),
    )!;
    const dir = path.querySelector(".git-change-dir")!;
    const base = path.querySelector(".git-change-base")!;
    return {
      dirTruncated: dir.scrollWidth > dir.clientWidth + 1,
      baseTruncated: base.scrollWidth > base.clientWidth + 1,
      baseText: (base.textContent || "").trim(),
    };
  });
  expect(halves.baseText).toBe("component-with-a-long-name.tsx");
  expect(halves.baseTruncated).toBe(false);
  expect(halves.dirTruncated).toBe(true);
});

test("a long branch name does not push the toolbar controls out of reach", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false, longBranch: true });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await expect(page.getByRole("button", { name: /^Branches:/ })).toBeVisible();
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-long-branch.png",
  });
  const report = await page.evaluate(() => {
    const toolbar = document.querySelector(".git-projects header")!;
    const tr = toolbar.getBoundingClientRect();
    const name = (el: Element) =>
      el.tagName.toLowerCase() +
      (typeof el.className === "string" && el.className
        ? "." + el.className.trim().split(/\s+/)[0]
        : "");
    const outside: string[] = [];
    for (const el of toolbar.querySelectorAll("button, [role='combobox']")) {
      const r = el.getBoundingClientRect();
      if (r.width === 0) continue;
      if (r.right > tr.right + 1 || r.left < tr.left - 1)
        outside.push(
          `${name(el)}[${Math.round(r.left)}..${Math.round(r.right)}]`,
        );
    }
    const picker = document.querySelector('[aria-label^="Branches:"]')!;
    const label = picker.querySelector("strong")!;
    return {
      outside,
      toolbarWidth: Math.round(tr.width),
      pickerWidth: Math.round(picker.getBoundingClientRect().width),
      labelTruncated: label.scrollWidth > label.clientWidth + 1,
      labelEllipsis: getComputedStyle(label).textOverflow === "ellipsis",
      labelText: (label.textContent || "").trim(),
      title: label.getAttribute("title"),
      ariaLabel: picker.getAttribute("aria-label"),
      labelTitle: label.getAttribute("title"),
    };
  });
  // The picker grows with its label until it would crowd the toolbar; what it
  // must never do is carry a control off the end of the bar.
  expect(report.outside).toEqual([]);
  // A label too long for the fixed-width picker is ellipsed rather than cut,
  // and the whole name stays reachable: in the accessible name for assistive
  // technology, and on the title for a pointer. Without the title a sighted
  // user had no way to read a branch the picker had shortened.
  expect(report.labelTruncated).toBe(true);
  expect(report.labelEllipsis).toBe(true);
  expect(report.ariaLabel).toContain("rework-the-repository-status-pipeline");
  expect(report.title).toContain("rework-the-repository-status-pipeline");
});

test("a long repository name stays reachable in the picker", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false, longRepo: true });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page
    .getByRole("button", {
      name: "Newport Workspace Platform — Repository Services",
      exact: true,
    })
    .click();
  const report = await page.evaluate(() => {
    const toolbar = document.querySelector(".git-projects header")!;
    const tr = toolbar.getBoundingClientRect();
    const picker = document.querySelector(
      '[aria-label^="Current repository"]',
    )!;
    const label = picker.querySelector("strong")!;
    const outside: string[] = [];
    for (const el of toolbar.querySelectorAll("button, [role='combobox']")) {
      const r = el.getBoundingClientRect();
      if (r.width === 0) continue;
      if (r.right > tr.right + 1 || r.left < tr.left - 1)
        outside.push(el.className.split(/\s+/)[0]);
    }
    return {
      outside,
      pickerWidth: Math.round(picker.getBoundingClientRect().width),
      labelTruncated: label.scrollWidth > label.clientWidth + 1,
      labelEllipsis: getComputedStyle(label).textOverflow === "ellipsis",
      title: label.getAttribute("title"),
      ariaLabel: picker.getAttribute("aria-label"),
    };
  });
  expect(report.outside).toEqual([]);
  // The title added alongside the branch picker was never exercised with a name
  // long enough to truncate; this is that exercise.
  expect(report.labelTruncated).toBe(true);
  expect(report.labelEllipsis).toBe(true);
  expect(report.title).toContain("Repository Services");
  expect(report.ariaLabel).toContain("Repository Services");
});

test("a panel error lines up with the library it sits above", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false, connectError: true });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  const alert = page.locator(".git-projects > [role='alert']");
  await expect(alert).toBeVisible();
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-panel-error.png",
  });
  const report = await page.evaluate(() => {
    const el = document.querySelector(".git-projects > [role='alert']")!;
    const library = document.querySelector(".git-library")!;
    const heading = library.querySelector("h1")!;
    return {
      alertLeft: Math.round(el.getBoundingClientRect().left),
      alertRight: Math.round(el.getBoundingClientRect().right),
      headingLeft: Math.round(heading.getBoundingClientRect().left),
      libraryRight: Math.round(library.getBoundingClientRect().right),
    };
  });
  // The alert is a sibling of the library, not a child of it, so nothing made
  // it share the library's 30px inset: it hung to the left of the very heading
  // it sits above. Aligned on both edges now.
  expect(report.alertLeft).toBe(report.headingLeft);
  expect(report.alertRight).toBeLessThanOrEqual(report.libraryRight);
});

test("a panel error lines up with the workspace it sits above", async ({
  page,
}) => {
  await page.addInitScript(fixture, {
    lines: false,
    statusErrorOnRefresh: true,
  });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  const alert = page.locator(".git-projects > [role='alert']");
  await expect(alert).toBeVisible();
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-workspace-error.png",
  });
  const report = await page.evaluate(() => {
    const el = document.querySelector(".git-projects > [role='alert']")!;
    const toolbar = document.querySelector(".git-projects header")!;
    const picker = toolbar.querySelector('[aria-label^="Current repository"]')!;
    return {
      alertLeft: Math.round(el.getBoundingClientRect().left),
      alertRight: Math.round(el.getBoundingClientRect().right),
      pickerLeft: Math.round(picker.getBoundingClientRect().left),
      toolbarRight: Math.round(toolbar.getBoundingClientRect().right),
    };
  });
  // Same rule as above the library: the alert lines up with the content it sits
  // over, here the toolbar's own controls, and stays inside its right edge.
  expect(report.alertLeft).toBe(report.pickerLeft);
  expect(report.alertRight).toBeLessThanOrEqual(report.toolbarRight);
});

test("a long project name stays inside its library row", async ({ page }) => {
  await page.addInitScript(fixture, { lines: false, longRepo: true });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await expect(page.locator(".git-library")).toBeVisible();
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-long-name-row.png",
  });
  const report = await page.evaluate(() => {
    const library = document.querySelector(".git-library")!;
    const lr = library.getBoundingClientRect();
    const identity = library.querySelector(".git-library-identity")!;
    const name =
      identity.querySelector("strong") ?? identity.firstElementChild!;
    const outside: string[] = [];
    for (const el of library.querySelectorAll("*")) {
      const r = el.getBoundingClientRect();
      if (r.width === 0) continue;
      if (r.right > lr.right + 1 || r.left < lr.left - 1)
        outside.push(
          `${el.tagName.toLowerCase()}.${typeof el.className === "string" ? el.className.split(/\s+/)[0] : ""}`,
        );
    }
    const cs = getComputedStyle(name);
    return {
      outside,
      nameText: (name.textContent || "").trim(),
      nameTruncated: name.scrollWidth > name.clientWidth + 1,
      nameEllipsis: cs.textOverflow === "ellipsis",
      nameWhiteSpace: cs.whiteSpace,
    };
  });
  // Nothing may leave the library's box, and if the name is too wide for its
  // column it has to say so rather than being cut.
  expect(report.outside).toEqual([]);
  expect(report.nameText).toContain("Repository Services");
  if (report.nameTruncated) expect(report.nameEllipsis).toBe(true);
  // At 1400 the name simply fits, so the assertion above proves nothing about
  // truncation. Narrow the window until the column really is too small.
  await page.setViewportSize({ width: 620, height: 900 });
  await page.waitForFunction(
    () => Math.abs(document.documentElement.clientWidth - 620) <= 20,
  );
  const narrow = await page.evaluate(() => {
    const library = document.querySelector(".git-library")!;
    const lr = library.getBoundingClientRect();
    const identity = library.querySelector(".git-library-identity")!;
    const name =
      identity.querySelector("strong") ?? identity.firstElementChild!;
    const outside: string[] = [];
    for (const el of library.querySelectorAll("*")) {
      const r = el.getBoundingClientRect();
      if (r.width === 0) continue;
      if (r.right > lr.right + 1 || r.left < lr.left - 1)
        outside.push(
          `${el.tagName.toLowerCase()}.${typeof el.className === "string" ? el.className.split(/\s+/)[0] : ""}`,
        );
    }
    const cs = getComputedStyle(name);
    return {
      outside,
      truncated: name.scrollWidth > name.clientWidth + 1,
      // A block element reports one client rect however many lines it has, so
      // wrapping is measured as height against a single line.
      wraps:
        name.getBoundingClientRect().height > parseFloat(cs.lineHeight) * 1.5,
      height: Math.round(name.getBoundingClientRect().height),
      lineHeight: cs.lineHeight,
      whiteSpace: cs.whiteSpace,
    };
  });
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-long-name-row-narrow.png",
  });
  expect(narrow.outside).toEqual([]);
  // The name WRAPS here rather than truncating, which is the better answer: the
  // row grows and nothing is lost, so there is nothing to recover with a title.
  // Asserted as "not truncated" so that adding `white-space: nowrap` to this
  // column — which would silently cut the name — fails instead of passing.
  expect(narrow.truncated).toBe(false);
  expect(narrow.wraps).toBe(true);
});

test("a source line wider than the diff pane scrolls rather than escaping it", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false, longLine: true });
  await page.goto("/");
  await page.setViewportSize({ width: 1100, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await page
    .getByRole("region", { name: "Unstaged", exact: true })
    .getByRole("button", { name: "src/app.ts", exact: true })
    .click();
  await expect(page.locator(".git-diff-code")).toBeVisible();
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-long-line.png",
  });
  const report = await page.evaluate(() => {
    const code = document.querySelector(".git-diff-code")!;
    const cr = code.getBoundingClientRect();
    const panel = document.querySelector(".git-projects")!;
    const pr = panel.getBoundingClientRect();
    // Content inside a scrolling box is SUPPOSED to be wider than the box —
    // that is what makes it reachable. Only content that escapes with nothing
    // to scroll is a fault, so anything with a scrollable ancestor is skipped.
    const scrollable = (el: Element) => {
      let n: Element | null = el.parentElement;
      while (n && n !== panel.parentElement) {
        const x = getComputedStyle(n).overflowX;
        if (x === "auto" || x === "scroll") return true;
        n = n.parentElement;
      }
      return false;
    };
    const escaped: string[] = [];
    for (const el of panel.querySelectorAll("*")) {
      const r = el.getBoundingClientRect();
      if (r.width === 0) continue;
      if (r.right > pr.right + 1 && !scrollable(el))
        escaped.push(
          `${el.tagName.toLowerCase()}.${typeof el.className === "string" ? el.className.split(/\s+/)[0] : ""}`,
        );
    }
    return {
      escaped,
      // The long line has to live somewhere: either the code area scrolls, or
      // the text wraps. What it must not do is widen the panel.
      codeScrolls: code.scrollWidth > code.clientWidth + 1,
      codeOverflowX: getComputedStyle(code).overflowX,
      codeWidth: Math.round(cr.width),
      panelWidth: Math.round(pr.width),
      docScrolls:
        document.scrollingElement!.scrollWidth >
        document.scrollingElement!.clientWidth + 1,
    };
  });
  expect(report.escaped).toEqual([]);
  expect(report.docScrolls).toBe(false);
  // The line is wider than the pane, so it must be reachable: the code area
  // scrolls. `overflow: hidden` here would clip it with no way to read the
  // rest, which is the regression this guards.
  expect(report.codeScrolls).toBe(true);
  expect(["auto", "scroll"]).toContain(report.codeOverflowX);
});

test("the commit composer stays reachable in a large working tree", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false, manyFiles: true });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await expect(
    page.locator(".git-changes-files li > button[aria-pressed]").first(),
  ).toBeVisible();
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-many-files.png",
  });
  const report = await page.evaluate(() => {
    const composer = document.querySelector(".git-composer")!;
    const summary = [...document.querySelectorAll('[role="tab"]')].find((tab) =>
      tab.textContent?.startsWith("Changes"),
    )!;
    const scroll = document.querySelector(".git-changes-scroll")!;
    const cr = composer.getBoundingClientRect();
    return {
      rows: document.querySelectorAll(
        ".git-changes-files li > button[aria-pressed]",
      ).length,
      countText: (summary.textContent || "").trim(),
      // The composer must be on screen without scrolling anything.
      composerVisible: cr.top >= 0 && cr.bottom <= innerHeight + 1,
      // The list is what scrolls, not the pane.
      listScrolls: scroll.scrollHeight > scroll.clientHeight + 1,
      docScrolls:
        document.scrollingElement!.scrollHeight >
        document.scrollingElement!.clientHeight + 1,
    };
  });
  expect(report.rows).toBe(121);
  // The total rides on the Changes tab; the groups carry their own counts.
  expect(report.countText).toBe("Changes121");
  // The claim in projects.css is that the composer does not end up eighty files
  // away. With a hundred and twenty-one of them, this is that claim measured.
  expect(report.composerVisible).toBe(true);
  expect(report.listScrolls).toBe(true);
  expect(report.docScrolls).toBe(false);
});

test("a long filename yields space to the controls beside it", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false, longPath: true });
  await page.goto("/");
  await page.setViewportSize({ width: 1100, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await page
    .getByRole("button", { name: /component-with-a-long-name\.tsx$/ })
    .first()
    .click();
  await expect(page.locator(".git-diff-toolbar")).toBeVisible();
  await page.emulateMedia({ colorScheme: "light" });
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-diff-header-long-name.png",
  });
  const report = await page.evaluate(() => {
    const escapedFrom = (box: Element) => {
      const br = box.getBoundingClientRect();
      return [...box.querySelectorAll("*")]
        .filter((el) => {
          const r = el.getBoundingClientRect();
          return (
            r.width > 0 && (r.right > br.right + 1 || r.left < br.left - 1)
          );
        })
        .map((el) => el.tagName.toLowerCase());
    };
    // The file is named once, in the diff's own header.
    const header = document.querySelector(".git-diff-toolbar")!;
    const path = header.querySelector<HTMLElement>(".git-change-path")!;
    const dir = path.querySelector(".git-change-dir")!;
    const base = path.querySelector(".git-change-base")!;
    const settings = header.querySelector<HTMLElement>(
      'button[aria-label="Diff Settings"]',
    )!;
    const actions = header.querySelector('[role="group"]')!;
    const detail = document.querySelector(".git-changes-detail")!;
    return {
      escaped: escapedFrom(header),
      dirTruncated: dir.scrollWidth > dir.clientWidth + 1,
      dirEllipsis: getComputedStyle(dir).textOverflow === "ellipsis",
      baseWhole: base.scrollWidth <= base.clientWidth + 1,
      pathHasTitle: path.title.endsWith("component-with-a-long-name.tsx"),
      settingsWidth: Math.round(settings.getBoundingClientRect().width),
      // Every file action keeps a usable width: the path gives way, not the
      // controls beside it.
      actionWidths: [...actions.querySelectorAll("button")].map((b) =>
        Math.round(b.getBoundingClientRect().width),
      ),
      named:
        detail.textContent!.split("component-with-a-long-name.tsx").length - 1,
    };
  });
  expect(report.escaped).toEqual([]);
  // The directory shrinks to an ellipsis rather than claiming the room the
  // controls need; the file's own name stays whole; the path stays whole in
  // its title.
  expect(report.dirTruncated).toBe(true);
  expect(report.dirEllipsis).toBe(true);
  expect(report.baseWhole).toBe(true);
  expect(report.pathHasTitle).toBe(true);
  expect(report.settingsWidth).toBe(28);
  expect(report.actionWidths.length).toBeGreaterThan(1);
  for (const width of report.actionWidths)
    expect(width).toBeGreaterThanOrEqual(28);
  // And it is named once.
  expect(report.named).toBe(1);
});

test("a failed branch read stops claiming it is still loading", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false, branchesError: true });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await page.getByRole("button", { name: /^Branches:/ }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("alert")).toContainText("resource limit");
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-branches-read-error.png",
  });
  // A dialog that reports a failure and goes on saying "Loading branches…" is
  // telling the reader two contradictory things and never resolves either.
  await expect(dialog.getByText("Loading branches…")).toHaveCount(0);
});

test("the Changes/History tablist keeps its measured shape in both themes", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await expect(page.getByRole("tab", { name: "Changes" })).toBeVisible();

  // The tabs head the list column: a segmented control as wide as the column
  // less its 8px insets, on a quiet track, with the active segment lifted.
  // The shape is asserted rather than eyeballed because it lives in utilities
  // on the shadcn `tabs` primitive, where a global group marker once stacked
  // it into a column without anything looking obviously broken in code.
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme, reducedMotion: "reduce" });
    // Wait for something that genuinely inverts: the body's own ink.
    await page.waitForFunction((want) => {
      const c = document.createElement("canvas");
      const x = c.getContext("2d")!;
      x.fillStyle = "#000";
      x.fillRect(0, 0, 1, 1);
      x.fillStyle = getComputedStyle(document.body).color;
      x.fillRect(0, 0, 1, 1);
      const light = x.getImageData(0, 0, 1, 1).data[0] > 180;
      return want === "dark" ? light : !light;
    }, scheme);

    const shape = await page.evaluate(() => {
      const list = document.querySelector(
        '.git-repository-tabs [data-slot="tabs-list"]',
      )!;
      const bar = list.parentElement!;
      const column = document.querySelector(".git-changes-files")!;
      const active = list.querySelector('[data-state="active"]')!;
      const inactive = list.querySelector('[data-state="inactive"]')!;
      const lr = list.getBoundingClientRect();
      const ar = active.getBoundingClientRect();
      const br = bar.getBoundingClientRect();
      const cr = column.getBoundingClientRect();
      const ac = getComputedStyle(active);
      const detailHeader = document.querySelector(".git-changes-detail")!;
      return {
        list: `${Math.round(lr.width)}x${Math.round(lr.height)}`,
        column: Math.round(cr.width),
        activeHeight: Math.round(ar.height),
        halves: Math.abs(ar.width - inactive.getBoundingClientRect().width),
        radius: ac.borderRadius,
        lifted: ac.backgroundColor !== getComputedStyle(list).backgroundColor,
        // The bar heads the list column only, and owns the rule under it.
        barWidth: Math.round(br.width),
        barHeight: Math.round(br.height),
        barBorder: getComputedStyle(bar).borderBottomWidth,
        // Its rule lines up with the detail pane's header rule beside it.
        detailTop: Math.round(detailHeader.getBoundingClientRect().top),
        barTop: Math.round(br.top),
        direction: getComputedStyle(list).flexDirection,
      };
    });
    expect(shape.list, scheme).toBe(`${shape.column - 16}x30`);
    expect(shape.direction, scheme).toBe("row");
    expect(shape.activeHeight, scheme).toBe(26);
    expect(shape.halves, scheme).toBeLessThanOrEqual(1);
    expect(shape.radius, scheme).toBe("5px");
    expect(shape.lifted, scheme).toBe(true);
    expect(shape.barWidth, scheme).toBe(shape.column);
    expect(shape.barHeight, scheme).toBe(46);
    expect(shape.barBorder, scheme).toBe("1px");
    expect(shape.barTop, scheme).toBe(shape.detailTop);
    mkdirSync(".impeccable/screenshots", { recursive: true });
    await page.screenshot({
      animations: "disabled",
      path: `.impeccable/screenshots/projects-review-nav-${scheme}.png`,
    });
  }
  await page.emulateMedia({ colorScheme: "light" });
});

test("the change filter reads as one joined control", async ({ page }) => {
  await page.addInitScript(fixture, { lines: false });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await expect(
    page.getByRole("textbox", { name: "Filter changed files" }),
  ).toBeVisible();

  const shape = await page.evaluate(() => {
    const row = document.querySelector(".git-changes-toolbar")!;
    const group = row.querySelector('[data-slot="input-group"]')!;
    const button = row.querySelector("button")!;
    const input = row.querySelector("input")!;
    const rr = row.getBoundingClientRect();
    const gr = group.getBoundingClientRect();
    const br = button.getBoundingClientRect();
    const ir = input.getBoundingClientRect();
    return {
      group: `${Math.round(gr.width)}x${Math.round(gr.height)}`,
      inset: Math.round(rr.width - gr.width),
      // The options button sits inside the field's own border, so the two are
      // one control; and after the text, so neither covers the other.
      inside:
        br.left >= gr.left &&
        br.right <= gr.right &&
        br.top >= gr.top &&
        br.bottom <= gr.bottom,
      clear: ir.right <= br.left + 0.5,
      button: `${Math.round(br.width)}x${Math.round(br.height)}`,
      border: getComputedStyle(group).borderTopWidth,
      radius: getComputedStyle(group).borderRadius,
    };
  });
  expect(shape.inset).toBe(20);
  expect(shape.group.endsWith("x28")).toBe(true);
  expect(shape.inside).toBe(true);
  expect(shape.clear).toBe(true);
  expect(shape.button).toBe("22x22");
  expect(shape.border).toBe("1px");
  expect(shape.radius).toBe("6px");

  // The options control narrows to a group the list already builds, so the
  // count has to follow it -- a filter that silently left the total alone would
  // read as a working tree that had changed.
  await page.getByRole("button", { name: "Filter options" }).click();
  // "Staged only" is a substring of "Unstaged only", so this has to be exact.
  await page
    .getByRole("menuitemradio", { name: "Staged only", exact: true })
    .click();
  await expect(page.locator(".git-projects-summary")).toHaveText(
    /^1 changed file 0 matching loaded files\./,
  );
  await expect(page.getByRole("heading", { name: /Unstaged/ })).toHaveCount(0);

  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.getByRole("button", { name: "Filter options" }).click();
  await page.getByRole("menuitemradio", { name: "All changes" }).click();
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-change-filter.png",
  });
});

test("switching repositories holds nothing open and reads the new one by its own id", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false, twoProjects: true });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Application" }),
  ).toBeAttached();

  await page
    .getByRole("button", { name: "Current repository: Application" })
    .click();
  await page.getByRole("menuitem", { name: "Second", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Second" })).toBeAttached();

  const requests: string[] = await page.evaluate(
    () =>
      (window as unknown as { hunkFixture: { requests: string[] } }).hunkFixture
        .requests,
  );
  // The agent is stateless: it holds no handle for the repository the page
  // was showing, so switching has nothing to release and sends no close.
  expect(requests.filter((each) => each.startsWith("repo.close"))).toHaveLength(
    0,
  );
  // The new repository is opened and read through its own id.
  expect(requests.filter((each) => each === "repo.open")).toHaveLength(2);
  const secondOpen = requests.lastIndexOf("repo.open");
  expect(requests.slice(secondOpen + 1)).toContain("repo.status repo-second");
  expect(requests.slice(secondOpen + 1)).not.toContain("repo.status repo");
});

test("a file selected in one repository is not shown in another", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false, twoProjects: true });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await page
    .getByRole("region", { name: "Unstaged", exact: true })
    .getByRole("button", { name: "src/app.ts", exact: true })
    .click();
  await expect(page.locator(".git-diff-toolbar")).toBeVisible();

  // Both repositories in this fixture report the same file under the same
  // entry id, so a selection that leaked across would find a match and draw
  // Application's choice inside Second.
  await page
    .getByRole("button", { name: "Current repository: Application" })
    .click();
  await page.getByRole("menuitem", { name: "Second", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Second" })).toBeAttached();
  await expect(
    page.getByText("Select a file to review its changes."),
  ).toBeVisible();
  await expect(page.locator(".git-diff-toolbar")).toHaveCount(0);
});

test("leaving Projects and coming back keeps the repository open", async ({
  page,
}) => {
  await page.addInitScript(fixture, { lines: false });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Application" }),
  ).toBeAttached();
  const count = (method: string) =>
    page.evaluate(
      (method) =>
        (
          window as unknown as { hunkFixture: { requests: string[] } }
        ).hunkFixture.requests.filter((each) => each.startsWith(method)).length,
      method,
    );
  expect(await count("repo.open")).toBe(1);

  // The rest of the workspace survives navigation; the open repository now
  // does too. It is the same session and the same handle, so coming back does
  // not open the repository again -- and nothing was closed on the way out.
  await page.getByRole("tab", { name: "Overview", exact: true }).click();
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Application" }),
  ).toBeAttached();
  await expect(
    page.getByRole("region", { name: "Unstaged", exact: true }),
  ).toBeVisible();
  expect(await count("repo.open")).toBe(1);
  expect(await count("repo.close")).toBe(0);
});

test("split view draws the old and new sides level", async ({ page }) => {
  await page.addInitScript(fixture, { lines: true });
  await page.goto("/");
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await page.getByRole("button", { name: "Application", exact: true }).click();
  await page
    .getByRole("region", { name: "Unstaged", exact: true })
    .getByRole("button", { name: "src/app.ts", exact: true })
    .click();
  await expect(page.locator(".cm-editor").first()).toBeVisible();
  // The display choice lives in the diff header's settings, beside the name
  // of the file being shown, together with how much context to read.
  await expect(page.locator(".git-diff-toolbar")).toContainText("src/app.ts");
  await page
    .getByRole("button", { name: "Diff Settings", exact: true })
    .click();
  await expect(
    page.getByRole("radio", { name: "Unified", exact: true }),
  ).toBeChecked();
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-diff-settings.png",
  });
  const pop = await page.evaluate(() => {
    const pop = document.querySelector('[data-slot="popover-content"]')!;
    const pr = pop.getBoundingClientRect();
    const box = (e: Element | null) => {
      if (!e) return null;
      const r = e.getBoundingClientRect();
      return { y: Math.round(r.top - pr.top), h: Math.round(r.height) };
    };
    return {
      w: Math.round(pr.width),
      pad: getComputedStyle(pop).padding,
      h3: box(pop.querySelector("h3")),
      legend: box(pop.querySelector("legend")),
      context: !!pop.querySelector('[aria-label="Context lines"]'),
    };
  });
  expect(pop.w).toBe(220);
  expect(pop.pad).toBe("12px");
  // The heading keeps its gap: the unlayered bare `h3 { margin: 0 }` erased
  // it once, and the grid gap is what holds it now.
  expect(pop.legend!.y - (pop.h3!.y + pop.h3!.h)).toBeGreaterThanOrEqual(10);
  expect(pop.context).toBe(true);
  await page.getByRole("radio", { name: "Split", exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(page.locator(".cm-editor")).toHaveCount(2);

  const sides = await page.evaluate(() =>
    [...document.querySelectorAll<HTMLElement>(".cm-editor")].map((editor) => {
      const r = editor.getBoundingClientRect();
      return {
        left: Math.round(r.left),
        width: Math.round(r.width),
        // The rows the side draws, spacers included: the two must agree or
        // a line would sit beside the wrong line on the other side.
        height: Math.round(
          editor.querySelector(".cm-content")!.getBoundingClientRect().height,
        ),
      };
    }),
  );
  expect(sides).toHaveLength(2);
  expect(sides[0].height).toBe(sides[1].height);
  expect(Math.abs(sides[0].width - sides[1].width)).toBeLessThanOrEqual(1);
  expect(sides[1].left).toBeGreaterThan(sides[0].left);
  // Staging still works from split: the controls stay reachable by role.
  await expect(page.getByRole("checkbox").first()).toBeAttached();
  mkdirSync(".impeccable/screenshots", { recursive: true });
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-diff-split.png",
  });
  // The diff's dark palette was chosen, not measured -- the prototype was
  // measured light -- so it is captured to be looked at.
  await page.emulateMedia({ colorScheme: "dark" });
  await page.waitForFunction(() =>
    document.documentElement.classList.contains("dark"),
  );
  await page.screenshot({
    animations: "disabled",
    path: ".impeccable/screenshots/projects-diff-split-dark.png",
  });
  await page.emulateMedia({ colorScheme: "light" });
});
