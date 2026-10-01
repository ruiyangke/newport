import { test, expect, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import {
  expectProjectStyles,
  recordProjectStyles,
} from "./support/projectStyles";

/** Creation actions moved into the library's "Add project" menu. */
async function libraryAction(page: Page, item: string) {
  await page.getByRole("button", { name: "Add project", exact: true }).click();
  await page.getByRole("menuitem", { name: item, exact: true }).click();
}

/** The repository picker replaced the back control; "All projects" returns. */
async function backToProjects(page: Page) {
  await page.getByRole("button", { name: /^Current repository:/ }).click();
  await page
    .getByRole("menuitem", { name: "All projects", exact: true })
    .click();
}

/** The four repository dialogs open from the grouped Git actions menu. */
async function openGitAction(page: Page, item: string) {
  await page.getByRole("button", { name: "Git actions", exact: true }).click();
  await page.getByRole("menuitem", { name: item, exact: true }).click();
}

test("saved projects open live reads, rename and remove bookmarks", async ({
  page,
}, info) => {
  test.setTimeout(90_000);
  const loadAllTags = async () => {
    const more = page.getByRole("button", {
      name: "Load more tags",
      exact: true,
    });
    const end = page.getByText("All tags loaded", { exact: true });
    await expect(more.or(end)).toBeVisible();
    while (await more.count()) await more.click();
    await expect(end).toBeVisible();
  };
  const styles = recordProjectStyles(page);
  await page.addInitScript(() => {
    const path = (display: string) => ({ display, bytesB64: btoa(display) });
    const oid = (char: string) => ({ algorithm: "sha1", hex: char.repeat(40) });
    const commit = (root = false) => ({
      oid: oid(root ? "d" : "a"),
      parents: root ? [] : [oid("b"), oid("c")],
      message: path(root ? "Initial commit" : "Merge feature"),
      messageTruncated: false,
      author: { name: "Developer", email: "dev@example.com" },
      time: 1789983160,
      offsetMinutes: 0,
    });
    const head = {
      name: path("refs/heads/main"),
      oid: null,
      unborn: true,
      detached: false,
    };
    let remoteReferences = [
      {
        reference: path("HEAD"),
        kind: "head",
        oid: oid("a"),
        symbolicTarget: path("refs/heads/old"),
      },
      {
        reference: path("refs/heads/old"),
        kind: "branch",
        oid: oid("b"),
        symbolicTarget: null,
      },
      {
        reference: path("refs/tags/old-tag"),
        kind: "tag",
        oid: oid("e"),
        symbolicTarget: null,
      },
      {
        reference: path("refs/tags/old-tag^{}"),
        kind: "peeled_tag",
        oid: oid("f"),
        symbolicTarget: null,
      },
    ];
    let removalAttempts = 0;
    let staged = true;
    let integration: any = null;
    let conflict = false;
    let branchNames = ["main"];
    const upstreams: Record<string, string | null> = {};
    let stashes: any[] = [];
    let tags: any[] = [];
    let stashSequence = 0;
    let remotes: { name: string; url: string; pushUrl: null; token: string }[] =
      [];
    let dropStageReply = true;
    let committed = false;
    let revision = 0;
    let receipts: any[] = [];
    const operations: Record<string, any> = {};

    let projects = [
      {
        id: "project",
        serverId: "server",
        name: "Web application",
        path: path("/srv/web-app"),
      },
    ];
    Object.assign(window, {
      isTauri: true,
      __TAURI_INTERNALS__: {
        metadata: { currentWindow: { label: "main" } },
        invoke: async (cmd: string, args: any) => {
          if (cmd === "snapshot")
            return {
              config: {
                servers: [
                  {
                    id: "server",
                    name: "Development",
                    sshUser: "dev",
                    sshHost: "dev.example.com",
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
          // The folder chooser browses the server, so the library's "Add
          // existing" flow needs a directory listing like any Files read.
          if (cmd === "files_cancel") return;
          if (cmd === "files_list") {
            const at = args.path === "." ? "/srv" : args.path;
            const entry = (name: string, kind = "directory") => ({
              name,
              path: at === "/" ? `/${name}` : `${at}/${name}`,
              kind,
              size: kind === "directory" ? null : 2048,
              modified: 1789983160,
              permissions: "755",
            });
            if (at === "/srv")
              return {
                path: at,
                entries: [
                  entry("web-app"),
                  entry("api-service"),
                  entry(".cache"),
                  entry("README", "file"),
                ],
              };
            return { path: at, entries: [entry("src"), entry("docs")] };
          }
          if (cmd === "git_projects_list") return projects;
          if (cmd === "git_projects_save") {
            projects = [
              ...projects.filter((p) => p.id !== args.project.id),
              args.project,
            ];
            return args.project;
          }
          if (cmd === "git_projects_remove") {
            if (removalAttempts++ === 0)
              throw new Error("Could not save project bookmarks. Try again.");
            projects = projects.filter((p) => p.id !== args.projectId);
            return;
          }
          if (cmd === "git_pending_operations") return receipts;
          if (cmd === "git_acknowledge_operation") {
            receipts = receipts.filter(
              (receipt) => receipt.operationId !== args.operationId,
            );
            return;
          }
          if (cmd === "git_connect")
            return {
              sessionId: "session",
              serverId: "server",
              info: {
                capabilities: {
                  features: [
                    "status_summary.path",
                    "status.filter",
                    "stash.entry_index",
                    "branches.filter",
                    "worktrees.filter",
                    "worktrees.snapshot_filter",
                  ],
                  actions: [
                    "stage",
                    "unstage",
                    "commit",
                    "conflict.resolve",
                    "branch.create",
                    "branch.rename",
                    "branch.set_upstream",
                    "branch.delete",
                    "checkout",
                    "remote.add",
                    "remote.rename",
                    "remote.set_url",
                    "remote.remove",
                    "fetch",
                    "pull.fast_forward",
                    "push",
                    "push.with_lease",
                    "merge",
                    "rebase",
                    "cherry_pick",
                    "revert",
                    "integration.continue",
                    "integration.abort",
                    "integration.skip",
                    "tag.create",
                    "tag.delete",
                    "tag.push",
                    "branch.delete_remote",
                    "tag.delete_remote",
                    "stash.save",
                    "stash.apply",
                    "stash.pop",
                    "stash.drop",
                  ],
                  methods: [
                    "operation.start",
                    "operation.get",
                    "repo.open",
                    "repo.status",
                    "repo.status_summary",
                    "repo.history",
                    "repo.commit",
                    "repo.branches",
                    "repo.worktrees",
                    "repo.remotes",
                    "repo.remote",
                    "repo.remote_names",
                    "repo.remote_refs",
                    "repo.stashes",
                    "repo.tags",
                    "repo.tag",
                    "repo.close",
                    "repo.diff",
                    "repo.diff_page",
                    "repo.commit_files",
                    "repo.commit_diff",
                    "repo.commit_diff_page",
                    "repo.blob",
                    "repo.blob_page",
                  ],
                },
              },
            };
          if (cmd === "git_request") {
            if (args.request.method === "operation.start") {
              const params = args.request.params;
              if (params.expectedSnapshot !== `s${revision}`)
                throw new Error("Stale fixture snapshot");
              const action = params.action;
              if (action.kind === "stage") {
                staged = true;
                conflict = false;
              }
              if (action.kind === "unstage") staged = false;
              if (action.kind === "commit") {
                if (!staged) throw new Error("Nothing staged");
                committed = true;
                Object.assign(head, { oid: oid("a"), unborn: false });
              }
              if (action.kind === "branch.create") {
                if (action.startOid !== oid("a").hex)
                  throw new Error("Wrong branch start");
                branchNames.push(action.name);
              }
              if (action.kind === "branch.set_upstream") {
                if (
                  action.expectedOid !== oid("a").hex ||
                  action.expectedToken !== `tracking${revision}`
                )
                  throw new Error("Wrong upstream guard");
                if (
                  action.upstream !== null &&
                  action.upstream !== "refs/heads/main"
                )
                  throw new Error("Wrong upstream reference");
                upstreams[action.name] = action.upstream;
              }
              if (action.kind === "branch.rename") {
                if (action.expectedOid !== oid("a").hex)
                  throw new Error("Wrong rename guard");
                branchNames = branchNames.map((name) =>
                  name === action.name ? action.newName : name,
                );
              }
              if (action.kind === "branch.delete") {
                if (action.expectedOid !== oid("a").hex || action.force)
                  throw new Error("Unsafe delete");
                branchNames = branchNames.filter(
                  (name) => name !== action.name,
                );
              }
              if (action.kind === "checkout") {
                if (action.target.expectedOid !== oid("a").hex)
                  throw new Error("Wrong checkout guard");
                head.name = path(`refs/heads/${action.target.name}`);
              }
              if (action.kind === "push.with_lease") {
                const row = remoteReferences.find(
                  (row) =>
                    row.reference.display ===
                    `refs/heads/${action.destinationBranch}`,
                );
                if (
                  !row ||
                  action.branch !== "main" ||
                  action.expectedOid !== oid("a").hex ||
                  action.expectedRemoteOid !== row.oid.hex ||
                  !remotes.some(
                    (remote) =>
                      remote.name === action.remote &&
                      remote.token === action.expectedToken,
                  )
                )
                  throw new Error("Wrong lease guards");
                row.oid = oid("a");
              }
              if (
                action.kind === "branch.delete_remote" ||
                action.kind === "tag.delete_remote"
              ) {
                const reference =
                  action.kind === "branch.delete_remote"
                    ? `refs/heads/${action.branch}`
                    : `refs/tags/${action.name}`;
                const row = remoteReferences.find(
                  (row) => row.reference.display === reference,
                );
                if (
                  !row ||
                  row.oid.hex !== action.expectedOid ||
                  !remotes.some(
                    (remote) =>
                      remote.name === action.remote &&
                      remote.token === action.expectedToken,
                  )
                )
                  throw new Error("Wrong remote deletion guard");
                remoteReferences = remoteReferences.filter(
                  (row) =>
                    row.reference.display !== reference &&
                    row.reference.display !== `${reference}^{}`,
                );
              }
              if (action.kind === "remote.add")
                remotes.push({
                  name: action.name,
                  url: action.url,
                  pushUrl: null,
                  token: `remote${revision}`,
                });
              if (
                [
                  "remote.rename",
                  "remote.set_url",
                  "remote.remove",
                  "fetch",
                  "pull.fast_forward",
                  "push",
                ].includes(action.kind)
              ) {
                const remote = remotes.find(
                  (remote) => remote.name === (action.remote ?? action.name),
                );
                if (!remote || action.expectedToken !== remote.token)
                  throw new Error("Wrong remote configuration guard");
                if (action.kind === "remote.rename") {
                  remote.name = action.newName;
                  remote.token = `remote${revision}`;
                }
                if (action.kind === "remote.set_url") {
                  remote.url = action.url;
                  remote.token = `remote${revision}`;
                }
                if (action.kind === "remote.remove")
                  remotes = remotes.filter((item) => item !== remote);
                if (
                  action.kind === "push" &&
                  (action.expectedOid !== oid("a").hex ||
                    action.branch !== "main" ||
                    action.destinationBranch !== "main")
                )
                  throw new Error("Wrong push selection");
                if (
                  action.kind === "pull.fast_forward" &&
                  action.remoteBranch !== "main"
                )
                  throw new Error("Wrong pull selection");
              }
              if (
                ["merge", "rebase", "cherry_pick", "revert"].includes(
                  action.kind,
                )
              ) {
                if (action.kind === "cherry_pick" && action.mainline !== 2)
                  throw new Error("Wrong mainline parent");
                integration = {
                  kind: action.kind,
                  managed: true,
                  canContinue: true,
                  canAbort: true,
                  canSkip: action.kind === "rebase",
                  position: action.kind === "rebase" ? 1 : null,
                  total: action.kind === "rebase" ? 2 : null,
                };
                conflict = true;
                staged = false;
              }
              if (action.kind === "conflict.resolve") {
                if (
                  action.side !== "theirs" ||
                  action.expectedOid !== oid("c").hex
                )
                  throw new Error("Wrong conflict side");
                conflict = false;
                staged = true;
              }
              if (action.kind === "integration.continue") {
                if (conflict) throw new Error("Unresolved conflicts");
                integration = null;
              }
              if (
                action.kind === "integration.abort" ||
                action.kind === "integration.skip"
              ) {
                integration = null;
                conflict = false;
              }
              if (action.kind === "tag.create") {
                if (action.targetOid !== oid("a").hex)
                  throw new Error("Wrong tag target");
                tags.push({
                  name: path(action.name),
                  reference: path(`refs/tags/${action.name}`),
                  oid: oid(action.annotation ? "e" : "a"),
                  symbolicTarget: null,
                  annotated: !!action.annotation,
                  detailsOmitted: false,
                  peeledOid: oid("a"),
                  peeledType: "commit",
                  message: action.annotation
                    ? path(action.annotation.message)
                    : null,
                  messageTruncated: false,
                  tagger: action.annotation
                    ? {
                        name: "Developer",
                        email: "dev@example.com",
                        time: 1789983160,
                        offsetMinutes: 0,
                      }
                    : null,
                });
              }
              if (action.kind === "tag.delete" || action.kind === "tag.push") {
                const tag = tags.find(
                  (tag) => tag.name.display === action.name,
                );
                if (!tag || tag.oid.hex !== action.expectedOid)
                  throw new Error("Wrong exact tag object guard");
                if (action.kind === "tag.delete")
                  tags = tags.filter((item) => item !== tag);
                else if (
                  !remotes.some(
                    (remote) =>
                      remote.name === action.remote &&
                      remote.token === action.expectedToken,
                  )
                )
                  throw new Error("Wrong tag remote guard");
              }
              if (action.kind === "stash.save") {
                if (stashSequence === 0 && !action.includeUntracked)
                  throw new Error("Missing untracked option");
                stashes.unshift({
                  index: stashSequence,
                  oid: oid("e").hex,
                  previousOid: oid("a").hex,
                  message: action.message,
                  messageTruncated: false,
                  time: 1789983160,
                });
                stashSequence++;
                stashes.forEach((stash, index) => {
                  stash.index = index;
                });
                committed = true;
              }
              if (
                ["stash.apply", "stash.pop", "stash.drop"].includes(action.kind)
              ) {
                if (
                  action.expectedToken !== `stashes${revision}` ||
                  !stashes.some(
                    (stash) =>
                      stash.oid === action.oid && stash.index === action.index,
                  )
                )
                  throw new Error("Wrong stash selection");
                if (action.kind !== "stash.drop") {
                  committed = false;
                  staged = true;
                }
                if (action.kind !== "stash.apply")
                  stashes = stashes.filter(
                    (stash) => stash.index !== action.index,
                  );
                stashes.forEach((stash, index) => {
                  stash.index = index;
                });
              }
              revision++;
              const result = {
                operationId: params.operationId,
                repository: "/srv/web-app",
                payloadHash: "fixture",
                state: ["merge", "rebase", "cherry_pick", "revert"].includes(
                  action.kind,
                )
                  ? "needs_resolution"
                  : "succeeded",
                seq: 1,
                result: {},
                error: null,
              };
              operations[params.operationId] = result;
              receipts.push({
                operationId: params.operationId,
                serverId: "server",
                action: action.kind,
                state: result.state,
              });
              if (action.kind === "stage" && dropStageReply) {
                dropStageReply = false;
                receipts[receipts.length - 1].state = "pending";
                throw {
                  code: "TRANSPORT_ERROR",
                  message: "Connection lost after dispatch",
                };
              }
              return result;
            }
            if (args.request.method === "operation.get") {
              const result = operations[args.request.params.operationId];
              receipts = receipts.map((receipt) =>
                receipt.operationId === result.operationId
                  ? { ...receipt, state: result.state }
                  : receipt,
              );
              return result;
            }
            if (args.request.method === "repo.worktrees") {
              const row = {
                kind: "main",
                name: null,
                path: path("/srv/web-app"),
                gitDir: path("/srv/web-app/.git"),
                state: "available",
                current: true,
                head: { ...head },
                locked: false,
                lockReason: null,
                prunable: false,
              };
              const matches =
                !args.request.params.branch ||
                args.request.params.branch === head.name.display;
              return {
                snapshot: `worktrees${revision}`,
                entries: matches ? [row] : [],
                nextCursor: null,
                metadata: {
                  listToken: `trees${revision}`,
                  totalEntries: 1,
                  matchingEntries: matches ? 1 : 0,
                  current: row,
                  main: row,
                },
              };
            }
            if (args.request.method === "repo.tag")
              return tags.find(
                (tag) => tag.oid.hex === args.request.params.oid,
              );
            if (args.request.method === "repo.tags") {
              const offset = args.request.params.cursor ? 1 : 0;
              return {
                snapshot: `tags${revision}`,
                entries: tags.slice(offset, offset + 1),
                metadata: {},
                nextCursor: offset === 0 && tags.length > 1 ? "tag-next" : null,
              };
            }
            if (args.request.method === "repo.stashes")
              return {
                snapshot: `stash-page${revision}`,
                entries: stashes,
                nextCursor: null,
                metadata: { listToken: `stashes${revision}` },
              };
            if (args.request.method === "repo.remote_refs") {
              const p = args.request.params;
              if (!p.forPush) throw new Error("Must inspect push destination");
              const offset = p.cursor ? 2 : 0;
              return {
                snapshot: `remote-refs${revision}`,
                entries: remoteReferences.slice(offset, offset + 2),
                nextCursor:
                  offset === 0 && remoteReferences.length > 2 ? "next" : null,
                metadata: {
                  remote: p.remote,
                  remoteToken: p.expectedToken,
                  forPush: true,
                  basis: "remote_advertisement",
                  truncated: false,
                },
              };
            }
            if (args.request.method === "repo.remote_names") {
              const filter = String(
                args.request.params.filter ?? "",
              ).toLowerCase();
              const entries = remotes
                .filter((r) => r.name.toLowerCase().includes(filter))
                .map((r) => ({ name: r.name }));
              return {
                snapshot: `remotes${revision}:${filter}`,
                entries,
                nextCursor: null,
                metadata: { totalEntries: entries.length },
              };
            }
            if (args.request.method === "repo.remote") {
              const remote = remotes.find(
                (remote) => remote.name === args.request.params.name,
              );
              if (!remote)
                throw { code: "REMOTE_NOT_FOUND", message: "Remote not found" };
              return remote;
            }
            if (args.request.method === "repo.remotes")
              return {
                entries: remotes,
                authentication: { ssh: "server_agent", https: "anonymous" },
              };
            if (args.request.method === "repo.branches")
              return {
                snapshot: `branches${revision}`,
                nextCursor: null,
                metadata: {},
                entries: branchNames
                  .filter(
                    (name) =>
                      args.request.params.branchKind !== "remote" &&
                      name
                        .toLowerCase()
                        .includes(
                          String(
                            args.request.params.filter ?? "",
                          ).toLowerCase(),
                        ),
                  )
                  .map((name) => ({
                    name: path(name),
                    reference: path(`refs/heads/${name}`),
                    oid: oid("a"),
                    remote: false,
                    current: head.name.display === `refs/heads/${name}`,
                    upstream: upstreams[name] ? path(upstreams[name]!) : null,
                    tracking: {
                      token: `tracking${revision}`,
                      editable: true,
                      configuration: { remote: [], merge: [] },
                    },
                  })),
              };
            if (args.request.method === "repo.open")
              return {
                repoId: "repo",
                commonRepoId: "common",
                root: path("/srv/web-app"),
                head,
                bare: false,
                objectFormat: "sha1",
                operationState: "Clean",
                integration,
                capabilities: { readOnly: false, workingTree: true },
              };
            if (
              args.request.method === "repo.status" ||
              args.request.method === "repo.status_summary"
            ) {
              const result = {
                snapshot: `s${revision}`,
                entries:
                  committed && !integration
                    ? []
                    : [
                        {
                          entryId: "file",
                          path: path("src/App.tsx"),
                          oldPath: null,
                          flags: 256,
                          staged: staged && !conflict,
                          unstaged: true,
                          untracked: false,
                          conflicted: conflict,
                          conflict: conflict
                            ? {
                                base: {
                                  oid: oid("a"),
                                  path: path("src/App.tsx"),
                                  mode: 33188,
                                },
                                ours: {
                                  oid: oid("b"),
                                  path: path("src/App.tsx"),
                                  mode: 33188,
                                },
                                theirs: {
                                  oid: oid("c"),
                                  path: path("src/App.tsx"),
                                  mode: 33188,
                                },
                              }
                            : null,
                        },
                      ],
                nextCursor: null,
                metadata: {
                  head,
                  operationState: "Clean",
                  integration,
                  ahead: null,
                  behind: null,
                  basis: "stored_refs",
                  upstreamRef: null,
                },
              };
              if (args.request.method === "repo.status_summary")
                return {
                  ...result.metadata,
                  totalEntries: result.entries.length,
                  truncated: false,
                };
              const { text = "", group = "all" } =
                args.request.params.filter ?? {};
              const entries = result.entries.filter(
                (entry) =>
                  entry.path.display.toLowerCase().includes(text) &&
                  (group === "all" ||
                    (group === "conflicted" && entry.conflicted) ||
                    (group === "staged" && entry.staged && !entry.conflicted) ||
                    (group === "unstaged" &&
                      entry.unstaged &&
                      !entry.conflicted)),
              );
              return {
                ...result,
                entries,
                metadata: {
                  ...result.metadata,
                  totalEntries: result.entries.length,
                  matchedEntries: entries.length,
                  groupCounts: {
                    staged: result.entries.filter((e) => e.staged).length,
                    unstaged: result.entries.filter(
                      (e) => e.unstaged && !e.conflicted,
                    ).length,
                    untracked: 0,
                    conflicted: result.entries.filter((e) => e.conflicted)
                      .length,
                  },
                },
              };
            }
            if (args.request.method === "repo.blob_page")
              return {
                snapshot: "blob",
                nextCursor: null,
                metadata: {
                  oid: { algorithm: "sha1", hex: args.request.params.oid },
                  size: 8,
                },
                entries: [{ offset: 0, bytesB64: btoa("theirs\n\n") }],
              };
            if (args.request.method === "repo.blob")
              return {
                oid: { algorithm: "sha1", hex: args.request.params.oid },
                size: 8,
                truncated: false,
                bytesB64: btoa("theirs\n\n"),
              };
            if (
              args.request.method === "repo.diff" ||
              args.request.method === "repo.diff_page"
            ) {
              const legacy = {
                snapshot: `s${revision}`,
                diff: {
                  truncated: false,
                  readOnly: true,
                  files: [
                    {
                      oldPath: path("src/App.tsx"),
                      newPath: path("src/App.tsx"),
                      oldOid: null,
                      newOid: null,
                      oldMode: 33188,
                      newMode: 33188,
                      status: "Modified",
                      binary: false,
                      additions: 1,
                      deletions: 1,
                      hunks: [
                        {
                          oldStart: 1,
                          oldLines: 1,
                          newStart: 1,
                          newLines: 1,
                          lines: [
                            {
                              origin: "-",
                              oldLine: 1,
                              newLine: null,
                              content: path("previous content\n"),
                            },
                            {
                              origin: "+",
                              oldLine: null,
                              newLine: 1,
                              content: path(
                                args.request.params.side === "head_to_index"
                                  ? "staged update\n"
                                  : "unstaged update\n",
                              ),
                            },
                          ],
                        },
                      ],
                    },
                  ],
                },
              };
              if (args.request.method === "repo.diff") return legacy;
              return {
                snapshot: `diff-${revision}`,
                nextCursor: null,
                metadata: {
                  sourceSnapshot: args.request.params.snapshot,
                  entryId: args.request.params.entryId,
                  side: args.request.params.side,
                  contextLines: args.request.params.contextLines ?? 3,
                  readOnly: false,
                  hasOmissions: false,
                  totalFiles: 1,
                  totalUnits: 2,
                },
                entries: legacy.diff.files.map((f, fileIndex) => ({
                  ...f,
                  fileIndex,
                  omissionReason: null,
                  hunks: f.hunks.map((h, index) => ({
                    ...h,
                    index,
                    id: "a".repeat(64),
                    totalLines: h.lines.length,
                    lines: h.lines.map((l, lineIndex) => ({
                      ...l,
                      lineIndex,
                      id: (lineIndex ? "b" : "c").repeat(64),
                      byteOffset: 0,
                      lineComplete: true,
                      contentBytesB64: l.content.bytesB64,
                    })),
                  })),
                })),
              };
            }
            if (args.request.method === "repo.commit") {
              const hex = args.request.params.commitOid;
              const stash = ["e", "9", "f"].some(
                (char) => hex === oid(char).hex,
              );
              return {
                ...commit(hex === oid("d").hex || hex === oid("f").hex),
                oid: { algorithm: "sha1", hex },
                ...(stash
                  ? {
                      parents:
                        hex === oid("f").hex
                          ? []
                          : [oid("b"), oid("c"), oid("f")],
                      message: path("Saved stash content"),
                    }
                  : {}),
              };
            }
            if (args.request.method === "repo.history")
              return {
                snapshot: "h",
                entries: ["e", "9", "f"].some(
                  (char) => args.request.params.revision === oid(char).hex,
                )
                  ? [
                      {
                        ...commit(),
                        oid: {
                          algorithm: "sha1",
                          hex: args.request.params.revision,
                        },
                        parents:
                          args.request.params.revision === oid("f").hex
                            ? []
                            : [oid("b"), oid("c"), oid("f")],
                        message: path("Saved stash content"),
                      },
                    ]
                  : args.request.params.cursor
                    ? [commit(true)]
                    : [commit()],
                nextCursor:
                  args.request.params.revision === "HEAD" &&
                  !args.request.params.cursor
                    ? "history-next"
                    : null,
                metadata: { resolvedRevision: oid("a"), truncated: false },
              };
            if (
              [
                "repo.commit_files",
                "repo.commit_diff",
                "repo.commit_diff_page",
              ].includes(args.request.method)
            ) {
              const params = args.request.params;
              const root = [oid("d").hex, oid("f").hex].includes(
                params.commitOid,
              );
              const isStash = [oid("e").hex, oid("9").hex].includes(
                params.commitOid,
              );
              const parents = root
                ? []
                : isStash
                  ? [oid("b"), oid("c"), oid("f")]
                  : [oid("b"), oid("c")];
              const comparison = {
                commitOid: { algorithm: "sha1", hex: params.commitOid },
                parents,
                parentIndex: root ? null : params.parentIndex,
                parentOid: root ? null : parents[params.parentIndex],
              };
              const file = {
                oldPath: null,
                newPath:
                  params.path ??
                  path(params.cursor ? "second.txt" : "historical.txt"),
                oldOid: null,
                newOid: oid("b"),
                oldMode: 0,
                newMode: 33188,
                status: "Added",
              };
              if (args.request.method === "repo.commit_files")
                return {
                  snapshot: "files",
                  entries: [file],
                  nextCursor: params.cursor ? null : "next",
                  metadata: { ...comparison, totalFiles: 2, truncated: false },
                };
              if (args.request.method === "repo.commit_diff_page")
                return {
                  snapshot: params.commitOid,
                  nextCursor: null,
                  metadata: {
                    ...comparison,
                    contextLines: 3,
                    selectedPath: params.path,
                    readOnly: true,
                    hasOmissions: false,
                    totalUnits: 1,
                  },
                  entries: [
                    {
                      ...file,
                      fileIndex: 0,
                      binary: false,
                      omissionReason: null,
                      additions: 1,
                      deletions: 0,
                      hunks: [
                        {
                          index: 0,
                          oldStart: 0,
                          oldLines: 0,
                          newStart: 1,
                          newLines: 1,
                          lines: [
                            {
                              lineIndex: 0,
                              byteOffset: 0,
                              lineComplete: true,
                              origin: "+",
                              oldLine: null,
                              newLine: 1,
                              contentBytesB64: path(
                                `Historical parent ${params.parentIndex + 1}\n`,
                              ).bytesB64,
                            },
                          ],
                        },
                      ],
                    },
                  ],
                };
              return {
                snapshot: params.commitOid,
                diff: {
                  ...comparison,
                  files: [
                    {
                      ...file,
                      binary: false,
                      additions: 1,
                      deletions: 0,
                      hunks: [
                        {
                          oldStart: 0,
                          oldLines: 0,
                          newStart: 1,
                          newLines: 1,
                          lines: [
                            {
                              origin: "+",
                              oldLine: null,
                              newLine: 1,
                              content: path(
                                `Historical parent ${params.parentIndex + 1}\n`,
                              ),
                            },
                          ],
                        },
                      ],
                    },
                  ],
                  truncated: false,
                  readOnly: true,
                },
              };
            }
            return { closed: true };
          }
          return null;
        },
      },
    });
  });
  await page.goto("/");
  await page.getByRole("tab", { name: "Projects", exact: true }).click();
  await expect(
    page.getByRole("tab", { name: "Projects", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(
    page.getByRole("button", { name: "Web application", exact: true }),
  ).toBeVisible();
  // The library reads a bookmark only when asked: listing must not open every
  // repository, so it offers the read -- for every row, and per row -- and
  // claims nothing until it has run.
  await expect(
    page.getByRole("button", { name: "Check status", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", {
      name: "Check status of Web application",
      exact: true,
    }),
  ).toBeAttached();
  await expect(page.getByText("1 change", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Check status", exact: true }).click();
  await expect(page.getByText("1 change", { exact: true })).toBeVisible();
  // The read succeeded, so the row must not still say it was not read.
  await expect(page.getByText("Not checked")).toHaveCount(0);
  await expect(page.getByText("Not read")).toHaveCount(0);
  await expect(page.getByText("main", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", {
      name: "Check status of Web application",
      exact: true,
    }),
  ).toHaveCount(0);
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-list-checked-${info.project.name}.png`,
  });
  // The folder chooser browses the server instead of demanding a typed path.
  await libraryAction(page, "Add existing");
  await page.getByRole("button", { name: "Browse…", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "Choose a repository folder" }),
  ).toBeVisible();
  // The listing must arrive: an unbrowsable chooser is the fault this covers.
  await expect(
    page.getByRole("button", { name: "web-app", exact: true }),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  // Head-truncating the path costs its character order unless it is isolated.
  expect(
    await page
      .locator(".git-folder-current bdi")
      .evaluate((node) => node.textContent),
  ).toBe("/srv");
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-folder-chooser-${info.project.name}.png`,
  });
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  mkdirSync(".impeccable/screenshots", { recursive: true });
  // The library's own tokens remap under .dark; capture it so the remap is seen.
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await page.screenshot({
    path: `.impeccable/screenshots/projects-list-dark-${info.project.name}.png`,
  });
  await page.emulateMedia({ colorScheme: "light" });
  await page.screenshot({
    path: `.impeccable/screenshots/projects-list-${info.project.name}.png`,
  });
  // Also captured at the width the library was measured at, so it is looked at
  // where its numbers were taken rather than only at the narrower default.
  await page.setViewportSize({ width: 1400, height: 900 });
  await expect(page.locator(".git-library-row").first()).toBeVisible();
  await page.screenshot({
    path: `.impeccable/screenshots/projects-list-wide-${info.project.name}.png`,
  });
  // The library had never been captured narrow, where its secondary columns
  // give way so the project's identity keeps its room.
  await page.setViewportSize({ width: 640, height: 900 });
  await expect(page.locator(".git-library-row").first()).toBeVisible();
  await page.screenshot({
    path: `.impeccable/screenshots/projects-list-narrow-${info.project.name}.png`,
  });
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.setViewportSize({ width: 960, height: 680 });
  await page
    .getByRole("button", { name: "Web application", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "src/App.tsx", exact: true }).first(),
  ).toBeVisible();
  const changedSearch = page.getByRole("textbox", {
    name: "Filter changed files",
  });
  await changedSearch.fill("missing-path");
  await expect(
    page.getByText("0 of 0 matching files loaded.", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("No files match your current filters", { exact: true }),
  ).toBeVisible();
  // No matches is not a clean tree and must not hide the search box.
  await expect(changedSearch).toBeFocused();
  await changedSearch.fill("APP");
  await expect(
    page.getByText("1 of 1 matching files loaded.", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "src/App.tsx", exact: true }).first(),
  ).toBeVisible();
  await changedSearch.fill("");
  await expect(
    page.getByText("Searching changed files…", { exact: true }),
  ).toHaveCount(0);
  // A clipped path stays reachable: the column is too narrow for a long one.
  await expect(page.locator(".git-change-path").first()).toHaveAttribute(
    "title",
    "src/App.tsx",
  );
  await page
    .getByRole("button", { name: "src/App.tsx", exact: true })
    .first()
    .click();
  await expect(page.getByText("staged update", { exact: true })).toBeVisible();
  await page.getByRole("combobox", { name: "Diff comparison" }).click();
  await page.getByRole("option", { name: "Unstaged", exact: true }).click();
  await expect(
    page.getByText("unstaged update", { exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: `.impeccable/screenshots/projects-changes-${info.project.name}.png`,
  });
  await page.getByRole("button", { name: "Unstage file", exact: true }).click();
  await page
    .getByRole("button", { name: "src/App.tsx", exact: true })
    .first()
    .click();
  await expect(
    page.getByRole("button", { name: "Unstage file", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Stage file", exact: true }).click();
  await expect(
    page.getByText("Outcome not yet confirmed", { exact: false }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Stage file", exact: true }),
  ).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "Dismiss outcome", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByText("Web application · /srv/web-app", { exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: `.impeccable/screenshots/projects-recovery-${info.project.name}.png`,
  });
  await page
    .getByRole("button", { name: "Branches: main", exact: true })
    .click();
  // A receipt must not block navigation or every repository on the server.
  // The agent's operation journal enforces the repository-specific write guard.
  await expect(
    page.getByPlaceholder("Filter branches", { exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "Check outcome", exact: true })
    .click();
  await expect(
    page.getByText("stage · Completed", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Dismiss outcome", exact: true })
    .click();
  await backToProjects(page);
  await page
    .getByRole("button", { name: "Web application", exact: true })
    .click();
  // The consolidated menu is grouped as the design groups it; no screenshot
  // showed this surface, and the grouping is the point of consolidating it.
  await page.getByRole("button", { name: "Git actions", exact: true }).click();
  await expect(
    page.getByRole("menuitem", { name: "Stashes", exact: true }),
  ).toBeVisible();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-git-actions-${info.project.name}.png`,
  });
  await page.keyboard.press("Escape");
  // Adjacent blocks that share a fill with no rule between them read as one
  // block. That has been the dominant fault here: a notice dissolved into the
  // diff's header band, invisible in screenshots because both were plainly
  // drawn — only the numbers showed they were the same colour and touching.
  // Exactly one such pair is intentional: the header band's own two rows, which
  // the prototype draws as a single unit.
  const fused = await page.evaluate(() => {
    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d")!;
    const read = (v: string, under: string) => {
      ctx.clearRect(0, 0, 1, 1);
      ctx.fillStyle = under;
      ctx.fillRect(0, 0, 1, 1);
      ctx.fillStyle = v;
      ctx.fillRect(0, 0, 1, 1);
      const d = ctx.getImageData(0, 0, 1, 1).data;
      return `${d[0]},${d[1]},${d[2]}`;
    };
    const fill = (el: Element) => {
      const v = getComputedStyle(el).backgroundColor;
      const w = read(v, "#fff");
      const b = read(v, "#000");
      return w === b ? w : null;
    };
    const name = (el: Element) =>
      el.tagName.toLowerCase() +
      (typeof el.className === "string" && el.className
        ? "." + el.className.trim().split(/\s+/)[0]
        : "");
    const roots = [
      ...document.querySelectorAll(
        '.git-projects, [data-slot="dialog-content"]',
      ),
    ];
    const pairs: string[] = [];
    for (const root of roots) {
      for (const el of root.querySelectorAll("*")) {
        const next = el.nextElementSibling;
        if (!next) continue;
        const a = fill(el);
        const b = fill(next);
        if (!a || !b || a !== b) continue;
        const ar = el.getBoundingClientRect();
        const br = next.getBoundingClientRect();
        if (ar.width === 0 || br.width === 0) continue;
        if (Math.abs(br.top - ar.bottom) > 1) continue;
        if (
          parseFloat(getComputedStyle(el).borderBottomWidth) > 0 ||
          parseFloat(getComputedStyle(next).borderTopWidth) > 0
        )
          continue;
        pairs.push(`${name(el)} + ${name(next)}`);
      }
    }
    const host = document.createElement("div");
    host.innerHTML =
      '<div style="background:#123456;height:4px"></div><div style="background:#123456;height:4px"></div>';
    (roots[0] as HTMLElement).appendChild(host);
    const kids = [...host.children];
    const control =
      fill(kids[0]) === fill(kids[1]) &&
      fill(kids[0]) !== null &&
      Math.abs(
        kids[1].getBoundingClientRect().top -
          kids[0].getBoundingClientRect().bottom,
      ) <= 1;
    host.remove();
    return { control, pairs };
  });
  expect(fused.control).toBe(true);
  // The diff header is one ruled row now, so no pair is intentional any more.
  expect(fused.pairs).toEqual([]);
  await openGitAction(page, "Stashes");
  await page
    .getByRole("button", { name: "Save changes…", exact: true })
    .click();
  await page.getByLabel("Stash message", { exact: true }).fill("Save work");
  await page
    .getByRole("checkbox", { name: "Include untracked files", exact: true })
    .check();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-stash-save-${info.project.name}.png`,
  });
  await page.getByRole("button", { name: "Save stash", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Stashes");
  await page.getByRole("button", { name: /Save work/ }).click();
  await expect(
    page.getByRole("button", { name: "Untracked files", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Untracked files", exact: true })
    .click();
  await page
    .getByRole("button", { name: "historical.txt", exact: true })
    .click();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-stash-inspect-${info.project.name}.png`,
  });
  // No dialog had ever been captured in dark, and they carry the destructive
  // buttons, the alert treatment and the selection tint. Capture the richest.
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  // Paint the colour to decide whether the theme has flipped: reading the first
  // number out of the computed string passes immediately in Chromium, where it
  // is `oklch(0.97 …)`, and the capture below would then be taken in light.
  await page.waitForFunction(() => {
    const el = document.querySelector(".git-stash-workspace");
    if (!el) return false;
    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d")!;
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, 1, 1);
    ctx.fillStyle = getComputedStyle(el).backgroundColor;
    ctx.fillRect(0, 0, 1, 1);
    return ctx.getImageData(0, 0, 1, 1).data[0] < 128;
  });
  await page.screenshot({
    path: `.impeccable/screenshots/projects-stash-inspect-dark-${info.project.name}.png`,
    animations: "disabled",
  });
  // No absolutely positioned element in these surfaces may escape the box it is
  // positioned against. The split handle straddles its seam by a deliberate
  // 5px; when it lost its containing block it was measured overhanging by
  // 676px, lying across unrelated content as a resize target -- invisible in
  // every screenshot, and breaking no assertion. The check carries a positive
  // control so a clean run cannot mean it did nothing.
  await page.setViewportSize({ width: 1400, height: 900 });
  await page.waitForFunction(
    () => Math.abs(document.documentElement.clientWidth - 1400) <= 20,
  );
  const stray = await page.evaluate(() => {
    const name = (el: Element | null) => {
      if (!el) return "NONE";
      if (el === document.body) return "BODY";
      const c =
        typeof el.className === "string" && el.className
          ? "." + el.className.trim().split(/\s+/).slice(0, 2).join(".")
          : "";
      return el.tagName.toLowerCase() + c;
    };
    const roots = [
      ...document.querySelectorAll(
        '.git-projects, [data-slot="dialog-content"]',
      ),
    ];
    const escapes: string[] = [];
    for (const root of roots) {
      for (const el of root.querySelectorAll("*")) {
        if (getComputedStyle(el).position !== "absolute") continue;
        const r = el.getBoundingClientRect();
        // Screen-reader-only labels are 1x1 and clipped; where they are
        // positioned from does not affect anything anyone can see or click.
        if (r.width < 4 || r.height < 4) continue;
        let op = el instanceof HTMLElement ? el.offsetParent : el.parentElement;
        if (!(el instanceof HTMLElement)) {
          while (op && getComputedStyle(op).position === "static")
            op = op.parentElement;
        }
        const opr = op ? op.getBoundingClientRect() : null;
        const out = opr
          ? Math.round(
              Math.max(
                0,
                opr.left - r.left,
                r.right - opr.right,
                opr.top - r.top,
                r.bottom - opr.bottom,
              ),
            )
          : 9999;
        // Distance from the element to whatever is positioning it. The real
        // fault was not geometric -- the stray handle overhung its dialog by
        // only 4px -- it was that the dialog, many levels up, was doing the
        // positioning at all. A local container is at most a couple of levels
        // above the element.
        let depth = 0;
        let n: Element | null = el.parentElement;
        while (n && n !== op && depth < 40) {
          n = n.parentElement;
          depth += 1;
        }
        const distant = !op || n !== op || depth > 2;
        if (distant) {
          escapes.push(
            `${name(el)} positioned by ${name(op)} ${depth} levels up`,
          );
        } else if (out > 8) {
          escapes.push(`${name(el)} out ${out} of ${name(op)}`);
        }
      }
    }
    const host = document.createElement("div");
    const spoiler = document.createElement("div");
    spoiler.style.cssText =
      "position:absolute;left:-400px;top:-400px;width:20px;height:20px";
    host.append(spoiler);
    (roots[0] as HTMLElement).append(host);
    const sop = spoiler.offsetParent;
    const sopr = sop ? sop.getBoundingClientRect() : null;
    const sr = spoiler.getBoundingClientRect();
    const control =
      !sop || (sopr !== null && sopr.left - sr.left > 8) ? "fires" : "DEAD";
    host.remove();
    return { control, escapes };
  });
  expect(stray.control).toBe("fires");
  expect(stray.escapes).toEqual([]);
  await page.setViewportSize({ width: 960, height: 680 });
  await page.emulateMedia({ colorScheme: "light" });
  await page.getByRole("button", { name: "Apply…", exact: true }).click();
  await page
    .getByRole("checkbox", {
      name: "Restore which changes were staged",
      exact: true,
    })
    .check();
  await page.getByRole("button", { name: "Apply stash", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Stashes");
  await page
    .getByRole("button", { name: "Save changes…", exact: true })
    .click();
  await page.getByLabel("Stash message", { exact: true }).fill("Second stash");
  await page.getByRole("button", { name: "Save stash", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Stashes");
  // Both entries deliberately share an OID; select the older entry at index 1.
  await page.getByRole("button", { name: /Save work/ }).click();
  await page.getByRole("button", { name: "Apply…", exact: true }).click();
  await page.getByRole("button", { name: "Apply stash", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Stashes");
  await page.getByRole("button", { name: /Second stash/ }).click();
  await page.getByRole("button", { name: "Drop…", exact: true }).click();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-stash-drop-${info.project.name}.png`,
  });
  await page.getByRole("button", { name: "Drop stash", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Stashes");
  await page.getByRole("button", { name: /Save work/ }).click();
  await page.getByRole("button", { name: "Pop…", exact: true }).click();
  await page.getByRole("button", { name: "Pop stash", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  // The composer sits under the file list, with a summary and a description.
  await page
    .getByLabel("Commit summary", { exact: true })
    .fill("Implement the application");
  await page
    .getByLabel("Commit description", { exact: true })
    .fill("Adds the first screens.");
  await page.getByRole("button", { name: /^Commit \d+ files? to / }).click();
  // A clean tree now names the branch and its commit instead of a bare line.
  await expect(
    page.getByText("No local changes", { exact: true }),
  ).toBeVisible();
  // With nothing left to commit the composer is not offered at all, rather
  // than sitting there disabled; the clean state offers where to go instead.
  await expect(page.getByLabel("Commit summary", { exact: true })).toHaveCount(
    0,
  );
  await expect(
    page.getByRole("button", { name: "View history", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("region", { name: "Git operation recovery" }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "Branches: main", exact: true })
    .click();
  await page.getByRole("button", { name: "New branch", exact: true }).click();
  await page.getByLabel("Branch name", { exact: true }).fill("feature");
  await page
    .getByRole("button", { name: "Create branch", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page
    .getByRole("button", { name: "Branches: main", exact: true })
    .click();
  await expect(page.getByText("feature", { exact: true })).toBeVisible();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-branches-${info.project.name}.png`,
  });
  await page
    .getByLabel("Filter branches", { exact: true })
    .fill("does-not-exist");
  await expect(
    page.getByText("No matching branches.", {
      exact: true,
    }),
  ).toBeVisible();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-branches-filter-${info.project.name}.png`,
  });
  await page.getByLabel("Filter branches", { exact: true }).fill("");
  for (const choice of ["main · local", "None — remove tracking"]) {
    await page
      .getByRole("option")
      .filter({ has: page.getByText("feature", { exact: true }) })
      .click({ button: "right" });
    await page
      .getByRole("menuitem", { name: "Set upstream…", exact: true })
      .click();
    await page
      .getByRole("combobox", { name: "Upstream branch", exact: true })
      .click();
    await expect(
      page.getByRole("option", { name: "feature · local", exact: true }),
    ).toHaveCount(0);
    await page.getByRole("option", { name: choice, exact: true }).click();
    await page.screenshot({
      animations: "disabled",
      path: `.impeccable/screenshots/projects-upstream-${choice.startsWith("None") ? "remove" : "set"}-${info.project.name}.png`,
    });
    await page
      .getByRole("button", { name: "Save upstream", exact: true })
      .click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await page
      .getByRole("button", { name: "Branches: main", exact: true })
      .click();
    if (choice.startsWith("main"))
      await expect(
        page.getByText("Tracks main", { exact: true }),
      ).toBeVisible();
    else
      await expect(page.getByText("Tracks main", { exact: true })).toHaveCount(
        0,
      );
  }
  await page
    .getByRole("option")
    .filter({ has: page.getByText("feature", { exact: true }) })
    .click({ button: "right" });
  await page.getByRole("menuitem", { name: "Rename…", exact: true }).click();
  await page.getByLabel("Branch name", { exact: true }).fill("feature-renamed");
  await page
    .getByRole("button", { name: "Rename branch", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page
    .getByRole("button", { name: "Branches: main", exact: true })
    .click();
  await page.getByRole("option").filter({ hasText: "feature-renamed" }).click();
  await expect(
    page.getByRole("button", {
      name: "Branches: feature-renamed",
      exact: true,
    }),
  ).toBeVisible();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page
    .getByRole("button", { name: "Branches: feature-renamed", exact: true })
    .click();
  await page
    .getByRole("option")
    .filter({ has: page.getByText("main", { exact: true }) })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page
    .getByRole("button", { name: "Branches: main", exact: true })
    .click();
  await page
    .getByRole("option")
    .filter({ has: page.getByText("feature-renamed", { exact: true }) })
    .click({ button: "right" });
  await page.getByRole("menuitem", { name: "Delete…", exact: true }).click();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-branch-delete-${info.project.name}.png`,
  });
  await page
    .getByRole("button", { name: "Delete branch", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Remotes");
  await expect(
    page.getByText(
      "No remotes configured. Add one to fetch and publish branches.",
      { exact: true },
    ),
  ).toBeVisible();
  await page.getByRole("button", { name: "Add remote", exact: true }).click();
  await page.getByLabel("Remote name", { exact: true }).fill("origin");
  await page
    .getByLabel("Remote URL", { exact: true })
    .fill("git@host:team/app.git");
  await page.getByRole("button", { name: "Save remote", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Remotes");
  await expect(
    page
      .getByRole("region", { name: "Remotes", exact: true })
      .getByRole("button", { name: "Fetch origin", exact: true }),
  ).toBeEnabled();
  await page.getByRole("combobox", { name: "Remote", exact: true }).click();
  await page
    .getByRole("combobox", { name: "Search remotes", exact: true })
    .fill("ORIGIN");
  await expect(
    page.getByRole("option", { name: "origin", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("combobox", { name: "Search remotes", exact: true })
    .fill("no-match");
  await expect(
    page.getByText("No matching remotes.", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("combobox", { name: "Search remotes", exact: true })
    .fill("origin");
  await page.getByRole("option", { name: "origin", exact: true }).click();
  await expect(
    page.getByRole("combobox", { name: "Remote", exact: true }),
  ).toContainText("origin");
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-remotes-${info.project.name}.png`,
  });
  await page
    .getByRole("button", { name: "Actions for remote origin", exact: true })
    .click();
  await page
    .getByRole("menuitem", { name: "Remote branches and tags…", exact: true })
    .click();
  await expect(
    page.getByText("All remote references loaded", { exact: true }),
  ).toBeVisible();
  await expect(
    page
      .getByRole("listitem")
      .filter({ hasText: "refs/tags/old-tag^{}" })
      .getByRole("button"),
  ).toHaveCount(0);
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-remote-refs-${info.project.name}.png`,
  });
  const remoteDialogBounds = await page
    .locator(".git-inspector-section")
    .boundingBox();
  const backBounds = await page
    .getByRole("button", { name: "Back to remotes", exact: true })
    .boundingBox();
  expect(backBounds!.x).toBeGreaterThanOrEqual(remoteDialogBounds!.x);
  expect(backBounds!.x + backBounds!.width).toBeLessThanOrEqual(
    remoteDialogBounds!.x + remoteDialogBounds!.width,
  );

  await page
    .getByRole("button", { name: "Push with lease…", exact: true })
    .click();
  await expect(
    page.getByText("This can remove commits from shared remote history.", {
      exact: false,
    }),
  ).toBeVisible();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-remote-lease-${info.project.name}.png`,
  });
  await page
    .getByRole("button", { name: "Back to remote references", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Push with lease…", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Replace remote branch", exact: true })
    .click();
  await expect(
    page
      .getByRole("listitem")
      .filter({ has: page.getByText("refs/heads/old", { exact: true }) }),
  ).toContainText("aaaaaaaaaaaa");
  for (const [reference, kind] of [
    ["refs/heads/old", "branch"],
    ["refs/tags/old-tag", "tag"],
  ]) {
    await page
      .getByRole("listitem")
      .filter({ has: page.getByText(reference, { exact: true }) })
      .getByRole("button", { name: "Delete…", exact: true })
      .click();
    await expect(
      page.getByText(
        "This removes the reference from the shared remote repository.",
        { exact: false },
      ),
    ).toBeVisible();
    await page.screenshot({
      animations: "disabled",
      path: `.impeccable/screenshots/projects-remote-delete-${kind}-${info.project.name}.png`,
    });
    await page
      .getByRole("button", { name: `Delete remote ${kind}`, exact: true })
      .click();
    await expect(
      page.getByRole("button", {
        name: "Refresh remote references",
        exact: true,
      }),
    ).toBeVisible();
    await expect(page.getByText(reference, { exact: true })).toHaveCount(0);
  }
  await page
    .getByRole("button", { name: "Back to remotes", exact: true })
    .click();
  await page
    .getByRole("region", { name: "Remotes", exact: true })
    .getByRole("button", { name: "Fetch origin", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Remotes");
  await page.locator(".git-remote-transfer summary").click();
  await page
    .getByRole("button", { name: "Pull fast-forward", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Remotes");
  await page.locator(".git-remote-transfer summary").click();
  await page.getByRole("button", { name: "Push branch", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Tags");
  await expect(page.getByText("No tags in this repository.")).toBeVisible();
  await page.getByRole("button", { name: "New tag", exact: true }).click();
  await page.getByLabel("Tag name", { exact: true }).fill("v0.1.0");
  await expect(
    page.getByLabel("Target object ID", { exact: true }),
  ).toHaveValue("a".repeat(40));
  await page.getByRole("button", { name: "Create tag", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Tags");
  await page.getByRole("button", { name: "New tag", exact: true }).click();
  await page.getByLabel("Tag name", { exact: true }).fill("v1.0.0");
  await page.getByRole("combobox", { name: "Tag type", exact: true }).click();
  await page.getByRole("option", { name: "Annotated", exact: true }).click();
  await page
    .getByLabel("Tag message", { exact: true })
    .fill("First stable release\nPreserves the annotation.");
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-tag-create-${info.project.name}.png`,
  });
  await page.getByRole("button", { name: "Create tag", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Tags");
  await loadAllTags();
  await page.getByRole("button", { name: /v1.0.0 Annotated/ }).click();
  await expect(
    page.getByText("First stable release", { exact: false }),
  ).toBeVisible();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-tags-${info.project.name}.png`,
  });
  await page.getByRole("button", { name: "Push…", exact: true }).click();
  await expect(
    page.getByRole("combobox", { name: "Remote", exact: true }),
  ).toContainText("origin");
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-tag-push-${info.project.name}.png`,
  });
  await page.getByRole("button", { name: "Push tag", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Tags");
  await loadAllTags();

  await expect(
    page.getByRole("button", { name: /v0.1.0 Lightweight/ }),
  ).toBeVisible();
  await loadAllTags();
  await page.getByRole("button", { name: /v1.0.0 Annotated/ }).click();
  await page
    .getByRole("button", { name: "Delete local tag…", exact: true })
    .click();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-tag-delete-${info.project.name}.png`,
  });
  await page
    .getByRole("button", { name: "Delete local tag", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);

  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Remotes");
  await page
    .getByRole("button", { name: "Actions for remote origin", exact: true })
    .click();
  await page.getByRole("menuitem", { name: "Rename…", exact: true }).click();
  await page.getByLabel("Remote name", { exact: true }).fill("upstream");
  await page.getByRole("button", { name: "Save remote", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Remotes");
  await page
    .getByRole("button", { name: "Actions for remote upstream", exact: true })
    .click();
  await page
    .getByRole("menuitem", { name: "Change URL…", exact: true })
    .click();
  await page
    .getByLabel("Remote URL", { exact: true })
    .fill("git@host:team/updated.git");
  await page.getByRole("button", { name: "Save remote", exact: true }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await openGitAction(page, "Remotes");
  await expect(
    page.getByText("git@host:team/updated.git", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Actions for remote upstream", exact: true })
    .click();
  await page.getByRole("menuitem", { name: "Remove…", exact: true }).click();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-remove-remote-${info.project.name}.png`,
  });
  await page
    .getByRole("button", { name: "Remove remote", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("tab", { name: "History", exact: true }).click();
  // The short first page fills the viewport automatically without a manual click.
  await expect(
    page.getByRole("button", { name: /Initial commit/ }),
  ).toBeVisible();
  await expect(page.getByText("End of history", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /Merge feature/ }).click();
  await page
    .getByRole("button", { name: "historical.txt", exact: true })
    .click();
  await expect(
    page.getByText("Historical parent 1", { exact: true }),
  ).toBeVisible();
  await page.getByRole("combobox", { name: "Compare with parent" }).click();
  await page.getByRole("option", { name: /Parent 2/ }).click();
  await page
    .getByRole("button", { name: "historical.txt", exact: true })
    .click();
  await expect(
    page.getByText("Historical parent 2", { exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: `.impeccable/screenshots/projects-history-${info.project.name}.png`,
  });
  await page.setViewportSize({ width: 1400, height: 900 });
  await expect(page.locator(".git-history-commits li").first()).toBeVisible();
  await page.screenshot({
    path: `.impeccable/screenshots/projects-history-wide-${info.project.name}.png`,
  });
  // The design gives the commit's files/diff split a handle, as the Changes
  // view has: present above the stacking width, absent once stacked.
  await expect(
    page.getByRole("separator", { name: "Resize the commit file list" }),
  ).toBeVisible();
  await page.setViewportSize({ width: 1000, height: 900 });
  await expect(page.locator(".git-commit-files li").first()).toBeVisible();
  await expect(
    page.getByRole("separator", { name: "Resize the commit file list" }),
  ).toBeHidden();
  await page.setViewportSize({ width: 1400, height: 900 });
  // Between roughly 780 and 1200px the commit's file list stacks above the
  // diff; that arrangement had never been captured.
  await page.setViewportSize({ width: 1000, height: 900 });
  await expect(page.locator(".git-commit-files li").first()).toBeVisible();
  await page.screenshot({
    path: `.impeccable/screenshots/projects-history-stacked-${info.project.name}.png`,
  });
  await page.setViewportSize({ width: 1400, height: 900 });
  // The history list's tint, its row rules and its selected-commit colour are
  // all token-derived; capture dark so the remap is seen rather than assumed.
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await page.evaluate(
    () => new Promise((done) => requestAnimationFrame(() => done(null))),
  );
  await page.screenshot({
    path: `.impeccable/screenshots/projects-history-dark-${info.project.name}.png`,
    animations: "disabled",
  });
  await page.emulateMedia({ colorScheme: "light" });
  await page.evaluate(
    () => new Promise((done) => requestAnimationFrame(() => done(null))),
  );
  await page.setViewportSize({ width: 960, height: 680 });
  await expect(
    page.getByText("All commit files loaded", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "second.txt", exact: true }),
  ).toBeVisible();

  await expect(
    page.getByRole("button", { name: "historical.txt", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: /Initial commit/ }).click();
  await expect(
    page.getByText("Initial commit · compared with an empty tree"),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Branches: main", exact: true })
    .click();
  await page.getByRole("button", { name: "New branch", exact: true }).click();
  await page.getByLabel("Branch name", { exact: true }).fill("from-history");
  await page
    .getByRole("button", { name: "Create branch", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(
    page.getByRole("tab", { name: "History", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  await expect(
    page.getByRole("button", { name: /Merge feature/ }),
  ).toBeVisible();
  await page.getByRole("button", { name: /Merge feature/ }).click();
  await page
    .getByRole("button", { name: "Commit actions", exact: true })
    .click();
  await page
    .getByRole("menuitem", { name: "Merge into current branch…", exact: true })
    .click();
  await page.getByRole("button", { name: "Merge", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "Active Git operation" }),
  ).toContainText("Merge in progress");
  await expect(
    page.getByRole("button", { name: "Continue merge", exact: true }),
  ).toBeDisabled();
  await page.getByRole("tab", { name: "Changes", exact: true }).click();
  await page
    .getByRole("button", { name: "src/App.tsx", exact: true })
    .first()
    .click();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-integration-${info.project.name}.png`,
  });
  // The three recorded sides are inspectable, and choosing one resolves it.
  await expect(
    page.getByText("Unresolved conflict", { exact: false }),
  ).toBeVisible();
  await page.getByRole("button", { name: "View Theirs", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "Theirs content", exact: true }),
  ).toContainText("theirs");
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-conflict-${info.project.name}.png`,
  });
  await page.getByRole("button", { name: "Use Theirs", exact: true }).click();
  await page
    .getByRole("button", { name: "Use Theirs", exact: true })
    .last()
    .click();
  await expect(
    page.getByText("Conflict resolved and staged.", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Continue merge", exact: true })
    .click();
  await page
    .getByLabel("Commit message (optional)", { exact: true })
    .fill("Resolve integration");
  await page
    .getByRole("button", { name: "Continue operation", exact: true })
    .click();
  await expect(
    page.getByRole("region", { name: "Active Git operation" }),
  ).toHaveCount(0);
  await page.getByRole("tab", { name: "History", exact: true }).click();
  // The short first page fills the viewport automatically without a manual click.
  await expect(
    page.getByRole("button", { name: /Initial commit/ }),
  ).toBeVisible();
  await expect(page.getByText("End of history", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: /Merge feature/ }).click();
  await page
    .getByRole("button", { name: "Commit actions", exact: true })
    .click();
  await page
    .getByRole("menuitem", { name: "Cherry-pick commit…", exact: true })
    .click();
  await page
    .getByRole("combobox", { name: "Mainline parent", exact: true })
    .click();
  await page.getByRole("option", { name: /Parent 2/ }).click();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-cherry-pick-${info.project.name}.png`,
  });
  await page.getByRole("button", { name: "Cherry-pick", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "Active Git operation" }),
  ).toContainText("Cherry-pick in progress");
  await page.getByRole("button", { name: "Abort…", exact: true }).click();
  await page.screenshot({
    animations: "disabled",
    path: `.impeccable/screenshots/projects-abort-${info.project.name}.png`,
  });
  await page
    .getByRole("button", { name: "Abort operation", exact: true })
    .click();
  await expect(
    page.getByRole("region", { name: "Active Git operation" }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: /Initial commit/ }).click();
  await page
    .getByRole("button", { name: "Commit actions", exact: true })
    .click();
  await page
    .getByRole("menuitem", {
      name: "Rebase current branch onto commit…",
      exact: true,
    })
    .click();
  await page.getByRole("button", { name: "Start rebase", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "Active Git operation" }),
  ).toContainText("Rebase in progress · 1 of 2");
  await page.getByRole("button", { name: "Skip commit…", exact: true }).click();
  await page.getByRole("button", { name: "Skip commit", exact: true }).click();
  await expect(
    page.getByRole("region", { name: "Active Git operation" }),
  ).toHaveCount(0);
  await backToProjects(page);
  await page
    .getByRole("button", { name: "Actions for Web application" })
    .click();
  await page.getByRole("menuitem", { name: "Rename…", exact: true }).click();
  await page.getByRole("textbox", { name: "Project name" }).fill("Frontend");
  await page.getByRole("button", { name: "Save name" }).click();
  await expect(
    page.getByRole("button", { name: "Frontend", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Actions for Frontend" }).click();
  await page.getByRole("menuitem", { name: "Remove from projects…" }).click();
  await page.getByRole("button", { name: "Remove bookmark" }).click();
  await expect(page.getByRole("dialog").getByRole("alert")).toContainText(
    "Could not save project bookmarks",
  );
  await page.screenshot({
    path: `.impeccable/screenshots/projects-removal-error-${info.project.name}.png`,
  });
  // A form dialog in dark: the alert treatment and the destructive confirm had
  // only ever been seen on a light ground.
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  // Paint the colour to decide whether the theme has flipped: reading the first
  // number out of the computed string passes immediately in Chromium, where it
  // is `oklch(0.97 …)`, and the capture below would then be taken in light.
  await page.waitForFunction(() => {
    const el = document.querySelector('[data-slot="dialog-content"]');
    if (!el) return false;
    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d")!;
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, 1, 1);
    ctx.fillStyle = getComputedStyle(el).backgroundColor;
    ctx.fillRect(0, 0, 1, 1);
    return ctx.getImageData(0, 0, 1, 1).data[0] < 128;
  });
  await page.screenshot({
    path: `.impeccable/screenshots/projects-removal-error-dark-${info.project.name}.png`,
    animations: "disabled",
  });
  await page.emulateMedia({ colorScheme: "light" });
  await page.getByRole("button", { name: "Remove bookmark" }).click();
  // The library states its own empty case, says what a project is, and
  // offers the two ways to get one right there.
  await expect(
    page.getByRole("heading", { name: "No projects yet", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("A project is a bookmark to a Git repository", {
      exact: false,
    }),
  ).toBeVisible();
  await expect(
    page
      .locator(".git-library-empty")
      .getByRole("button", { name: "Clone repository", exact: true }),
  ).toBeVisible();
  await libraryAction(page, "Add existing");
  await page
    .getByRole("textbox", { name: "Project name" })
    .fill("Restored app");
  await page
    .getByRole("textbox", { name: "Repository path on server" })
    .fill("/srv/web-app/src");
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Add repository", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Restored app" }),
  ).toBeVisible();
  // A clean tree now names the branch and its commit instead of a bare line.
  await expect(
    page.getByText("No local changes", { exact: true }),
  ).toBeVisible();
  // The repository's own directory opens in the Files browser.
  await openGitAction(page, "Open in Files");
  await expect(
    page.getByRole("tab", { name: "Files", exact: true }),
  ).toHaveAttribute("aria-selected", "true");
  expectProjectStyles(styles, info);
});
