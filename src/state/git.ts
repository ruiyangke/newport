import { createStore, useSelector } from "@tanstack/react-store";
import { useCallback, type SetStateAction } from "react";
import type { GitProject } from "../domain/git";
import type { GitRepository } from "../domain/gitResponses";
import type {
  GitProjectFilter,
  GitProjectSort,
} from "../components/GitProjectLibrary";
import { useCurrentServerScope, useServerScope } from "../query/keys";

export type GitChangeGroup = "conflicted" | "staged" | "unstaged" | "untracked";
export type DiffLayout = "unified" | "split";

/**
 * The Projects page's client state, per server connection.
 *
 * Query owns everything read from the server; this holds only what the user
 * chose. Like the rest of the workspace it survives navigation and never
 * reaches persistent storage.
 *
 * `opened` is the one entry that is not purely a choice: it is the descriptor
 * `repo.open` returned, and its `repoId` names a handle the agent is holding.
 * It lives here rather than in a query because a query may refetch or be
 * collected at any time, and each refetch of `repo.open` would take another
 * handle without closing the last. Opening and closing are explicit, and this
 * is where the result of the last open is kept until it is closed.
 */
export interface GitPageState {
  opened: { project: GitProject; repository: GitRepository } | null;
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
export const gitPageDefaults: GitPageState = {
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

export const gitStore = createStore<Record<string, GitPageState>>({});

export function readGitState(connection: string) {
  return gitStore.state[connection] ?? gitPageDefaults;
}
export function setGitState<K extends keyof GitPageState>(
  connection: string,
  key: K,
  next: SetStateAction<GitPageState[K]>,
) {
  gitStore.setState((state) => {
    const page = state[connection] ?? gitPageDefaults;
    const previous = page[key];
    const value =
      typeof next === "function"
        ? (next as (value: GitPageState[K]) => GitPageState[K])(previous)
        : next;
    if (Object.is(previous, value)) return state;
    return { ...state, [connection]: { ...page, [key]: value } };
  });
}
/** Several fields that must change together, as one update. */
export function patchGitState(
  connection: string,
  patch: Partial<GitPageState>,
) {
  gitStore.setState((state) => ({
    ...state,
    [connection]: { ...(state[connection] ?? gitPageDefaults), ...patch },
  }));
}

export function useGitState<K extends keyof GitPageState>(id: string, key: K) {
  const { connection } = useServerScope(id);
  const value = useSelector(
    gitStore,
    (state) => (state[connection] ?? gitPageDefaults)[key],
  );
  const set = useCallback(
    (next: SetStateAction<GitPageState[K]>) =>
      setGitState(connection, key, next),
    [connection, key],
  );
  return [value, set] as const;
}

/**
 * The same, for components that sit inside one server's page without being
 * told which server it is -- the diff views, which the page renders.
 */
export function useCurrentGitState<K extends keyof GitPageState>(key: K) {
  const { connection } = useCurrentServerScope();
  const value = useSelector(
    gitStore,
    (state) => (state[connection] ?? gitPageDefaults)[key],
  );
  const set = useCallback(
    (next: SetStateAction<GitPageState[K]>) =>
      setGitState(connection, key, next),
    [connection, key],
  );
  return [value, set] as const;
}

export function retainGitState(valid: Set<string>) {
  gitStore.setState((state) => {
    if (Object.keys(state).every((key) => valid.has(key))) return state;
    return Object.fromEntries(
      Object.entries(state).filter(([key]) => valid.has(key)),
    );
  });
}
