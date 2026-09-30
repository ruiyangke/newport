// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { GitProjectLibrary } from "./GitProjectLibrary";
import { gitPath, type GitProject } from "../domain/git";

let root: Root;
let host: HTMLDivElement;
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

function project(name: string, path: string): GitProject {
  return { id: name, serverId: "server", name, path: gitPath(path) };
}
const projects = [
  project("webcontainers-demo", "/home/ruiyang/Projects/webcontainers-demo"),
  project("vivari-demo", "/home/ruiyang/Projects/vivari-demo"),
];
function button(label: string) {
  return [...host.querySelectorAll("button")].find(
    (element) =>
      element.textContent === label ||
      element.getAttribute("aria-label") === label,
  )!;
}
/*
 * The menus are shadcn DropdownMenus: Radix opens a trigger on pointerdown or
 * a key, not on a synthetic click, and portals the items to <body> as
 * `menuitem` / `menuitemradio` rather than buttons inside the library.
 */
async function openMenu(label: string) {
  await act(async () =>
    button(label).dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    ),
  );
}
function menuItem(label: string, role = "menuitem") {
  return [...document.querySelectorAll<HTMLElement>(`[role="${role}"]`)].find(
    (element) => element.textContent === label,
  )!;
}
function names() {
  return [...host.querySelectorAll(".git-library-open")].map(
    (element) => element.textContent,
  );
}
function render(
  overrides: Partial<Parameters<typeof GitProjectLibrary>[0]> = {},
) {
  const callbacks = {
    onSearch: vi.fn(),
    onOpen: vi.fn(),
    onEdit: vi.fn(),
    onRemove: vi.fn(),
    onAdd: vi.fn(),
    onClone: vi.fn(),
    onCreate: vi.fn(),
    onToggleFavourite: vi.fn(),
    onSort: vi.fn(),
    onFilter: vi.fn(),
  };
  const node = (
    <GitProjectLibrary
      projects={projects}
      search=""
      favourites={new Set<string>()}
      sort="name"
      filter="all"
      {...callbacks}
      {...overrides}
    />
  );
  return { ...callbacks, node };
}

it("lists each project by name and path, sorted and counted", async () => {
  const { node } = render();
  await act(async () => root.render(node));
  expect(names()).toEqual(["vivari-demo", "webcontainers-demo"]);
  expect(host.textContent).toContain("/home/ruiyang/Projects/vivari-demo");
  expect(host.textContent).toContain("2 projects");
  // The server is named once, by the app's header; the page says what it holds.
  expect(host.textContent).toContain(
    "2 Git repositories bookmarked on this server",
  );
});

