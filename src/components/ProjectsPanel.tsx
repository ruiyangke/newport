import { GitEmptyState } from "./GitEmptyState";
import {
  GitForm,
  GitField,
  GitPathField,
  GitFieldHint,
  GitError,
  GitFormActions,
  GitFormFields,
} from "./GitForm";
import { GitCommandLog } from "./GitCommandLog";
import { useGitPageLoader } from "../hooks/useGitPageLoader";
import { useNow } from "../hooks/useNow";
import { GitLoadMore } from "./GitLoadMore";
import { useEffect, useRef, useState } from "react";
import {
  isCancelledError,
  skipToken,
  useIsMutating,
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
  type QueryKey,
  type UseQueryOptions,
} from "@tanstack/react-query";
import { useNavigate } from "react-router";
import {
  Archive,
  ArrowLeft,
  CircleCheck,
  Ellipsis,
  FileDiff,
  FolderGit2,
  FolderOpen,
  GitCommitHorizontal,
  Globe,
  LayoutList,
  RefreshCw,
  Tag,
} from "lucide-react";
import { cn } from "cn";
import { GitProjects, GitProjectCreatedError } from "../api/gitProjects";
import { GitCreateProject } from "./GitCreateProject";
import { GitFolderChooser } from "./GitFolderChooser";
import { toast } from "sonner";
import { GitFileActions, GitRecoveryPanel } from "./GitWriteControls";
import type {
  GitOperationReceipt,
  GitWriteAction,
  GitPath,
  GitProject,
  GitBootstrapRequest,
} from "../domain/git";
import {
  type GitHistory,
  type GitRepository,
  type GitStatus,
} from "../domain/gitResponses";
import type { ReactNode } from "react";
import type { Server } from "../types";
import { patchWorkspaceValue } from "../state/workspace";
import {
  patchGitState,
  readGitState,
  selectionArrivalPatch,
  useGitState,
} from "../state/git";
import { useServerScope } from "../query/keys";
import { refreshQuery } from "../query/client";
import {
  gitKeys,
  gitQueries,
  invalidateRepository,
  normalizeStatusFilter,
} from "../query/git";
import {
  gitProjectsFor,
  gitResources,
  resetGitProjects,
} from "../git/registry";
import { Button, Input } from "./controls";
import { GitPicker } from "./GitPicker";
import { Modal } from "./Editors";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "./ui/dropdown-menu";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "./ui/tabs";
import { GitChangesPreview } from "./GitChangesPreview";
import { GitChangesSplit } from "./diff/GitChangesSplit";
import { GitCommitComposer } from "./GitCommitComposer";
import {
  GitProjectLibrary,
  type GitProjectRowStatus,
} from "./GitProjectLibrary";
import { GitStashControls } from "./GitStashControls";
import { GitTagControls } from "./GitTagControls";
import {
  GitWorktreeControls,
  type GitWorktreeEdit,
} from "./GitWorktreeControls";
import { GitWorktreePicker } from "./GitWorktreePicker";
import { GitNewWorktree } from "./GitNewWorktree";
import { openable, type WorktreeRow } from "../git/worktrees";
import { GitRemoteControls } from "./GitRemoteControls";
import { GitBranchControls } from "./GitBranchControls";
import { GitIntegrationControls } from "./GitIntegrationControls";
import { GitSyncControl } from "./GitSyncControl";
import { GitHistoryActions } from "./GitHistoryActions";
import { GitCommitInspector, relativeTime } from "./GitCommitInspector";
import { GitChangeList, changeGroups, changeMark } from "./GitChangeList";
import { GitNotice } from "./GitNotice";
import { gitErrorCode, gitErrorMessage } from "../git/errors";

function message(error: unknown) {
  return gitErrorMessage(error);
}
/**
 * How long ago a read returned. Describes Newport's own read only: nothing here
 * says the upstream server was contacted.
 */
