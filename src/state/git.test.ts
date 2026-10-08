import { beforeEach, expect, it } from "vitest";
import {
  getGitPageDefaults,
  gitStore,
  patchGitState,
  readGitState,
  retainGitState,
  selectionArrivalPatch,
  setGitState,
} from "./git";

beforeEach(() => gitStore.setState(() => ({})));

it("starts every scope from fresh defaults", () => {
  // Note: writing a key's default value is a no-op by design, so both
  // scopes write non-default values to force page creation.
  setGitState("a", "librarySearch", "x");
  setGitState("b", "librarySearch", "y");
  expect(gitStore.state.a.favourites).not.toBe(
    gitStore.state.b.favourites,
  );
  expect(gitStore.state.a.operationRepositories).not.toBe(
    gitStore.state.b.operationRepositories,
  );
});

it("reads defaults for unknown scopes", () => {
  expect(readGitState("missing")).toEqual(getGitPageDefaults());
});

it("applies selection changes atomically", () => {
  let notifications = 0;
  const subscription = gitStore.subscribe(() => {
    notifications += 1;
  });
  try {
    // The project a selection was made in travels with the selection
    // itself: one update, so a selection can never be observed in a
    // repository it was not made in.
    patchGitState("a", {
      selectionProject: "project:checkout",
      selectedEntry: "entry",
      selectedCommit: "commit",
    });
    expect(notifications).toBe(1);
    expect(readGitState("a").selectionProject).toBe("project:checkout");
    expect(readGitState("a").selectedEntry).toBe("entry");
  } finally {
    subscription.unsubscribe();
  }
});

it("prunes obsolete scopes", () => {
  setGitState("a", "librarySearch", "x");
  setGitState("b", "librarySearch", "y");
  retainGitState(new Set(["b"]));
  expect(Object.keys(gitStore.state)).toEqual(["b"]);
});

it("keeps selections when coming back to their checkout", () => {
  expect(
    selectionArrivalPatch("project:root", "project:root", false),
  ).toEqual({
    actionsPanel: null,
    tab: "changes",
    commitSummary: "",
    commitDescription: "",
    selectionProject: "project:root",
  });
});

it("clears selections carried into another checkout", () => {
  expect(selectionArrivalPatch("project:root", "project:other", false)).toEqual(
    {
      actionsPanel: null,
      tab: "changes",
      commitSummary: "",
      commitDescription: "",
      selectionProject: "project:other",
      selectedEntry: null,
      selectedCommit: null,
    },
  );
});

it("opens bare repositories on the history tab", () => {
  expect(selectionArrivalPatch(null, "project:root", true).tab).toBe(
    "history",
  );
});