it("filters the rows by the search text and reports typing", async () => {
  const { onSearch, node } = render({ search: "vivari" });
  await act(async () => root.render(node));
  expect(names()).toEqual(["vivari-demo"]);
  expect(host.textContent).toContain("1 project");
  const input = host.querySelector("input")!;
  // The field is labelled for assistive technology, not only by placeholder.
  expect(input.closest("label")!.textContent).toContain("Search projects");
  await act(async () => {
    // React tracks the node's own value, so bypass its tracker to type.
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(input, "web");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  expect(onSearch).toHaveBeenCalledWith("web");
});

it("switches the visible set when a filter tab is chosen", async () => {
  const favourites = new Set(["vivari-demo"]);
  const { onFilter, node } = render({ favourites });
  await act(async () => root.render(node));
  expect(names()).toHaveLength(2);
  await act(async () => button("Favorites").click());
  expect(onFilter).toHaveBeenCalledWith("favorites");
  await act(async () =>
    root.render(render({ favourites, filter: "favorites" }).node),
  );
  expect(names()).toEqual(["vivari-demo"]);
  await act(async () => root.render(render({ filter: "archived" }).node));
  expect(names()).toEqual([]);
  expect(host.textContent).toContain("No archived projects.");
});

it("announces the active filter as a pressed button in a labelled group", async () => {
  // The toolbar is built from shadcn primitives; a segmented control that
  // reported itself as a radiogroup, or dropped aria-pressed, would be a
  // different control to assistive technology.
  const { node } = render({ filter: "favorites" });
  await act(async () => root.render(node));
  const group = host.querySelector(
    '[role="group"][aria-label="Filter projects"]',
  )!;
  expect(
    [...group.querySelectorAll("button")].map((element) => [
      element.textContent,
      element.getAttribute("aria-pressed"),
    ]),
  ).toEqual([
    ["All", "false"],
    ["Favorites", "true"],
  ]);
  // Archived is offered only when something can be archived: a tab that could
  // only ever be empty is not offered at all.
  await act(async () =>
    root.render(render({ filter: "favorites", archived: new Set() }).node),
  );
  expect(
    [
      ...host
        .querySelector('[role="group"][aria-label="Filter projects"]')!
        .querySelectorAll("button"),
    ].map((element) => element.textContent),
  ).toEqual(["All", "Favorites", "Archived"]);
});

it("reports the pressed state of the star and toggles it by id", async () => {
  const { onToggleFavourite, node } = render({
    favourites: new Set(["vivari-demo"]),
  });
  await act(async () => root.render(node));
  const on = button("Remove vivari-demo from favorites");
  expect(on.getAttribute("aria-pressed")).toBe("true");
  const off = button("Add webcontainers-demo to favorites");
  expect(off.getAttribute("aria-pressed")).toBe("false");
  await act(async () => off.click());
  expect(onToggleFavourite).toHaveBeenCalledWith("webcontainers-demo");
});

it("keeps open, edit and remove reachable for a row", async () => {
  const { onOpen, onEdit, onRemove, node } = render();
  await act(async () => root.render(node));
  await act(async () => button("vivari-demo").click());
  expect(onOpen).toHaveBeenCalledWith(
    expect.objectContaining({ id: "vivari-demo" }),
  );
  await openMenu("Actions for vivari-demo");
  await act(async () => menuItem("Rename…").click());
  expect(onEdit).toHaveBeenCalledWith(
    expect.objectContaining({ id: "vivari-demo" }),
  );
  await openMenu("Actions for vivari-demo");
  await act(async () => menuItem("Remove from projects…").click());
  expect(onRemove).toHaveBeenCalledWith(
    expect.objectContaining({ id: "vivari-demo" }),
  );
});

it("offers clone, create and add from the primary action", async () => {
  const { onAdd, onClone, onCreate, node } = render();
  await act(async () => root.render(node));
  await openMenu("Add project");
  await act(async () => menuItem("Clone repository").click());
  expect(onClone).toHaveBeenCalled();
  await openMenu("Add project");
  await act(async () => menuItem("New repository").click());
  expect(onCreate).toHaveBeenCalled();
  await openMenu("Add project");
  await act(async () => menuItem("Add existing").click());
  expect(onAdd).toHaveBeenCalled();
});

it("changes the sort through the labelled sort control", async () => {
  const { onSort, node } = render();
  await act(async () => root.render(node));
  await openMenu("Sort: Name");
  // The current order is announced as the checked one of the set.
  expect(menuItem("Name", "menuitemradio").getAttribute("aria-checked")).toBe(
    "true",
  );
  await act(async () => menuItem("Path", "menuitemradio").click());
  expect(onSort).toHaveBeenCalledWith("path");
});

it("shows observed Git state and invents none for a project without it", async () => {
  const { node } = render({
    status: {
      "vivari-demo": {
        branch: "feat/session-recovery",
        changes: 3,
        outgoing: 1,
      },
    },
  });
  await act(async () => root.render(node));
  const [known, unknown] = [...host.querySelectorAll(".git-library-row")];
  expect(known.textContent).toContain("feat/session-recovery");
  expect(known.textContent).toContain("3 changes");
  expect(known.querySelector('[title="3 uncommitted changes"]')).not.toBe(null);
  expect(known.querySelector('[aria-label="1 ahead"]')).not.toBe(null);
  // Nothing was observed for this project, so nothing is claimed about it.
  expect(unknown.textContent).not.toMatch(/clean|change|ahead|commit/i);
  expect(unknown.textContent).not.toContain("0");
  // Unknown state is stated in words rather than shown as a bare glyph.
  expect(unknown.textContent).toContain("Not checked");
});

it("shows commits ahead of the upstream only when they were read", async () => {
  const { node } = render({
    status: { "vivari-demo": { branch: "main", changes: 0, outgoing: 2 } },
  });
  await act(async () => root.render(node));
  const [known, unknown] = [...host.querySelectorAll(".git-library-row")];
  const ahead = known.querySelector('[aria-label="2 ahead"]')!;
  expect(ahead).not.toBe(null);
  // The count compares stored refs, and says so where it is explained.
  expect(ahead.getAttribute("title")).toContain("from stored refs");
  expect(unknown.querySelector('[aria-label$="ahead"]')).toBe(null);
});

it("distinguishes a clean tree from an unreported one", async () => {
  const { node } = render({
    status: {
      "vivari-demo": { branch: "main", clean: true },
      "webcontainers-demo": { branch: "main" },
    },
  });
  await act(async () => root.render(node));
  const [clean, unreported] = [...host.querySelectorAll(".git-library-row")];
  expect(clean.textContent).toContain("Working tree: Clean");
  expect(unreported.textContent).not.toContain("Clean");
  expect(unreported.textContent).toContain("Working tree: Not read");
});

it("disables the actions that mutate bookmarks while busy", async () => {
  const { onOpen, node } = render({ busy: true });
  await act(async () => root.render(node));
  expect(button("Add project").disabled).toBe(true);
  expect(button("vivari-demo").disabled).toBe(true);
  await act(async () => button("vivari-demo").click());
  expect(onOpen).not.toHaveBeenCalled();
});

it("offers a read per row instead of reading every bookmark on sight", async () => {
  const onCheck = vi.fn();
  const { node } = render({
    onCheck,
    checking: new Set(["vivari-demo"]),
    status: {},
  });
  await act(async () => root.render(node));
  // Rows are sorted by name, so vivari-demo leads.
  const [vivari, webcontainers] = [
    ...host.querySelectorAll(".git-library-row"),
  ];
  // Nothing was read, so no row claims a branch or a working-tree state.
  expect(webcontainers.textContent).not.toMatch(/clean|change|main/i);
  const offer =
    webcontainers.querySelector<HTMLButtonElement>(".git-library-check")!;
  expect(offer.textContent).toBe("Check");
  expect(offer.getAttribute("aria-label")).toBe(
    "Check status of webcontainers-demo",
  );
  expect(offer.disabled).toBe(false);
  await act(async () => offer.click());
  expect(onCheck).toHaveBeenCalledTimes(1);
  expect(onCheck.mock.calls[0][0].id).toBe("webcontainers-demo");
  // A read already in flight says so and cannot be asked for twice.
  const running =
    vivari.querySelector<HTMLButtonElement>(".git-library-check")!;
  expect(running.textContent).toBe("Reading…");
  expect(running.disabled).toBe(true);
});

it("checks every row being shown, and only those, when asked", async () => {
  const onCheckAll = vi.fn();
  const { node } = render({ onCheckAll, search: "vivari", status: {} });
  await act(async () => root.render(node));
  await act(async () => button("Check status").click());
  expect(onCheckAll).toHaveBeenCalledTimes(1);
  expect(
    onCheckAll.mock.calls[0][0].map((project: GitProject) => project.id),
  ).toEqual(["vivari-demo"]);
});

it("states that status was not loaded when no read can be offered", async () => {
  const { node } = render({ status: {} });
  await act(async () => root.render(node));
  const row = host.querySelector(".git-library-row")!;
  expect(row.querySelector(".git-library-check")).toBe(null);
  expect(row.textContent).toContain("Not checked");
});

it("separates a branch that was not read from one that is absent", async () => {
  const { node } = render({
    status: {
      "vivari-demo": { branch: null, clean: true },
      "webcontainers-demo": { clean: true },
    },
  });
  await act(async () => root.render(node));
  const [detached, unread] = [...host.querySelectorAll(".git-library-row")];
  // Read, and HEAD is on no branch: a fact, not a gap.
  expect(detached.textContent).toContain("Branch: Detached HEAD");
  expect(detached.querySelector(".git-library-unknown")).toBe(null);
  // Never read: still a gap, and still said in words.
  expect(unread.textContent).toContain("Branch: Not read");
});