function readAge(at: number | null, now: number) {
  if (at === null) return null;
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  const hours = Math.round(minutes / 60);
  return `${hours} hour${hours === 1 ? "" : "s"} ago`;
}
/** The current branch's short name, when HEAD is on one. */
function headBranch(status: GitStatus) {
  const head = status.metadata.head;
  if (head.detached || head.unborn || !head.name) return null;
  return head.name.display.replace(/^refs\/heads\//, "");
}
const EMPTY_PROJECTS: GitProject[] = [];
/** The open checkout's descriptor, resolved from the selection by query. */
export interface Opened {
  project: GitProject;
  repository: GitRepository;
}

export function ProjectsPanel({ server }: { server: Server }) {
  const scope = useServerScope(server.id);
  const queryClient = useQueryClient();
  const connection = scope.connection;

  // What the user chose lives in the Git store and survives navigation; what
  // the server said lives in Query. Dialog drafts stay local to this page.
  // `selection` names the open checkout (bookmark plus path); the resolved
  // descriptor below is a query, so background refreshes can never replace
  // the selection itself.
  const [selection] = useGitState(server.id, "opened");
  const openedQuery = useQuery(
    (selection
      ? {
          ...gitQueries.opened(scope, selection.project, selection.path),
          // A newly chosen checkout resolves behind the previous view,
          // exactly as opening inside the action did.
          placeholderData: (previous: Opened | undefined) => previous,
        }
      : {
          queryKey: [...gitKeys.all(scope), "opened"] as const,
          queryFn: skipToken,
        }) as UseQueryOptions<Opened, Error, Opened, QueryKey>,
  );
  // A failed open keeps the previous view, as a failed action did: the page
  // shows the last good descriptor with the new error, not an empty page.
  const lastGood = useRef<Opened | null>(null);
  if (openedQuery.data && !openedQuery.isPlaceholderData)
    lastGood.current = openedQuery.data;
  const opened = openedQuery.data ?? lastGood.current;
  const [tab, setTabState] = useGitState(server.id, "tab");
  const setTab = (next: string) =>
    setTabState(next === "history" ? "history" : "changes");
  const [selectedEntry] = useGitState(server.id, "selectedEntry");
  const [selectedSide] = useGitState(server.id, "selectedSide");
  const [selectedCommit, setSelectedCommit] = useGitState(
    server.id,
    "selectedCommit",
  );
  const [fileFilter, setFileFilter] = useGitState(server.id, "fileFilter");
  // The design puts a filter-options control beside the text filter. What it
  // offers here are the groups the list already builds -- it narrows to one of
  // them rather than introducing a classification of its own.
  const [groupFilter, setGroupFilter] = useGitState(server.id, "groupFilter");
  const [commitSummary, setCommitSummary] = useGitState(
    server.id,
    "commitSummary",
  );
  const [commitDescription, setCommitDescription] = useGitState(
    server.id,
    "commitDescription",
  );
  const [operationRepositories, setOperationRepositories] = useGitState(
    server.id,
    "operationRepositories",
  );
  // Receipts are acknowledged and forgotten; their repository names leave
  // with them instead of accumulating until the connection is evicted.
  function forgetOperationRepository(operationId: string) {
    setOperationRepositories((previous) => {
      if (!(operationId in previous)) return previous;
      const next = { ...previous };
      delete next[operationId];
      return next;
    });
  }
  const [search, setSearch] = useGitState(server.id, "librarySearch");
  const [libraryFilter, setLibraryFilter] = useGitState(
    server.id,
    "libraryFilter",
  );
  const [librarySort, setLibrarySort] = useGitState(server.id, "librarySort");
  const [favourites, setFavourites] = useGitState(server.id, "favourites");
  const [actionsPanel, setActionsPanel] = useGitState(
    server.id,
    "actionsPanel",
  );

  const [actionError, setError] = useState("");
  const [editor, setEditor] = useState<"add" | GitProject | null>(null);
  const [removing, setRemoving] = useState<GitProject | null>(null);
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [creating, setCreating] = useState<"init" | "clone" | null>(null);
  const [browsing, setBrowsing] = useState(false);
  const [newWorktree, setNewWorktree] = useState(false);
  const [worktreeEdit, setWorktreeEdit] = useState<GitWorktreeEdit>();

  const projectsQuery = useQuery(gitQueries.projects(scope));
  const projects = projectsQuery.data ?? EMPTY_PROJECTS;
  const receiptsQuery = useQuery(gitQueries.receipts(scope));
  // A failed read of saved outcomes is not an empty list: writes stay blocked
  // until they can be read, exactly as when they were held in component state.
  const receipts: GitOperationReceipt[] | null = receiptsQuery.isError
    ? null
    : (receiptsQuery.data ?? null);
  const recoveryError = receiptsQuery.isError
    ? `Cannot read saved outcomes: ${message(receiptsQuery.error)}`
    : "";

  const repoId = opened?.repository.repoId ?? "";
  const bare = opened?.repository.bare ?? false;
  const [debouncedFileFilter, setDebouncedFileFilter] = useState(fileFilter);
  useEffect(() => {
    const timer = setTimeout(() => setDebouncedFileFilter(fileFilter), 200);
    return () => clearTimeout(timer);
  }, [fileFilter]);
  const statusFilter = normalizeStatusFilter({
    text: debouncedFileFilter,
    group: groupFilter,
  });
  const filterPending =
    fileFilter.trim().toLowerCase() !==
    debouncedFileFilter.trim().toLowerCase();
  const statusQuery = useQuery({
    ...gitQueries.status(scope, repoId, statusFilter),
    enabled: !!opened && !bare,
    // Keep the filter input mounted while replacing its results. Never carry
    // rows between repositories; pending rows cannot be selected or written.
    placeholderData: (previous, query) =>
      query?.queryKey[gitKeys.repo(scope, repoId).length - 1] === repoId
        ? previous
        : undefined,
    // Agents edit while you watch: the open checkout is read again every
    // fifteen seconds while the window is showing, and not while an action of
    // the page's own is running. A read is cheap -- status fingerprints files
    // by metadata -- and an unchanged tree keeps its snapshot, so an unchanged
    // re-read disturbs nothing on screen.
    refetchInterval: () =>
      queryClient.isMutating({ mutationKey: [...gitKeys.all(scope), "action"] })
        ? false
        : 15_000,
  });
  const [lastStatus, setLastStatus] = useState<{
    repoId: string;
    data: GitStatus;
  }>();
  useEffect(() => {
    if (statusQuery.data && !statusQuery.isPlaceholderData)
      setLastStatus({ repoId, data: statusQuery.data });
  }, [repoId, statusQuery.data, statusQuery.isPlaceholderData]);
  const status =
    opened && !bare
      ? (statusQuery.data ??
        (lastStatus?.repoId === repoId ? lastStatus.data : null))
      : null;
  // A clean working tree shows the commit it matches, so history is read
  // for that view too.
  const cleanTree =
    (status?.metadata.totalEntries ?? status?.entries.length) === 0;
  const historyQuery = useQuery({
    ...gitQueries.history(scope, repoId),
    enabled: !!opened && (tab === "history" || bare || cleanTree),
  });
  const history = opened ? (historyQuery.data ?? null) : null;
  // When the repository was last read, taken from the read itself rather than
  // kept alongside it, so the footer cannot describe a read that did not
  // happen.
  const readAt =
    (bare ? historyQuery.dataUpdatedAt : statusQuery.dataUpdatedAt) || null;
  const readFailure =
    projectsQuery.error ??
    openedQuery.error ??
    (opened ? (bare ? historyQuery.error : statusQuery.error) : null);
  // The open checkout is gone from the server: its folder was deleted. For a
  // linked worktree that's routine (`git worktree remove`, or rm -rf), so the
  // page moves to the project's main worktree by itself and says why; the
  // worktree picker then lists the missing one for removal. If the project's
  // own folder is gone, there's nothing to move to.
  const checkoutGone =
    !!opened &&
    gitErrorCode(bare ? historyQuery.error : statusQuery.error) ===
      "REPO_NOT_FOUND";
  // Another of the project's worktrees, not the bookmarked checkout itself:
  // the bookmark's folder is where the page can go back to.
  const linkedWorktree =
    !!opened &&
    opened.repository.root.bytesB64 !== opened.project.path.bytesB64;
  const [goneWorktree, setGoneWorktree] = useState<{
    name: string;
    path: string;
  } | null>(null);
  const movedFrom = useRef<string | null>(null);
  const gonePath = useRef<{ name: string; path: string } | null>(null);
  // A newly resolved descriptor applies the selection restore exactly once:
  // coming back to the checkout the selections were made in keeps them,
  // any other checkout clears them. Refetches keep the same repository id
  // and must never disturb a selection made meanwhile.
  const processedSelection = useRef<string | null>(null);
  useEffect(() => {
    const data = openedQuery.data;
    if (!selection || !data || openedQuery.isPlaceholderData) return;
    const key = `${selection.project.id}:${selection.path.bytesB64}@${data.repository.repoId}`;
    if (processedSelection.current === key) return;
    processedSelection.current = key;
    const checkout = `${selection.project.id}:${data.repository.root.bytesB64}`;
    patchGitState(server.id, {
      ...selectionArrivalPatch(
        readGitState(server.id).selectionProject,
        checkout,
        data.repository.bare,
      ),
    });
    if (movedFrom.current) {
      movedFrom.current = null;
      const gone = gonePath.current;
      gonePath.current = null;
      if (gone) setGoneWorktree(gone);
    }
    // Runs once per newly opened checkout; refs carry the rest.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selection, openedQuery.data, openedQuery.isPlaceholderData]);
  useEffect(() => {
    if (!checkoutGone || !linkedWorktree || !opened || !selection) return;
    if (movedFrom.current === repoId) return;
    movedFrom.current = repoId;
    const path = opened.repository.root.display;
    gonePath.current = {
      name: path.split("/").filter(Boolean).pop() ?? path,
      path,
    };
    // Back to the bookmark; the arrival effect above announces the move
    // once the main checkout resolves.
    stopChecks();
    setGoneWorktree(null);
    setNewWorktree(false);
    setWorktreeEdit(undefined);
    patchGitState(server.id, {
      opened: { project: selection.project, path: selection.project.path },
    });
    // Once per vanished checkout; refs carry the rest.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [checkoutGone, linkedWorktree, repoId]);
  // A failed read reaches the same places an action's failure does, as it did
  // when reads ran inside actions.
  const error =
    actionError ||
    (checkoutGone && linkedWorktree
      ? ""
      : checkoutGone && opened
        ? `This project’s folder is gone from the server: ${opened.repository.root.display}`
        : readFailure
          ? message(readFailure)
          : "");

  const inspectedCommit = history?.entries.find(
    (commit) => commit.oid.hex === selectedCommit,
  );
  const statusFiltering =
    filterPending || statusQuery.isPlaceholderData || statusQuery.isError;
  const previewEntry = !statusFiltering
    ? status?.entries.find((entry) => entry.entryId === selectedEntry)
    : undefined;
  // Only what the user asked to read: a library row is read when its "Check
  // status" is pressed, never on sight, because each read opens a repository.
  const { libraryStatus, libraryChecking } = useQueries({
    queries: projects.map((project) => ({
      ...gitQueries.summary(scope, project),
      enabled: false,
    })),
    combine: (results) => ({
      libraryStatus: Object.fromEntries(
        results.flatMap((result, index) =>
          result.data && projects[index]
            ? [[projects[index].id, result.data]]
            : [],
        ),
      ) as Record<string, GitProjectRowStatus>,
      libraryChecking: new Set(
        results.flatMap((result, index) =>
          result.fetchStatus === "fetching" && projects[index]
            ? [projects[index].id]
            : [],
        ),
      ),
    }),
  });

  const checkRun = useRef(0);
  const navigate = useNavigate();
  // Files keeps its location in workspace state, so seeding it and navigating
  // opens the browser exactly where the repository lives. One patch: the
  // path and its input always change together.
  function openInFiles(path: string) {
    patchWorkspaceValue(server.id, { "files.path": path, "files.input": path });
    void navigate(`/servers/${encodeURIComponent(server.id)}/files`);
  }
  const now = useNow(30_000);

  /*
   * Every action the user starts runs through here, one at a time, as a Query
   * mutation. The agent serves one request at a time anyway; what this adds is
   * that the page is busy while an action is in flight, and that a second one
   * is refused rather than queued behind it. The mutation key is shared, so a
   * page remounted mid-action still sees it as running.
   */
  const actionKey = [...gitKeys.all(scope), "action"] as const;
  const action = useMutation({
    mutationKey: actionKey,
    mutationFn: (task: (projects: GitProjects) => Promise<void>) =>
      task(gitProjectsFor(scope)),
  });
  const acting = useIsMutating({ mutationKey: actionKey }) > 0;
  // The descriptor resolving counts as busy too: until it lands there is no
  // repository to write to, as while the open ran inside the action.
  const openedLoading = !!selection && openedQuery.fetchStatus !== "idle";
  const busy =
    acting ||
    openedLoading ||
    projectsQuery.isPending ||
    (receiptsQuery.isPending && receiptsQuery.fetchStatus !== "idle");
  /** Whether a session is still the one this page's connection is using. */
  const isCurrent = (projects: GitProjects) =>
    gitResources.get(connection) === projects;

  async function run(task: (projects: GitProjects) => Promise<void>) {
    // Consult the cache, not the render: a second click can land before React
    // has drawn the first action as pending.
    if (queryClient.isMutating({ mutationKey: actionKey })) return false;
    setError("");
    try {
      await action.mutateAsync(task);
      return true;
    } catch (reason) {
      setError(message(reason));
      return false;
    }
  }
  async function refreshReceipts() {
    // The failure is shown by the receipts query itself; see recoveryError.
    await refreshQuery(queryClient, gitQueries.receipts(scope)).catch(
      () => undefined,
    );
  }
  /**
   * Reads one bookmark's Git state because the user asked for that row. Kept
   * off `run` deliberately: a per-row read must not lock the whole library, and
   * several rows may be resolving at once. The summary query opens, reads and
   * closes inside the one read, so the library holds nothing open.
   */
  async function checkLibraryStatus(project: GitProject) {
    try {
      await queryClient.fetchQuery({
        ...gitQueries.summary(scope, project),
        staleTime: 0,
      });
    } catch (reason) {
      // A read given up on -- its row left the screen -- is not a failure.
      if (isCancelledError(reason)) return;
      setError(`Cannot read ${project.name}: ${message(reason)}`);
    }
  }
  /*
   * Which "Check status" run is current. Reads go one at a time on the
   * server's one connection, so a run over a hundred agent worktrees would
   * hold everything else behind it -- opening a project would wait for the
   * whole run. Opening anything, refreshing, or leaving the page starts a new
   * generation, and a run that is no longer current stops before its next
   * read.
   */
  const stopChecks = () => {
    checkRun.current += 1;
  };
  useEffect(() => stopChecks, []);
  /**
   * Reads every row the library is showing, one at a time on the server's
   * one connection, because the user asked for all of them. Rows already read
   * are read again: "check" means now.
   */
  async function checkAllLibraryStatus(shown: GitProject[]) {
    const run = ++checkRun.current;
    for (const project of shown) {
      if (run !== checkRun.current) return;
      await checkLibraryStatus(project);
    }
  }
  /**
   * Names the open checkout: a bookmark plus the checkout path. The
   * descriptor resolves through the `opened` query behind the previous
   * view, so a failed open leaves the page where it was; the arrival
   * effect above applies the selection restore once it resolves.
   *
   * Selections belong to one checkout: a project's worktrees share its
   * bookmark but not its files, so the restore key is the checkout's root
   * as well (resolved in the arrival effect, which also clears a return
   * to a file that has since gone by re-resolving its entry id).
   */
  function selectCheckout(project: GitProject, path: GitPath) {
    stopChecks();
    setGoneWorktree(null);
    setNewWorktree(false);
    setWorktreeEdit(undefined);
    setError("");
    // Force the arrival effect below to run for this choice, even when it
    // names the checkout already showing: reopening resets the tab and the
    // composer exactly as opening inside the action did.
    processedSelection.current = null;
    // A failed open stays cached as an error under an infinite stale time,
    // so reselecting it must retire that failure or the retry reads nothing.
    // (Unlike `show`, which re-ran the RPC on every click.)
    if (
      queryClient.getQueryState(gitKeys.opened(scope, project.id, path))
        ?.status === "error"
    ) {
      void queryClient.invalidateQueries({
        queryKey: gitKeys.opened(scope, project.id, path),
      });
    }
    patchGitState(server.id, { opened: { project, path } });
  }
  function openProject(project: GitProject) {
    // Open the new one before letting go of the old: the previous
    // descriptor stays on screen (placeholder plus last good) until the
    // new one resolves, so a failed open leaves the page where it was.
    selectCheckout(project, project.path);
  }
  /**
   * Makes one of the project's worktrees the page's workspace. The bookmark
   * stays the project's; only the checkout changes, so the repository picker
   * keeps its name and the worktree picker says which checkout this is.
   */
  function openCheckout(path: GitPath) {
    if (!selection) return;
    selectCheckout(selection.project, path);
  }
  function openWorktree(row: WorktreeRow) {
    if (!row.path) return;
    openCheckout(row.path);
  }
  /** Back to the library. Nothing is held open, so there is nothing to close. */
  function leaveRepository() {
    lastGood.current = null;
    processedSelection.current = null;
    patchGitState(server.id, { opened: null });
  }
  /**
   * After a connection failure the user chose to reset: drop this server's
   * connection so the next request starts on a fresh one. Repository ids
   * are stateless and content-addressed reads stay cached; the selection
   * survives and its descriptor re-resolves on the new connection.
   */
  function resetConnection() {
    resetGitProjects(scope);
    lastGood.current = null;
    processedSelection.current = null;
    patchGitState(server.id, { opened: null });
  }
  function upsertProject(project: GitProject) {
    queryClient.setQueryData(
      gitQueries.projects(scope).queryKey,
      (current = []) =>
        current.some((each) => each.id === project.id)
          ? current.map((each) => (each.id === project.id ? project : each))
          : [...current, project],
    );
  }

  async function write(action: GitWriteAction, expectedSnapshot?: string) {
    if (!opened || !selection) return false;
    // The descriptor's own query key, for patching its head after the
    // write. The store selection names it; the descriptor carries the data.
    const openedKey = gitQueries.opened(
      scope,
      selection.project,
      selection.path,
    ).queryKey;
    const snapshot = action.kind.startsWith("worktree.")
      ? expectedSnapshot
      : (expectedSnapshot ?? status?.snapshot);
    if (!snapshot) return false;
    const current = opened;
    const id = current.repository.repoId;
    return run(async (projects) => {
      try {
        const operationId = crypto.randomUUID();
        setOperationRepositories((previous) => ({
          ...previous,
          [operationId]: `${current.project.name} · ${current.project.path.display}`,
        }));
        const outcome = await projects.mutations.start({
          operationId,
          repoId: id,
          expectedSnapshot: snapshot,
          action,
        });
        if (!isCurrent(projects)) return;
        if (
          outcome.state !== "succeeded" &&
          outcome.state !== "needs_resolution" &&
          outcome.state !== "failed"
        )
          throw new Error(
            "The operation’s outcome is not confirmed. Check its saved outcome before repeating it.",
          );
        if (action.kind === "commit" && outcome.state === "succeeded") {
          patchGitState(server.id, {
            commitSummary: "",
            commitDescription: "",
          });
        }
        try {
          /*
           * The order here is the safety property, not a detail. The saved
           * outcome is acknowledged only once the repository's new state is
           * in hand, so status is re-read first, in full, bypassing the cache;
           * a failed re-read leaves the outcome unacknowledged for the user to
           * check. Everything else the write made stale is invalidated after.
           */
          const next = current.repository.bare
            ? null
            : await refreshQuery(
                queryClient,
                gitQueries.status(scope, id, statusFilter),
              );
          if (!isCurrent(projects)) return;
          const retainSelection =
            ("hunks" in action && !!action.hunks) ||
            action.kind === "conflict.resolve";
          // The entry and the refreshed head change together: one update,
          // so the page never renders the new head with the old selection.
          const retainedEntry =
            retainSelection && previewEntry?.path
              ? (next?.entries.find(
                  (entry) =>
                    entry.path?.bytesB64 === previewEntry.path?.bytesB64,
                )?.entryId ?? null)
              : null;
          patchGitState(server.id, { selectedEntry: retainedEntry });
          if (next) {
            // The descriptor is query-owned now: patch its head in the
            // cache, so a concurrent refresh cannot clobber it with the
            // pre-write read. The selection itself is untouched.
            queryClient.setQueryData(
              openedKey,
              (current: Opened | undefined) =>
                current
                  ? {
                      ...current,
                      repository: {
                        ...current.repository,
                        head: next.metadata.head,
                      },
                    }
                  : current,
            );
          }
          const rereadHistory = tab === "history" || current.repository.bare;
          if (rereadHistory)
            await refreshQuery(queryClient, gitQueries.history(scope, id));
          if (!isCurrent(projects)) return;
          void invalidateRepository(queryClient, scope, id, [
            gitKeys.status(scope, id, statusFilter),
            ...(rereadHistory ? ["history"] : []),
          ]);
          if (outcome.state !== "failed") {
            await projects.mutations.acknowledge(outcome.operationId);
            // The receipt is gone: its repository name can go with it.
            forgetOperationRepository(outcome.operationId);
          }
        } catch (reason) {
          throw new Error(
            `The saved operation outcome is ${outcome.state.replaceAll("_", " ")}, but refreshing or dismissing it failed: ${message(reason)}`,
            { cause: reason },
          );
        }
        if (outcome.state === "failed")
          throw new Error(
            outcome.error?.message ?? "The Git operation failed.",
          );
        if (outcome.state === "needs_resolution")
          toast.info(
            action.kind === "stash.apply" || action.kind === "stash.pop"
              ? "Resolve the conflicts and stage the resolutions. The stash was kept."
              : "Resolve the conflicts, stage the resolutions, then continue the operation.",
          );
        else {
          // Name what was actually edited; never imply the whole file moved.
          const subject =
            "hunks" in action && action.hunks
              ? action.hunks.lines
                ? action.hunks.lines.length === 1
                  ? "Line"
                  : "Lines"
                : "Hunk"
              : "File";
          toast.success(
            action.kind === "commit"
              ? "Changes committed."
              : action.kind === "stage"
                ? `${subject} staged.`
                : action.kind === "unstage"
                  ? `${subject} unstaged.`
                  : action.kind === "discard"
                    ? `${subject} discarded.`
                    : action.kind === "conflict.resolve"
                      ? "Conflict resolved and staged."
                      : action.kind === "worktree.add"
                        ? `Worktree ${action.name} created${action.newBranch ? ` on new branch ${action.branch}` : ` on ${action.branch}`}.`
                        : "Git operation completed.",
          );
        }
      } finally {
        await refreshReceipts();
      }
    });
  }
  async function checkOutcome(id: string) {
    await run(async (projects) => {
      // The page's own connection: an outcome lookup reads the durable journal,
      // and no request depends on anything a connection holds.
      try {
        const outcome = await projects.mutations.check(id);
        if (isCurrent(projects)) {
          if (outcome.state === "succeeded")
            toast.success(
              "The saved operation completed. Reopen the repository to see its changes.",
            );
          else if (outcome.state === "reviewed_unknown")
            toast.info(
              "This interruption was reviewed. Its original outcome remains unknown.",
            );
          else
            setError(
              outcome.error?.message ??
                `Operation outcome: ${outcome.state.replaceAll("_", " ")}.`,
            );
        }
      } catch (reason) {
        // The agent records an operation before running it, so no record means
        // the write never began. The receipt is resolved rather than left
        // blocking every later write.
        if (
          reason &&
          typeof reason === "object" &&
          "code" in reason &&
          reason.code === "OPERATION_NOT_FOUND"
        ) {
          if (isCurrent(projects))
            toast.success(
              "The operation never started on the server, so nothing was changed. Dismiss the saved outcome to continue.",
            );
        } else throw reason;
      } finally {
        await refreshReceipts();
      }
    });
  }
  async function createProject(request: GitBootstrapRequest, name: string) {
    return run(async (projects) => {
      setOperationRepositories((previous) => ({
        ...previous,
        [request.params.operationId]:
          `${name} · ${request.params.path.display}`,
      }));
      try {
        const result = await projects.bootstrap(request, name);
        if (!isCurrent(projects)) return;
        if (!result.selection)
          throw new Error(
            result.operation.error?.message ??
              "Creation is not confirmed. Check its saved outcome before trying again.",
          );
        upsertProject(result.selection.project);
        setCreating(null);
        toast.success(
          request.method === "repo.clone"
            ? "Repository cloned."
            : "Repository created.",
        );
        selectCheckout(result.selection.project, result.selection.project.path);
        if (isCurrent(projects)) {
          await projects.mutations.acknowledge(result.operation.operationId);
          forgetOperationRepository(result.operation.operationId);
        }
      } catch (reason) {
        if (reason instanceof GitProjectCreatedError && isCurrent(projects)) {
          // Recovery only adds a bookmark. The successful creation is never replayed.
          setCreating(null);
          setName(name);
          setPath(reason.requestedPath.display);
          setEditor("add");
        }
        throw reason;
      } finally {
        await refreshReceipts();
      }
    });
  }
  /** Re-reads whatever the page is showing, bypassing the cache. */
  function refreshShown() {
    return run(async () => {
      if (!opened) await refreshQuery(queryClient, gitQueries.projects(scope));
      else if (tab === "history" || bare)
        await refreshQuery(queryClient, gitQueries.history(scope, repoId));
      else
        await refreshQuery(
          queryClient,
          gitQueries.status(scope, repoId, statusFilter),
        );
    });
  }
  const statusPages = useGitPageLoader({
    prefetch: true,
    queryKey: gitQueries.status(scope, repoId, statusFilter).queryKey,
    page: status,
    enabled: !!opened && !busy && !statusFiltering && tab === "changes",
    read: (cursor, signal) =>
      gitProjectsFor(scope)
        .repositories.withSignal(signal)
        .status(repoId, cursor, statusFilter),
    entryKey: (entry) => entry.entryId,
  });
  const historyPages = useGitPageLoader({
    prefetch: true,
    queryKey: gitQueries.history(scope, repoId).queryKey,
    page: history,
    enabled: !!opened && !busy && tab === "history",
    read: (cursor, signal) =>
      gitProjectsFor(scope)
        .repositories.withSignal(signal)
        .history(repoId, "HEAD", cursor),
    entryKey: (entry) => `${entry.oid.algorithm}:${entry.oid.hex}`,
  });
  const hasSavedOutcome =
    receipts?.some(
      (receipt) =>
        receipt.state === "pending" || receipt.state === "outcome_unknown",
    ) ?? false;
  const branchBlockedReason =
    receipts === null
      ? "Saved outcomes are unavailable. Refresh them before making changes."
      : opened?.repository.capabilities.readOnly
        ? "This repository is read-only."
        : opened?.repository.bare
          ? "Branch changes in bare repositories are not available yet."
          : !status
            ? "Refresh the repository status before changing branches."
            : undefined;

  const readOnly = opened?.repository.capabilities.readOnly ?? false;
  const worktreeBlockedReason =
    receipts === null
      ? "Saved outcomes are unavailable. Refresh them before making changes."
      : readOnly
        ? "This repository is read-only."
        : undefined;
  // Whether the index may be written right now: the file and group actions in
  // the list, and the file actions in the diff header, all follow this.
  const indexWritable =
    !!opened && !readOnly && !opened.repository.bare && receipts !== null;
  const clean = cleanTree;
  const branchName = status ? headBranch(status) : null;
  const selectedGroup =
    previewEntry &&
    changeGroups(status?.entries ?? [], "", selectedSide).some((group) =>
      group.entries.some((entry) => entry.entryId === previewEntry.entryId),
    )
      ? selectedSide
      : previewEntry
        ? (changeGroups([previewEntry], "")[0]?.key ?? selectedSide)
        : selectedSide;

  const fullPageTool =
    !newWorktree &&
    (actionsPanel === "tags" ||
      actionsPanel === "stashes" ||
      actionsPanel === "remotes");

  return (
    <section
      className="git-projects @container/git-project flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden text-foreground"
      aria-label="Git projects"
      aria-busy={busy}
    >
      {opened && (
        /* One compact bar, as tall as Files' toolbar: where you are on the
           left (repository, branch), what you can do on the right (transfer,
           refresh, everything else). */
        <header className="git-projects-toolbar flex h-[46px] flex-none items-center gap-[4px] border-b border-border pr-[12px] pl-[12px]">
          {/* The picker is the control; the heading keeps the document structure. */}
          <h2 className="sr-only">{opened.project.name}</h2>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <GitPicker
                disabled={busy}
                aria-label={`Current repository: ${opened.project.name}`}
                icon={<FolderGit2 size={15} />}
                label="Current repository"
                value={opened.project.name}
                valueTitle={opened.project.name}
              />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="min-w-[220px]">
              <DropdownMenuItem
                disabled={busy}
                onSelect={() => void run(async () => leaveRepository())}
              >
                <LayoutList size={14} aria-hidden="true" />
                All projects
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              {projects.map((project) => (
                <DropdownMenuItem
                  key={project.id}
                  disabled={busy || project.id === opened.project.id}
                  onSelect={() => openProject(project)}
                >
                  <FolderGit2 size={14} aria-hidden="true" />
                  {project.name}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
          <span
            className="mx-[2px] h-[16px] w-px flex-none bg-border"
            aria-hidden="true"
          />
          <GitWorktreePicker
            project={opened.project}
            repository={opened.repository}
            busy={busy}
            onOpen={openWorktree}
            onNew={() => {
              setActionsPanel(null);
              setNewWorktree(true);
            }}
            onManage={(edit) => {
              setWorktreeEdit(edit);
              setActionsPanel("worktrees");
            }}
            onFiles={openInFiles}
            currentSummary={
              status
                ? {
                    changes:
                      status.metadata.totalEntries ??
                      (status.nextCursor || status.metadata.truncated
                        ? null
                        : status.entries.length),
                    outgoing: status.metadata.ahead ?? null,
                  }
                : undefined
            }
          />
          <div className="git-repository-meta flex min-w-0 flex-[0_1_auto]">
            <GitBranchControls
              repository={{
                ...opened.repository,
                head: status?.metadata.head ?? opened.repository.head,
              }}
              snapshot={status?.snapshot}
              busy={busy}
              writable={
                !!status &&
                !opened.repository.capabilities.readOnly &&
                !opened.repository.bare &&
                receipts !== null
              }
              blockedReason={branchBlockedReason}
              recoveryAvailable={hasSavedOutcome || receipts === null}
              error={error}
              onAction={write}
              worktrees={{
                openIfHeld: async (branch, isCurrent) => {
                  const page = await queryClient.fetchQuery({
                    ...gitQueries.worktrees(scope, repoId, undefined, {
                      branch,
                      pageSize: 2,
                    }),
                    staleTime: 0,
                  });
                  if (!isCurrent()) return false;
                  const row = page.entries.find(
                    (row) => !row.current && row.head?.name?.display === branch,
                  );
                  if (!row) return false;
                  if (!openable(row))
                    throw new Error(
                      `The worktree holding ${branch.replace(/^refs\/heads\//, "")} is ${row.state}. Open Worktrees to inspect or repair it.`,
                    );
                  openWorktree(row);
                  return true;
                },
              }}
            />
          </div>
          <div className="git-projects-actions ml-auto flex flex-none items-center gap-[6px] pl-[8px]">
            <GitSyncControl
              repository={{
                ...opened.repository,
                head: status?.metadata.head ?? opened.repository.head,
              }}
              status={status}
              busy={busy}
              blockedReason={branchBlockedReason
                ?.replace("changing branches", "transferring commits")
                .replace("Branch changes", "Transfers")}
              onAction={write}
            />
            <Button
              size="icon"
              aria-label="Refresh"
              disabled={busy}
              className="h-[28px] w-[28px]"
              onClick={() => void refreshShown()}
            >
              <RefreshCw size={14} aria-hidden="true" />
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  size="icon"
                  aria-label="Git actions"
                  disabled={busy}
                  className="h-[28px] w-[28px]"
                >
                  <Ellipsis size={15} aria-hidden="true" />
                </Button>
              </DropdownMenuTrigger>
              {/* Repository tools open in the inspector; worktree management
                  stays in the worktree picker. */}
              <DropdownMenuContent
                align="end"
                className="git-actions-menu min-w-[200px]"
              >
                <DropdownMenuItem
                  disabled={busy}
                  onSelect={() => setActionsPanel("stashes")}
                >
                  <Archive size={14} aria-hidden="true" />
                  Stashes
                </DropdownMenuItem>
                <DropdownMenuItem
                  disabled={busy}
                  onSelect={() => setActionsPanel("tags")}
                >
                  <Tag size={14} aria-hidden="true" />
                  Tags
                </DropdownMenuItem>
                <DropdownMenuItem
                  disabled={busy}
                  onSelect={() => setActionsPanel("remotes")}
                >
                  <Globe size={14} aria-hidden="true" />
                  Remotes
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onSelect={() => openInFiles(opened.repository.root.display)}
                >
                  <FolderOpen size={14} aria-hidden="true" />
                  Open in Files
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </header>
      )}
      <GitFolderChooser
        serverId={server.id}
        open={browsing}
        initialPath={path || undefined}
        title="Choose a repository folder"
        busy={busy}
        onCancel={() => setBrowsing(false)}
        onChoose={(chosen) => {
          // Saving resolves a subdirectory to its repository root, so the
          // chosen path is a starting point rather than a claim about one.
          setPath(chosen);
          setBrowsing(false);
        }}
      />
      <GitRecoveryPanel
        receipts={receipts}
        repositories={operationRepositories}
        error={recoveryError}
        busy={busy}
        onCheck={(id) => void checkOutcome(id)}
        onReview={(id) =>
          void run(async (projects) => {
            await projects.mutations.review(id);
            await refreshReceipts();
            if (opened && !bare)
              await refreshQuery(
                queryClient,
                gitQueries.status(scope, repoId, statusFilter),
              );
            toast.info(
              "Review recorded. The original outcome remains unknown.",
            );
          })
        }
        onAcknowledge={(id) =>
          void run(async (projects) => {
            await projects.mutations.acknowledge(id);
            forgetOperationRepository(id);
            await refreshReceipts();
          })
        }
        onRefresh={() => void run(refreshReceipts)}
      />
      {error && (
        <GitNotice
          tone="error"
          className={opened ? "mx-[12px] my-[8px]" : "mx-[22px] mt-[18px] mb-0"}
        >
          {error}
          {!opened && (
            <Button
              disabled={busy}
              onClick={() => void run(async () => resetConnection())}
            >
              Reset connection
            </Button>
          )}
          {checkoutGone && !linkedWorktree && (
            <Button disabled={busy} onClick={leaveRepository}>
              Back to projects
            </Button>
          )}
        </GitNotice>
      )}
      {opened && checkoutGone && linkedWorktree && (
        <GitNotice tone="info" className="mx-[12px] my-[8px]">
          This worktree was removed from the server. Opening the main worktree…
        </GitNotice>
      )}
      {goneWorktree && (
        <GitNotice tone="info" className="mx-[12px] my-[8px]">
          <span className="min-w-0 flex-1">
            The worktree <strong>{goneWorktree.name}</strong> was removed from
            the server ({goneWorktree.path}), so the main worktree is open. Git
            still lists it as missing under Worktrees until its registration is
            removed.
          </span>
          <Button variant="ghost" onClick={() => setGoneWorktree(null)}>
            Dismiss
          </Button>
        </GitNotice>
      )}
      {!opened ? (
        <GitProjectLibrary
          projects={projects}
          busy={busy}
          search={search}
          onSearch={setSearch}
          filter={libraryFilter}
          onFilter={setLibraryFilter}
          sort={librarySort}
          onSort={setLibrarySort}
          favourites={favourites}
          onToggleFavourite={(id) =>
            setFavourites((current) => {
              const next = new Set(current);
              if (next.has(id)) next.delete(id);
              else next.add(id);
              return next;
            })
          }
          onOpen={(project) => {
            // The row's cached read is about to be superseded by the live
            // one, and would otherwise be shown again, stale, on return.
            queryClient.removeQueries({
              queryKey: gitKeys.summary(scope, project.id),
            });
            openProject(project);
          }}
          onEdit={(project) => {
            setName(project.name);
            setPath(project.path.display);
            setEditor(project);
          }}
          onRemove={(project) => setRemoving(project)}
          onRefresh={() => {
            stopChecks();
            // Refresh means "read again", so nothing read earlier survives it.
            queryClient.removeQueries({ queryKey: gitKeys.summaries(scope) });
            void run(async () => {
              await refreshQuery(queryClient, gitQueries.projects(scope));
            });
          }}
          onAdd={() => {
            setName("");
            setPath("");
            setEditor("add");
          }}
          onClone={() => {
            setError("");
            setCreating("clone");
          }}
          onCreate={() => {
            setError("");
            setCreating("init");
          }}
          status={libraryStatus}
          checking={libraryChecking}
          onCheck={(project) => void checkLibraryStatus(project)}
          onCheckAll={(shown) => void checkAllLibraryStatus(shown)}
        />
      ) : (
        <>
          {status && (
            <GitIntegrationControls
              status={status}
              busy={busy}
              disabled={busy || !!branchBlockedReason}
              error={error}
              onAction={write}
            />
          )}
          <div
            className="git-repository-workspace group/repository flex min-h-0 flex-1"
            data-inspector={!!actionsPanel || newWorktree}
            data-full-page={fullPageTool}
          >
            <Tabs
              className={cn(
                "git-repository-tabs min-w-0 flex min-h-0 flex-1 flex-col",
                fullPageTool
                  ? "hidden"
                  : (actionsPanel || newWorktree) &&
                      "@max-[760px]/git-project:hidden",
              )}
              value={tab}
              // Choosing a tab is all this does: the tab's read is enabled by it,
              // and runs only if nothing fresh is cached.
              onValueChange={setTab}
            >
              {/* One split for both views, so History gets the same resizable
                list, the same narrow single-pane flow, and the same header
                line as Changes. The tabs head the list column -- they choose
                what the list shows -- and stay mounted across the switch, so
                a keyboard user's focus is never on something that unmounts. */}
              <GitChangesSplit
                className="min-h-0 flex-1 overflow-hidden"
                listClassName="git-changes-files flex min-h-0 flex-col overflow-hidden"
                detailClassName="git-changes-detail h-full min-h-0 overflow-y-auto"
                detailTitle={
                  tab === "changes"
                    ? previewEntry
                      ? (previewEntry.path?.display ??
                        previewEntry.oldPath?.display ??
                        "Selected file")
                      : undefined
                    : inspectedCommit
                      ? inspectedCommit.message.display.split("\n")[0]
                      : undefined
                }
                detailKey={
                  tab === "changes"
                    ? previewEntry?.entryId
                    : (inspectedCommit?.oid.hex ?? undefined)
                }
                list={
                  <>
                    <div className="flex h-[46px] flex-none items-center border-b border-border px-[8px]">
                      {/* `flex-row!`: shadcn keys `flex-col` off
                        `group-data-vertical/tabs`, a global group marker,
                        and this app's sidebar is itself a vertical Tabs. */}
                      <TabsList
                        aria-label="Repository view"
                        className="h-[30px]! w-full flex-row! gap-[2px] rounded-[7px] bg-(--native-toolbar) p-[2px]"
                      >
                        <TabsTrigger
                          value="changes"
                          disabled={busy || opened.repository.bare}
                          className="h-[26px] flex-1 gap-[6px] rounded-[5px] text-[12px] font-medium text-muted-foreground data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-[0_1px_2px_rgb(0_0_0/0.1)]"
                        >
                          Changes
                          {!!(
                            status?.metadata.totalEntries ??
                            status?.entries.length
                          ) && (
                            <span
                              aria-hidden="true"
                              className="min-w-[18px] rounded-full bg-[color-mix(in_srgb,var(--foreground)_9%,transparent)] px-[5px] text-[10px] leading-[16px] font-semibold tabular-nums"
                            >
                              {status.metadata.totalEntries?.toLocaleString() ??
                                (status.nextCursor || status.metadata.truncated
                                  ? `${status.entries.length}+`
                                  : status.entries.length)}
                            </span>
                          )}
                        </TabsTrigger>
                        <TabsTrigger
                          value="history"
                          disabled={busy}
                          className="h-[26px] flex-1 rounded-[5px] text-[12px] font-medium text-muted-foreground data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-[0_1px_2px_rgb(0_0_0/0.1)]"
                        >
                          History
                        </TabsTrigger>
                      </TabsList>
                    </div>
                    <TabsContent
                      value="changes"
                      className="flex min-h-0 flex-1 flex-col"
                    >
                      {statusQuery.isFetching && !status ? (
                        <GitEmptyState role="status">
                          Loading changes…
                        </GitEmptyState>
                      ) : status &&
                        clean &&
                        !fileFilter &&
                        groupFilter === "all" ? (
                        <GitEmptyState className="text-[12px]">
                          No changed files
                        </GitEmptyState>
                      ) : status ? (
                        <>
                          <GitChangeList
                            status={status}
                            filter={fileFilter}
                            onFilter={setFileFilter}
                            group={groupFilter}
                            onGroup={setGroupFilter}
                            selectedEntry={selectedEntry}
                            selectedSide={selectedSide}
                            onSelect={(entryId, group) => {
                              // Entry and side change together: the group
                              // names which comparison the entry was
                              // chosen under. One update, never split.
                              patchGitState(server.id, {
                                selectedEntry: entryId,
                                selectedSide: group,
                              });
                            }}
                            writable={indexWritable}
                            busy={busy}
                            onStage={(entryIds) =>
                              void write({ kind: "stage", entryIds })
                            }
                            onUnstage={(entryIds) =>
                              void write({ kind: "unstage", entryIds })
                            }
                            onLoadMore={statusPages.load}
                            filterLoading={
                              statusFiltering && !statusQuery.isError
                            }
                            filterError={
                              statusQuery.isError
                                ? message(statusQuery.error)
                                : ""
                            }
                            onRetryFilter={() => void statusQuery.refetch()}
                            pageLoading={statusPages.loading}
                            pageError={statusPages.error}
                          />
                          <GitCommitComposer
                            status={status}
                            branch={
                              (
                                status.metadata.head ?? opened.repository.head
                              ).name?.display.replace(/^refs\/heads\//, "") ??
                              null
                            }
                            summary={commitSummary}
                            description={commitDescription}
                            disabled={busy || !indexWritable}
                            onSummary={setCommitSummary}
                            onDescription={setCommitDescription}
                            onAction={write}
                          />
                        </>
                      ) : null}
                    </TabsContent>
                    <TabsContent
                      value="history"
                      className="flex min-h-0 flex-1 flex-col"
                    >
                      {historyQuery.isFetching && !history ? (
                        <GitEmptyState role="status">
                          Loading history…
                        </GitEmptyState>
                      ) : history ? (
                        <div className="git-history-list flex min-h-0 flex-1 flex-col overflow-y-auto">
                          {history.entries.length === 0 ? (
                            <GitEmptyState>No commits yet.</GitEmptyState>
                          ) : (
                            <ul className="git-history-commits m-0 list-none px-[6px] py-[6px]">
                              {history.entries.map((commit) => {
                                const chosen =
                                  selectedCommit === commit.oid.hex;
                                const when = new Date(commit.time * 1000);
                                return (
                                  <li key={commit.oid.hex}>
                                    <Button
                                      variant="ghost"
                                      aria-pressed={chosen}
                                      onClick={() =>
                                        setSelectedCommit(commit.oid.hex)
                                      }
                                      className={cn(
                                        "h-auto w-full flex-col items-stretch gap-[2px] rounded-[5px] border-0 px-[10px] py-[6px] text-left font-normal text-foreground",
                                        chosen
                                          ? "bg-(--git-tint) hover:bg-(--git-tint)"
                                          : "hover:bg-accent",
                                      )}
                                    >
                                      <span className="truncate text-[12px] leading-[17px] font-medium">
                                        {commit.message.display.split(
                                          "\n",
                                        )[0] || "Empty commit message"}
                                      </span>
                                      <span className="flex min-w-0 items-center gap-[5px] text-[11px] leading-[15px] text-muted-foreground">
                                        <span className="truncate">
                                          {commit.author.name}
                                        </span>
                                        <span aria-hidden="true">·</span>
                                        <time
                                          className="flex-none"
                                          title={
                                            Number.isNaN(when.getTime())
                                              ? undefined
                                              : when.toLocaleString()
                                          }
                                        >
                                          {relativeTime(commit.time, now)}
                                        </time>
                                        <code className="ml-auto flex-none pl-[6px] text-[10.5px]">
                                          {commit.oid.hex.slice(0, 7)}
                                        </code>
                                      </span>
                                    </Button>
                                  </li>
                                );
                              })}
                            </ul>
                          )}
                          {history.metadata.truncated && (
                            <GitNotice tone="info">
                              History exceeds the agent’s listing limit. This is
                              a partial history.
                            </GitNotice>
                          )}
                          <GitLoadMore
                            cursor={history.nextCursor}
                            loading={historyPages.loading}
                            error={historyPages.error}
                            disabled={busy}
                            onLoad={historyPages.load}
                            label="Load more commits"
                            endLabel={
                              history.metadata.truncated
                                ? "End of available history"
                                : "End of history"
                            }
                          />
                        </div>
                      ) : null}
                    </TabsContent>
                  </>
                }
                detail={
                  tab === "changes" ? (
                    !status ? null : clean ? (
                      <CleanWorkingTree
                        branch={branchName}
                        head={status.metadata.head.oid?.hex ?? null}
                        latest={history?.entries[0] ?? null}
                        now={now}
                        busy={busy}
                        onHistory={() => setTab("history")}
                        onFiles={() =>
                          openInFiles(opened.repository.root.display)
                        }
                      />
                    ) : previewEntry ? (
                      <GitChangesPreview
                        key={`${status.snapshot}:${previewEntry.entryId}`}
                        repoId={opened.repository.repoId}
                        snapshot={status.snapshot}
                        entry={previewEntry}
                        mark={changeMark(previewEntry, selectedGroup)}
                        preferredSide={
                          selectedSide === "staged"
                            ? "head_to_index"
                            : "index_to_worktree"
                        }
                        disabled={busy || !!branchBlockedReason}
                        blockedReason={
                          status.metadata.integration ||
                          (status.metadata.groupCounts?.conflicted ??
                            status.entries.filter((entry) => entry.conflicted)
                              .length) > 0
                            ? "Resolve conflicts and finish any integration before staging individual hunks."
                            : branchBlockedReason
                        }
                        conflictBlockedReason={branchBlockedReason}
                        actions={
                          <GitFileActions
                            entry={previewEntry}
                            status={status}
                            busy={busy}
                            error={error}
                            compact
                            disabled={busy || !indexWritable}
                            onAction={write}
                          />
                        }
                        onAction={write}
                      />
                    ) : (
                      <EmptyPane
                        icon={<FileDiff size={26} strokeWidth={1.4} />}
                        text="Select a file to review its changes."
                      />
                    )
                  ) : inspectedCommit ? (
                    <GitCommitInspector
                      key={inspectedCommit.oid.hex}
                      repoId={opened.repository.repoId}
                      commit={inspectedCommit}
                      actions={(fullCommit) => (
                        <GitHistoryActions
                          key={inspectedCommit.oid.hex}
                          commit={fullCommit}
                          snapshot={status?.snapshot}
                          conflicted={
                            (status?.metadata.groupCounts?.conflicted ??
                              status?.entries.filter(
                                (entry) => entry.conflicted,
                              ).length ??
                              0) > 0
                          }
                          repository={{
                            ...opened.repository,
                            head:
                              status?.metadata.head ?? opened.repository.head,
                          }}
                          disabled={
                            busy ||
                            !!branchBlockedReason ||
                            !!status?.metadata.integration
                          }
                          busy={busy}
                          error={error}
                          onAction={write}
                        />
                      )}
                    />
                  ) : (
                    <EmptyPane
                      icon={<GitCommitHorizontal size={26} strokeWidth={1.4} />}
                      text="Select a commit to inspect its changes."
                    />
                  )
                }
              />
            </Tabs>
            {(actionsPanel || newWorktree) && (
              <aside
                key={`${connection}:${opened.repository.repoId}`}
                className={cn(
                  "git-repository-inspector relative flex min-h-0 min-w-0 flex-col bg-(--native-surface)",
                  fullPageTool
                    ? "w-full max-w-none flex-1"
                    : "w-[400px] max-w-[48%] flex-none border-l border-border @max-[760px]/git-project:w-full @max-[760px]/git-project:max-w-none @max-[760px]/git-project:border-l-0",
                )}
                role={fullPageTool ? "region" : undefined}
                aria-label={
                  fullPageTool ? "Repository tools" : "Repository inspector"
                }
              >
                {!newWorktree && (
                  <div className="flex flex-none items-center gap-3 border-b border-border py-2 pr-12 pl-4">
                    {fullPageTool && (
                      <Button
                        variant="ghost"
                        disabled={busy}
                        onClick={() => setActionsPanel(null)}
                      >
                        <ArrowLeft size={14} aria-hidden="true" />
                        Back to {tab === "history" ? "History" : "Changes"}
                      </Button>
                    )}
                    <h2 className="text-sm font-medium">
                      {actionsPanel === "stashes"
                        ? "Stashes"
                        : actionsPanel === "tags"
                          ? "Tags"
                          : actionsPanel === "remotes"
                            ? "Repository settings"
                            : "Worktrees"}
                    </h2>
                  </div>
                )}
                {!newWorktree && (
                  <>
                    <GitRemoteControls
                      open={actionsPanel === "remotes"}
                      onOpenChange={(next) =>
                        setActionsPanel(next ? "remotes" : null)
                      }
                      hideTrigger
                      repository={{
                        ...opened.repository,
                        head: status?.metadata.head ?? opened.repository.head,
                      }}
                      projectName={opened.project.name}
                      snapshot={status?.snapshot}
                      busy={busy}
                      blockedReason={branchBlockedReason
                        ?.replace("changing branches", "making Git changes")
                        .replace("Branch changes", "Remote changes")}
                      recoveryAvailable={hasSavedOutcome || receipts === null}
                      error={error}
                      onAction={write}
                    />
                    <GitStashControls
                      open={actionsPanel === "stashes"}
                      onOpenChange={(next) =>
                        setActionsPanel(next ? "stashes" : null)
                      }
                      hideTrigger
                      repoId={opened.repository.repoId}
                      projectName={opened.project.name}
                      snapshot={status?.snapshot}
                      busy={busy}
                      blockedReason={
                        status?.metadata.integration
                          ? "Finish the current Git operation before using stashes."
                          : branchBlockedReason
                              ?.replace("changing branches", "changing stashes")
                              .replace("Branch changes", "Stash changes")
                      }
                      error={error}
                      onAction={write}
                    />
                    <GitTagControls
                      open={actionsPanel === "tags"}
                      onOpenChange={(next) =>
                        setActionsPanel(next ? "tags" : null)
                      }
                      hideTrigger
                      repository={{
                        ...opened.repository,
                        head: status?.metadata.head ?? opened.repository.head,
                      }}
                      snapshot={status?.snapshot}
                      busy={busy}
                      blockedReason={branchBlockedReason
                        ?.replace("changing branches", "changing tags")
                        .replace("Branch changes", "Tag changes")}
                      error={error}
                      onAction={write}
                    />
                    <GitWorktreeControls
                      open={actionsPanel === "worktrees"}
                      onOpenChange={(next) => {
                        if (!next) setWorktreeEdit(undefined);
                        setActionsPanel(next ? "worktrees" : null);
                      }}
                      hideTrigger
                      initial={worktreeEdit}
                      repository={opened.repository}
                      busy={busy}
                      blockedReason={worktreeBlockedReason}
                      error={error}
                      onAction={async (action, snapshot) => {
                        const done = await write(action, snapshot);
                        if (done)
                          void queryClient.invalidateQueries({
                            queryKey: gitKeys.repo(
                              scope,
                              opened.repository.repoId,
                            ),
                          });
                        return done;
                      }}
                    />
                  </>
                )}
                {newWorktree && opened && (
                  <GitNewWorktree
                    repository={{
                      ...opened.repository,
                      head: status?.metadata.head ?? opened.repository.head,
                    }}
                    serverId={server.id}
                    busy={busy}
                    blockedReason={worktreeBlockedReason}
                    error={error}
                    onClose={() => setNewWorktree(false)}
                    onCreate={async (action, snapshot, openAfter) => {
                      const done = await write(action, snapshot);
                      if (!done) return false;
                      setNewWorktree(false);
                      void queryClient.invalidateQueries({
                        queryKey: gitKeys.repo(scope, opened.repository.repoId),
                      });
                      if (openAfter) openCheckout(action.path);
                      return true;
                    }}
                  />
                )}
              </aside>
            )}
          </div>
        </>
      )}
      {editor && (
        <Modal
          title={editor === "add" ? "Add repository" : "Rename project"}
          busy={busy}
          onClose={() => setEditor(null)}
        >
          <GitForm
            dialog
            onSubmit={(event) => {
              event.preventDefault();
              void run(async (projects) => {
                if (editor === "add") {
                  const selection = await projects.add(path, name);
                  upsertProject(selection.project);
                  setEditor(null);
                  selectCheckout(selection.project, selection.project.path);
                } else {
                  upsertProject(await projects.rename(editor, name));
                  setEditor(null);
                }
              });
            }}
          >
            <GitField>
              Project name
              <Input
                autoFocus
                required
                value={name}
                onChange={(event) => setName(event.target.value)}
                disabled={busy}
              />
            </GitField>
            {editor === "add" && (
              <GitField>
                Repository path on server
                <GitPathField>
                  <Input
                    required
                    placeholder="/home/user/projects/app"
                    value={path}
                    onChange={(event) => setPath(event.target.value)}
                    disabled={busy}
                  />
                  <Button
                    type="button"
                    disabled={busy}
                    onClick={() => setBrowsing(true)}
                  >
                    Browse…
                  </Button>
                </GitPathField>
                <GitFieldHint>
                  Use an absolute path. The server agent must be installed.
                </GitFieldHint>
              </GitField>
            )}
            {error && <GitError role="alert">{error}</GitError>}
            <GitFormActions>
              <Button disabled={busy} onClick={() => setEditor(null)}>
                Cancel
              </Button>
              <Button type="submit" loading={busy}>
                {editor === "add" ? "Add repository" : "Save name"}
              </Button>
            </GitFormActions>
          </GitForm>
        </Modal>
      )}
      {creating && (
        <GitCreateProject
          key={creating}
          mode={creating}
          serverId={server.id}
          serverName={server.name}
          busy={busy}
          blockedReason={
            receipts === null
              ? "Saved outcomes are unavailable. Close this dialog and refresh them before creating a repository."
              : undefined
          }
          error={error}
          onClose={() => setCreating(null)}
          pendingOperationIds={receipts
            ?.filter(
              (receipt) =>
                receipt.state === "pending" ||
                receipt.state === "outcome_unknown",
            )
            .map((receipt) => receipt.operationId)}
          onCreate={createProject}
        />
      )}
      {removing && (
        <Modal
          title={`Remove ${removing.name}?`}
          busy={busy}
          onClose={() => setRemoving(null)}
        >
          {/* Inside the form, which is what carries the dialog's body padding:
              a bare child of the Modal sits flush against its left edge. */}
          <GitFormFields dialog>
            <p>
              Only the saved bookmark will be removed. Repository files on the
              server will remain.
            </p>
            {error && <GitError role="alert">{error}</GitError>}
            <GitFormActions>
              <Button disabled={busy} onClick={() => setRemoving(null)}>
                Cancel
              </Button>
              <Button
                variant="destructive"
                disabled={busy}
                onClick={() =>
                  void run(async (projects) => {
                    await projects.remove(removing);
                    queryClient.setQueryData(
                      gitQueries.projects(scope).queryKey,
                      (current = []) =>
                        current.filter((project) => project.id !== removing.id),
                    );
                    queryClient.removeQueries({
                      queryKey: gitKeys.summary(scope, removing.id),
                    });
                    setRemoving(null);
                  })
                }
              >
                Remove bookmark
              </Button>
            </GitFormActions>
          </GitFormFields>
        </Modal>
      )}
      {opened && (
        <GitCommandLog
          key={`${server.id}:${opened.repository.repoId}`}
          serverId={server.id}
          repoId={opened.repository.repoId}
        >
          <span className="flex-none">
            {readAge(readAt, now)
              ? `Read ${readAge(readAt, now)}`
              : "Not read yet"}
          </span>
        </GitCommandLog>
      )}
    </section>
  );
}

/** A pane with nothing chosen yet: a quiet mark and one line of instruction. */
function EmptyPane({ icon, text }: { icon: ReactNode; text: string }) {
  return (
    <GitEmptyState
      as="div"
      className="flex h-full flex-col items-center justify-center gap-[10px] text-[12px] text-muted-foreground"
    >
      <span className="opacity-60" aria-hidden="true">
        {icon}
      </span>
      <p>{text}</p>
    </GitEmptyState>
  );
}

/**
 * A working tree with nothing to review. The main pane says so and offers
 * where to go next, instead of asking for a file that does not exist.
 */
function CleanWorkingTree({
  branch,
  head,
  latest,
  now,
  busy,
  onHistory,
  onFiles,
}: {
  branch: string | null;
  head: string | null;
  /** The newest commit, when history has been read. */
  latest: GitHistory["entries"][number] | null;
  now: number;
  busy: boolean;
  onHistory: () => void;
  onFiles: () => void;
}) {
  // Only claim the commit's subject when the read commit is the one HEAD is on.
  const commit = latest && head && latest.oid.hex === head ? latest : null;
  return (
    <div className="git-changes-clean flex h-full flex-col items-center justify-center gap-[6px] px-[24px] py-[40px] text-center">
      <CircleCheck
        size={28}
        strokeWidth={1.5}
        className="mb-[4px] text-(--green)"
        aria-hidden="true"
      />
      <h3 className="text-[15px] font-semibold">No local changes</h3>
      <p className="text-[12px] text-muted-foreground">
        {branch ? (
          <>
            <code className="text-[12px]">{branch}</code> matches its last
            commit.
          </>
        ) : (
          "The working tree matches its last commit."
        )}
      </p>
      {head && (
        <p className="git-changes-clean-commit mt-[8px] flex max-w-[420px] min-w-0 items-center gap-[8px] rounded-[6px] border border-border px-[10px] py-[6px] text-[12px]">
          <code className="flex-none text-muted-foreground">
            {head.slice(0, 7)}
          </code>
          {commit && (
            <>
              <span className="min-w-0 truncate">
                {commit.message.display.split("\n")[0]}
              </span>
              <span className="flex-none text-[11px] text-muted-foreground">
                {relativeTime(commit.time, now)}
              </span>
            </>
          )}
        </p>
      )}
      <div className="git-changes-clean-actions mt-[12px] flex gap-[8px]">
        <Button disabled={busy} onClick={onHistory}>
          View history
        </Button>
        <Button onClick={onFiles}>Open in Files</Button>
      </div>
    </div>
  );
}
