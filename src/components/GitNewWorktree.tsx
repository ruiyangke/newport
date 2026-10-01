import { readSharedQuery } from "../query/client";
import { gitErrorMessage } from "../git/errors";
import { useEffect, useId, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { gitPath, type GitWorktreeAction } from "../domain/git";
import type { GitRepository, decodeGitBranches } from "../domain/gitResponses";
import { useGitPageLoader } from "../hooks/useGitPageLoader";
import { GitLoadMore } from "./GitLoadMore";
import { gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import {
  defaultWorktreePath,
  mainWorktree,
  worktreeByBranch,
  worktreeLabel,
  worktreeNameFor,
} from "../git/worktrees";
import { Button, Checkbox, Input } from "./controls";
import { GitInspectorSection } from "./GitInspectorSection";
import { GitFolderChooser } from "./GitFolderChooser";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "./ui/select";

type BranchEntry = ReturnType<typeof decodeGitBranches>["entries"][number];
const NOTE = "text-[12px] text-muted-foreground";

/**
 * A new worktree in one step: usually on a new branch started from the branch
 * the work should build on, as `git worktree add -b` does -- which is how an
 * agent is given a checkout of its own -- or on an existing branch that is not
 * checked out anywhere else.
 *
 * The name and the location follow the branch until they are edited, so the
 * common case is typing one branch name.
 */
export function GitNewWorktree({
  repository,
  serverId,
  busy,
  blockedReason,
  error,
  onClose,
  onCreate,
}: {
  repository: GitRepository;
  serverId: string;
  busy: boolean;
  blockedReason?: string;
  error: string;
  onClose: () => void;
  /** Starts the operation; resolves whether it succeeded. */
  onCreate: (
    action: Extract<GitWorktreeAction, { kind: "worktree.add" }>,
    snapshot: string,
    openAfter: boolean,
  ) => Promise<boolean>;
}) {
  const scope = useCurrentServerScope();
  const formId = useId();
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => setFilter(search.trim()), 200);
    return () => clearTimeout(timer);
  }, [search]);
  const branchOptions = useMemo(
    () => (filter ? { filter } : undefined),
    [filter],
  );
  const branchQuery = gitQueries.branches(
    scope,
    repository.repoId,
    undefined,
    branchOptions,
  );
  const worktrees = useQuery({
    ...gitQueries.worktrees(scope, repository.repoId, undefined, {
      pageSize: 1,
    }),
    refetchOnMount: "always",
  });
  const branches = useQuery({
    ...branchQuery,
    refetchOnMount: "always",
  });
  const pages = useGitPageLoader({
    queryKey: branchQuery.queryKey,
    page: branches.data ?? null,
    enabled:
      !branches.isFetching && !branches.isError && search.trim() === filter,
    entryKey: (entry: BranchEntry) => entry.reference.bytesB64,
    read: (cursor, signal) =>
      readSharedQuery(
        queryClient,
        {
          ...gitQueries.branches(
            scope,
            repository.repoId,
            cursor,
            branchOptions,
          ),
          staleTime: Infinity,
        },
        signal,
      ),
  });
  useEffect(() => {
    if (
      !branches.data?.nextCursor ||
      branches.isFetching ||
      branches.isError ||
      search.trim() !== filter
    )
      return;
    void queryClient.prefetchQuery({
      ...gitQueries.branches(
        scope,
        repository.repoId,
        branches.data.nextCursor,
        branchOptions,
      ),
      staleTime: Infinity,
    });
  }, [
    branches.data?.nextCursor,
    branches.isFetching,
    branches.isError,
    search,
    filter,
    queryClient,
    scope,
    repository.repoId,
    branchOptions,
  ]);
  const rows = useMemo(() => worktrees.data?.entries ?? [], [worktrees.data]);
  const inUse = useMemo(() => worktreeByBranch(rows), [rows]);
  const main = worktrees.data?.metadata.main ?? mainWorktree(rows);
  const mainRoot = main?.path?.display ?? repository.root.display;
  const currentBranch =
    repository.head.detached || !repository.head.name
      ? null
      : repository.head.name.display.replace(/^refs\/heads\//, "");

  const [mode, setMode] = useState<"new" | "existing">("new");
  const [branch, setBranch] = useState("");
  const [existing, setExisting] = useState("");
  // Starting point: a branch reference, local or remote-tracking.
  const [base, setBase] = useState<string | null>(null);
  const [selectedBase, setSelectedBase] = useState<BranchEntry | null>(null);
  const [selectedExisting, setSelectedExisting] = useState<BranchEntry | null>(
    null,
  );
  const [name, setName] = useState<string | null>(null);
  const [path, setPath] = useState<string | null>(null);
  const [openAfter, setOpenAfter] = useState(true);
  const [locked, setLocked] = useState(false);
  const [browsing, setBrowsing] = useState(false);
  const [localError, setLocalError] = useState("");

  const entries = useMemo(() => {
    const loaded = branches.data?.entries.filter((entry) => entry.oid) ?? [];
    const seen = new Set(loaded.map((entry) => entry.reference.bytesB64));
    return [
      ...loaded,
      ...[selectedBase, selectedExisting].filter(
        (entry): entry is BranchEntry => {
          if (!entry || seen.has(entry.reference.bytesB64)) return false;
          seen.add(entry.reference.bytesB64);
          return true;
        },
      ),
    ];
  }, [branches.data, selectedBase, selectedExisting]);
  const local = entries.filter((entry) => !entry.remote);
  const remote = entries.filter((entry) => entry.remote);
  const baseRef =
    base ??
    entries.find((entry) => entry.current)?.reference.display ??
    local[0]?.reference.display ??
    "";
  const baseEntry = entries.find(
    (entry) => entry.reference.display === baseRef,
  );
  const existingEntry = local.find((entry) => entry.name.display === existing);
  const chosenBranch = mode === "new" ? branch.trim() : existing;
  const derivedName = worktreeNameFor(chosenBranch);
  const effectiveName = name ?? derivedName;
  const effectivePath =
    path ?? (effectiveName ? defaultWorktreePath(mainRoot, effectiveName) : "");
  const [checkedName, setCheckedName] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => setCheckedName(effectiveName), 200);
    return () => clearTimeout(timer);
  }, [effectiveName]);
  const nameLookup = useQuery({
    ...gitQueries.worktrees(scope, repository.repoId, undefined, {
      name: checkedName,
      pageSize: 1,
    }),
    enabled: !!checkedName,
    staleTime: 0,
  });
  const branchLookup = useQuery({
    ...gitQueries.worktrees(scope, repository.repoId, undefined, {
      branch: `refs/heads/${chosenBranch}`,
      pageSize: 1,
    }),
    enabled: !!chosenBranch && mode === "existing",
    staleTime: 0,
  });
  const nameTaken =
    nameLookup.data?.entries.some(
      (row) => row.name?.display === effectiveName,
    ) ?? false;
  const holder = branchLookup.data?.entries.find(
    (row) => row.head?.name?.display === `refs/heads/${chosenBranch}`,
  );
  const branchTaken =
    mode === "new" &&
    local.some((entry) => entry.name.display === branch.trim());
  const expectedOid =
    mode === "new" ? baseEntry?.oid?.hex : existingEntry?.oid?.hex;
  const ready =
    !busy &&
    !blockedReason &&
    !branches.isFetching &&
    !branches.isError &&
    search.trim() === filter &&
    !worktrees.isFetching &&
    !worktrees.isError &&
    !!worktrees.data &&
    !!chosenBranch &&
    !!expectedOid &&
    !!effectiveName &&
    !nameTaken &&
    checkedName === effectiveName &&
    !nameLookup.isFetching &&
    !nameLookup.isError &&
    !!nameLookup.data &&
    (mode !== "existing" ||
      (!branchLookup.isFetching &&
        !branchLookup.isError &&
        !!branchLookup.data &&
        !holder)) &&
    !branchTaken &&
    effectivePath.startsWith("/");

  function submit() {
    setLocalError("");
    if (!ready || !worktrees.data || !expectedOid) return;
    try {
      void onCreate(
        {
          kind: "worktree.add",
          name: effectiveName,
          path: gitPath(effectivePath),
          branch: chosenBranch,
          expectedOid,
          locked,
          ...(mode === "new" ? { newBranch: true } : {}),
        },
        worktrees.data.snapshot,
        openAfter,
      );
    } catch (reason) {
      setLocalError(gitErrorMessage(reason));
    }
  }

  const readError =
    (worktrees.isError && gitErrorMessage(worktrees.error)) ||
    (branches.isError && gitErrorMessage(branches.error)) ||
    (nameLookup.isError && gitErrorMessage(nameLookup.error)) ||
    (branchLookup.isError && gitErrorMessage(branchLookup.error)) ||
    "";

  return (
    <>
      <GitInspectorSection title="New worktree" busy={busy} onClose={onClose}>
        <form
          id={formId}
          className="git-project-form"
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          <p className={NOTE}>
            A separate checkout of this repository on its own branch — a place
            for an agent, or you, to work without touching{" "}
            {currentBranch ? <code>{currentBranch}</code> : "this checkout"}.
          </p>
          {blockedReason && (
            <p className={NOTE} role="status">
              {blockedReason}
            </p>
          )}
          {(readError || localError || error) && (
            <p role="alert">{readError || localError || error}</p>
          )}
          <div
            role="radiogroup"
            aria-label="Branch"
            className="flex w-fit items-center gap-[2px] rounded-[7px] bg-(--native-toolbar) p-[2px]"
          >
            {(
              [
                ["new", "New branch"],
                ["existing", "Existing branch"],
              ] as const
            ).map(([value, label]) => (
              <Button
                key={value}
                variant="ghost"
                role="radio"
                aria-checked={mode === value}
                onClick={() => setMode(value)}
                className={
                  mode === value
                    ? "h-[26px]! rounded-[5px]! border-0 bg-background px-[10px]! text-[12px] font-medium text-foreground shadow-[0_1px_2px_rgb(0_0_0/0.1)] hover:bg-background dark:hover:bg-background"
                    : "h-[26px]! rounded-[5px]! border-0 bg-transparent px-[10px]! text-[12px] font-medium text-muted-foreground hover:bg-transparent dark:hover:bg-transparent"
                }
              >
                {label}
              </Button>
            ))}
          </div>
          <label>
            Find a branch
            <Input
              aria-label="Find a branch"
              placeholder="Search all branches"
              value={search}
              disabled={busy}
              onChange={(event) => {
                // Keep the explicit starting point and its expected OID while
                // searching another page; the backend revalidates it on write.
                if (baseEntry) {
                  setSelectedBase(baseEntry);
                  setBase(baseRef);
                }
                if (existingEntry) setSelectedExisting(existingEntry);
                setSearch(event.target.value);
              }}
            />
          </label>
          {(branches.isFetching || search.trim() !== filter) && (
            <p role="status" className={NOTE}>
              Finding branches…
            </p>
          )}
          {branches.isError && (
            <Button onClick={() => void branches.refetch()}>
              Retry branches
            </Button>
          )}
          {mode === "new" ? (
            <>
              <label>
                Branch name
                <Input
                  autoFocus
                  value={branch}
                  placeholder="agent/fix-login"
                  onChange={(event) => setBranch(event.target.value)}
                  disabled={busy}
                  aria-invalid={branchTaken || undefined}
                />
                {branchTaken && (
                  <small>
                    That branch exists. Choose it under Existing branch, or pick
                    another name.
                  </small>
                )}
              </label>
              <label>
                Start from
                <Select
                  value={baseRef}
                  // Radix reports "" while the options are still arriving;
                  // that is not a choice, and must not replace the default.
                  onValueChange={(value) => {
                    if (!value) return;
                    setBase(value);
                    setSelectedBase(
                      entries.find(
                        (entry) => entry.reference.display === value,
                      ) ?? null,
                    );
                  }}
                  disabled={busy || branches.isPending}
                >
                  <SelectTrigger aria-label="Start from">
                    <SelectValue
                      placeholder={
                        branches.isPending
                          ? "Loading branches…"
                          : "Choose a branch"
                      }
                    />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectGroup>
                      <SelectLabel>Local</SelectLabel>
                      {local.map((entry) => (
                        <SelectItem
                          key={entry.reference.bytesB64}
                          value={entry.reference.display}
                        >
                          {entry.name.display}
                          {entry.current ? " (current)" : ""}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                    {remote.length > 0 && (
                      <SelectGroup>
                        <SelectLabel>Remote</SelectLabel>
                        {remote.map((entry) => (
                          <SelectItem
                            key={entry.reference.bytesB64}
                            value={entry.reference.display}
                          >
                            {entry.name.display}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    )}
                  </SelectContent>
                </Select>
                {baseEntry?.oid && (
                  <small>
                    Starts at <code>{baseEntry.oid.hex.slice(0, 7)}</code>
                    {baseEntry.remote
                      ? ", the last fetched commit of that remote branch."
                      : "."}
                  </small>
                )}
              </label>
            </>
          ) : (
            <label>
              Branch
              <Select
                value={existing}
                onValueChange={(value) => {
                  if (!value) return;
                  setExisting(value);
                  setSelectedExisting(
                    local.find((entry) => entry.name.display === value) ?? null,
                  );
                }}
                disabled={busy || branches.isPending}
              >
                <SelectTrigger aria-label="Existing branch">
                  <SelectValue placeholder="Choose a branch" />
                </SelectTrigger>
                <SelectContent>
                  {local.map((entry) => {
                    const holder = inUse.get(entry.name.display);
                    return (
                      <SelectItem
                        key={entry.reference.bytesB64}
                        value={entry.name.display}
                        disabled={!!holder}
                      >
                        {entry.name.display}
                        {holder ? ` — in ${worktreeLabel(holder)}` : ""}
                      </SelectItem>
                    );
                  })}
                </SelectContent>
              </Select>
              <small>
                {holder
                  ? `Already checked out in ${worktreeLabel(holder)}.`
                  : "A branch can be checked out in one worktree at a time."}
              </small>
            </label>
          )}
          {branches.data && (
            <GitLoadMore
              cursor={branches.data.nextCursor}
              loading={pages.loading}
              error={pages.error}
              disabled={
                branches.isFetching ||
                branches.isError ||
                search.trim() !== filter
              }
              automatic={false}
              onLoad={() => void pages.load()}
              label="Load more branches"
              endLabel="All matching branches loaded"
            />
          )}
          <label>
            Worktree name
            <Input
              value={effectiveName}
              onChange={(event) => setName(event.target.value)}
              disabled={busy}
              aria-invalid={nameTaken || undefined}
            />
            {nameTaken && <small>A worktree with that name exists.</small>}
          </label>
          {/* Not one wrapping label: that would name the field after the
              Browse button and the help text as well. */}
          <div className="grid gap-[6px]">
            <label htmlFor={`${formId}-path`}>Location on server</label>
            <div className="git-path-field">
              <Input
                id={`${formId}-path`}
                aria-describedby={`${formId}-path-help`}
                value={effectivePath}
                placeholder="/srv/app-agent-fix-login"
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
            </div>
            <small id={`${formId}-path-help`}>
              Beside the main checkout by default. The folder is created; its
              parent must exist.
            </small>
          </div>
          <label className="git-checkbox-row">
            <Checkbox
              checked={openAfter}
              onCheckedChange={(value) => setOpenAfter(value === true)}
              disabled={busy}
              aria-label="Open it when created"
            />
            Open it when created
          </label>
          <label className="git-checkbox-row">
            <Checkbox
              checked={locked}
              onCheckedChange={(value) => setLocked(value === true)}
              disabled={busy}
              aria-label="Lock against removal"
            />
            Lock against removal
          </label>
          <footer>
            <Button disabled={busy} onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" loading={busy} disabled={!ready}>
              Create worktree
            </Button>
          </footer>
        </form>
      </GitInspectorSection>
      <GitFolderChooser
        serverId={serverId}
        open={browsing}
        initialPath={
          effectivePath.slice(0, effectivePath.lastIndexOf("/")) || "/"
        }
        title="Choose where the worktree goes"
        busy={busy}
        onCancel={() => setBrowsing(false)}
        onChoose={(chosen) => {
          // The chooser picks the parent; the worktree gets its own folder.
          setPath(
            `${chosen.replace(/\/+$/, "")}/${effectiveName || "worktree"}`,
          );
          setBrowsing(false);
        }}
      />
    </>
  );
}
