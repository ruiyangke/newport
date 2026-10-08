import type { GitPath, GitProject } from "../domain/git";
import type {
  GitProjectFilter,
  GitProjectSort,
} from "../components/GitProjectLibrary";
import { createScopedStore, registerRetain } from "./scoped";

export type GitChangeGroup = "conflicted" | "staged" | "unstaged" | "untracked";
export type DiffLayout = "unified" | "split";

/**
 * The open checkout, as chosen by the user: a bookmark plus the checkout
 * path. `repo.open` resolves the path to the repository root; the resolved
 * descriptor lives in the query cache (`gitQueries.opened`), never here.
 */
export interface GitSelection {
  project: GitProject;
  path: GitPath;
}

/**
 * The Projects page's client state, per server.
 *
 * Query owns everything read from the server; this holds only what the user
 * chose. Like the rest of the workspace it survives navigation and never
 * reaches persistent storage. Keyed by stable server id so it also survives
 * reconnects; only native handles reset with the connection.
 *
 * `opened` is the one entry that names something off-page: the checkout the
 * user chose. It holds only the selection (bookmark plus checkout path);
 * the resolved descriptor is a query (`gitQueries.opened`), so background
 * refreshes can never replace the selection itself.
 */
export interface GitPageState {
  opened: GitSelection | null;
  tab: "changes" | "history";
  selectedEntry: string | null;
  // Which comparison the chosen group represents, so a file listed under both
  // Staged and Unstaged opens the side it was clicked under.
  selectedSide: GitChangeGroup;
  selectedCommit: string | null;
  // Which project the selections above were made in. Coming back to that
  // project keeps them; opening a different one clears them, so a selection
  // can never carry over into a repository it was not made in.
  selectionProject: string | null;
  fileFilter: string;
  groupFilter: GitChangeGroup | "all";
  commitSummary: string;
  commitDescription: string;
  actionsPanel: "remotes" | "stashes" | "tags" | "worktrees" | null;
  // Unified or split, chosen in the review bar's Diff Settings and followed by
  // every diff on the page, the Changes view's and History's alike.
  diffLayout: DiffLayout;
  librarySearch: string;
  libraryFilter: GitProjectFilter;
  librarySort: GitProjectSort;
  favourites: ReadonlySet<string>;
  // Which repository each write was started against, so a receipt can name it
  // after the page has moved on.
  operationRepositories: Readonly<Record<string, string>>;
}
/** Fresh objects every call: one scope's `Set`s must never reach another's. */
export function getGitPageDefaults(): GitPageState {
  return {
    opened: null,
    tab: "changes",
    selectedEntry: null,
    selectedSide: "unstaged",
    selectedCommit: null,
    selectionProject: null,
    fileFilter: "",
    groupFilter: "all",
    commitSummary: "",
    commitDescription: "",
    actionsPanel: null,
    diffLayout: "unified",
    librarySearch: "",
    libraryFilter: "all",
    librarySort: "name",
    favourites: new Set(),
    operationRepositories: {},
  };
}

const scoped = createScopedStore<GitPageState>({
  getDefaults: getGitPageDefaults,
  // UI choices are keyed by stable server id: filters, selections and the
  // open repository survive reconnects. Native handles keep the connection.
  selectScopeKey: (scope) => scope.id,
});

export const gitStore = scoped.store;

export const readGitState = scoped.read;
export const setGitState = scoped.set;
/** Several fields that must change together, as one update. */
export const patchGitState = scoped.patch;

export const useGitState = scoped.useScoped;

/**
 * The patch a newly resolved descriptor applies to the page state. Pure so
 * the selection-restore rule is unit-testable: coming back to the checkout
 * the selections were made in keeps them; any other checkout clears them.
 */
export function selectionArrivalPatch(
  selectionProject: string | null,
  checkout: string,
  bare: boolean,
): Partial<GitPageState> {
  const returning = selectionProject === checkout;
  return {
    actionsPanel: null,
    tab: bare ? "history" : "changes",
    commitSummary: "",
    commitDescription: "",
    selectionProject: checkout,
    ...(returning ? {} : { selectedEntry: null, selectedCommit: null }),
  };
}

/**
 * The same, for components that sit inside one server's page without being
 * told which server it is -- the diff views, which the page renders.
 */
export const useCurrentGitState = scoped.useCurrentScoped;

export const retainGitState = scoped.retain;

// Unified retention participant (wired via `retainAll` in App.tsx).
registerRetain(({ servers }) => scoped.retain(servers));
