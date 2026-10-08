import { GitInspectorToolbar } from "./GitInspectorPrimitives";
import {
  GitError,
  GitForm,
  GitField,
  GitFormActions,
  GitCheckboxField,
} from "./GitForm";
import { Checkbox } from "./ui/checkbox";
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { MoreHorizontal, Plus, Download, ArrowDownUp } from "lucide-react";
import { gitPath, type GitWriteAction } from "../domain/git";
import type { GitRemotes, GitRepository } from "../domain/gitResponses";
import { gitKeys, gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button, Input } from "./controls";
import { GitInspectorSection } from "./GitInspectorSection";
import { GitRemoteRefs } from "./GitRemoteRefs";
import { GitRemotePicker } from "./GitRemotePicker";
import { useGitRemoteSelection } from "../hooks/useGitRemoteSelection";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "./ui/dropdown-menu";

type Remote = GitRemotes["entries"][number];
type Editor =
  | { kind: "add" }
  | { kind: "rename" | "url" | "remove" | "refs"; remote: Remote };
export function GitRemoteControls({
  repository,
  projectName,
  snapshot,
  busy,
  blockedReason,
  recoveryAvailable,
  open: controlledOpen,
  onOpenChange,
  hideTrigger = false,
  error,
  onAction,
}: {
  /** Still passed by the page; reads go through the session's registry. */
  repository: GitRepository;
  projectName: string;
  snapshot?: string;
  busy: boolean;
  blockedReason?: string;
  recoveryAvailable: boolean;
  /** Controlled disclosure, so a shared actions menu can open this dialog. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  hideTrigger?: boolean;
  error: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
}) {
  const [selfOpen, setSelfOpen] = useState(false);
  const open = controlledOpen ?? selfOpen;
  const setOpen = onOpenChange ?? setSelfOpen;
  // The chosen remote outlives the dialog, so reopening it returns to the
  // remote the user was looking at, while that remote still exists.
  const [selected, setSelected] = useState("");
  return (
    <>
      {!hideTrigger && (
        <Button
          disabled={busy}
          onClick={() => {
            setOpen(true);
          }}
        >
          Remotes
        </Button>
      )}
      {/* Mounted per disclosure, whether opened by the trigger or the actions
          menu: every opening starts from the list and reads it afresh. */}
      {open && (
        <RemotesDialog
          repository={repository}
          projectName={projectName}
          snapshot={snapshot}
          busy={busy}
          blockedReason={blockedReason}
          recoveryAvailable={recoveryAvailable}
          error={error}
          onAction={onAction}
          selected={selected}
          onSelect={setSelected}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}

function RemotesDialog({
  repository,
  projectName,
  snapshot,
  busy,
  blockedReason,
  recoveryAvailable,
  error,
  onAction,
  selected: storedSelection,
  onSelect: setSelected,
  onClose,
}: {
  repository: GitRepository;
  projectName: string;
  snapshot?: string;
  busy: boolean;
  blockedReason?: string;
  recoveryAvailable: boolean;
  error: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
  selected: string;
  onSelect: (name: string) => void;
  onClose: () => void;
}) {
  const scope = useCurrentServerScope();
  const queryClient = useQueryClient();
  const repoId = repository.repoId;
  const current = repository.head.name?.display.replace(/^refs\/heads\//, "");
  const [branch, setBranch] = useState(current ?? "");
  const [editor, setEditor] = useState<Editor | null>(null);
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const selection = useGitRemoteSelection(repoId, storedSelection, setSelected);
  const { selected, remote, loading, error: readError } = selection;
  // Repository settings ride the remotes listing, which the write flow
  // refreshes after every action.
  const remotesQuery = useQuery(gitQueries.remotes(scope, repoId));
  const configuration = remotesQuery.data?.configuration;
  const configToken = configuration?.token ?? null;
  const [authorName, setAuthorName] = useState("");
  const [authorEmail, setAuthorEmail] = useState("");
  // Drafts follow the server values, never the other way around: typing
  // only resets when a refresh reports new values.
  useEffect(() => {
    setAuthorName(configuration?.userName ?? "");
    setAuthorEmail(configuration?.userEmail ?? "");
  }, [configuration]);
  const seenSnapshot = useRef(snapshot);
  useEffect(() => {
    if (seenSnapshot.current === snapshot) return;
    seenSnapshot.current = snapshot;
    void queryClient.invalidateQueries({
      queryKey: [...gitKeys.repo(scope, repoId), "remote-names"],
    });
    void queryClient.invalidateQueries({
      queryKey: gitKeys.remote(scope, repoId, selected),
    });
    void queryClient.invalidateQueries({
      queryKey: gitKeys.remotes(scope, repoId),
    });
  }, [queryClient, scope, repoId, snapshot, selected]);
  const disabled = busy || loading || !!blockedReason;
  const attached =
    !!current &&
    !repository.head.detached &&
    !repository.head.unborn &&
    !!repository.head.oid &&
    !!repository.head.name &&
    gitPath(repository.head.name.display).bytesB64 ===
      repository.head.name.bytesB64;
  async function submit(action: GitWriteAction) {
    if (disabled) return;
    if (await onAction(action)) {
      onClose();
      setEditor(null);
    }
  }
  return (
    <GitInspectorSection
      title={
        editor?.kind === "add"
          ? "Add remote"
          : editor?.kind === "rename"
            ? "Rename remote"
            : editor?.kind === "url"
              ? "Change remote URL"
              : editor?.kind === "remove"
                ? "Remove remote"
                : editor?.kind === "refs"
                  ? "Remote branches and tags"
                  : "Repository settings"
      }
      busy={busy}
      onClose={onClose}
    >
      <div className="min-h-0 flex-auto overflow-y-auto overscroll-contain grid gap-[12px] px-[24px] pt-0 pb-[24px] [&_p]:text-[12px] [&_p]:wrap-anywhere [&_p]:text-muted-foreground">
        {editor && <p className="git-remote-context m-0">{projectName}</p>}
        {error && <GitError role="alert">{error}</GitError>}
        {blockedReason && (
          <div>
            <p>{blockedReason}</p>
            {recoveryAvailable && (
              <Button onClick={onClose}>View saved outcomes</Button>
            )}
          </div>
        )}
        {editor?.kind === "refs" ? (
          <GitRemoteRefs
            repoId={repository.repoId}
            remote={editor.remote}
            snapshot={snapshot}
            disabled={disabled}
            busy={busy}
            source={
              attached && current && repository.head.oid
                ? { name: current, oid: repository.head.oid.hex }
                : undefined
            }
            onAction={onAction}
            onBack={() => setEditor(null)}
          />
        ) : editor ? (
          <GitForm
            onSubmit={(event) => {
              event.preventDefault();
              if (disabled) return;
              if (editor.kind === "add" && name.trim() && url.trim())
                void submit({
                  kind: "remote.add",
                  name: name.trim(),
                  url: url.trim(),
                });
              else if (editor.kind === "rename" && name.trim())
                void submit({
                  kind: "remote.rename",
                  name: editor.remote.name,
                  newName: name.trim(),
                  expectedToken: editor.remote.token,
                });
              else if (editor.kind === "url" && url.trim())
                void submit({
                  kind: "remote.set_url",
                  name: editor.remote.name,
                  url: url.trim(),
                  expectedToken: editor.remote.token,
                });
              else if (editor.kind === "remove")
                void submit({
                  kind: "remote.remove",
                  name: editor.remote.name,
                  expectedToken: editor.remote.token,
                });
            }}
          >
            {editor.kind === "remove" ? (
              <p>
                Remove <strong>{editor.remote.name}</strong> from this
                repository’s configuration? Its remote-tracking references will
                also be removed. The remote repository itself is kept.
              </p>
            ) : (
              <>
                {(editor.kind === "add" || editor.kind === "rename") && (
                  <GitField>
                    Remote name
                    <Input
                      autoFocus
                      value={name}
                      onChange={(event) => setName(event.target.value)}
                      required
                      disabled={busy}
                    />
                  </GitField>
                )}
                {(editor.kind === "add" || editor.kind === "url") && (
                  <GitField>
                    Remote URL
                    <Input
                      value={url}
                      onChange={(event) => setUrl(event.target.value)}
                      placeholder="git@host:team/repository.git"
                      required
                      disabled={busy}
                    />
                  </GitField>
                )}
              </>
            )}
            <GitFormActions>
              <Button disabled={busy} onClick={() => setEditor(null)}>
                Back
              </Button>
              <Button
                type="submit"
                variant={editor.kind === "remove" ? "destructive" : "default"}
                disabled={disabled}
              >
                {editor.kind === "remove" ? "Remove remote" : "Save remote"}
              </Button>
            </GitFormActions>
          </GitForm>
        ) : (
          <>
            {loading && <p role="status">Loading remotes…</p>}
            {readError && (
              <GitError role="alert">
                {readError}{" "}
                <Button disabled={busy || loading} onClick={selection.refresh}>
                  Retry remotes
                </Button>
              </GitError>
            )}
            {!loading && !readError && (
              <>
                <GitInspectorToolbar>
                  <span className="text-[11px] font-normal text-muted-foreground">
                    Repository connections
                  </span>
                  <Button
                    disabled={disabled}
                    onClick={() => {
                      setName("");
                      setUrl("");
                      setEditor({ kind: "add" });
                    }}
                  >
                    <Plus size={13} aria-hidden="true" /> Add remote
                  </Button>
                </GitInspectorToolbar>
                {selection.empty ? (
                  <p>
                    No remotes configured. Add one to fetch and publish
                    branches.
                  </p>
                ) : (
                  <section aria-label="Remotes" className="grid gap-[12px]">
                    <div className="flex items-end gap-[8px]">
                      <label className="git-remote-selection grid min-w-0 flex-1 gap-[6px] text-[12px]">
                        Remote
                        <GitRemotePicker
                          repoId={repoId}
                          value={selected}
                          onChange={setSelected}
                          disabled={busy}
                        />
                      </label>
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button
                            size="icon"
                            variant="ghost"
                            aria-label={`Actions for remote ${selected}`}
                            disabled={busy || loading}
                          >
                            <MoreHorizontal size={16} />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem
                            onSelect={() =>
                              remote && setEditor({ kind: "refs", remote })
                            }
                            disabled={!remote}
                          >
                            Remote branches and tags…
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            disabled={disabled || !remote}
                            onSelect={() => {
                              if (!remote) return;
                              setName(remote.name);
                              setEditor({ kind: "rename", remote });
                            }}
                          >
                            Rename…
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            disabled={disabled || !remote}
                            onSelect={() => {
                              if (!remote) return;
                              setUrl("");
                              setEditor({ kind: "url", remote });
                            }}
                          >
                            Change URL…
                          </DropdownMenuItem>
                          <DropdownMenuItem
                            disabled={disabled || !remote}
                            onSelect={() =>
                              remote &&
                              setEditor({ kind: "remove", remote })
                            }
                          >
                            Remove…
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                    {remote && (
                      <div className="git-remote-address">
                        <span className="text-[11px] font-normal text-muted-foreground">
                          Fetch URL
                        </span>
                        <p className="mt-[4px] wrap-anywhere">
                          {remote.url ?? "Fetch URL unavailable"}
                        </p>
                        {remote.pushUrl && <p>Push URL: {remote.pushUrl}</p>}
                      </div>
                    )}
                  </section>
                )}
                        {remote && (
                          <>
                            <section
                              aria-label="Fetching"
                              className="grid gap-[8px]"
                            >
                              <span className="text-[12px] font-medium">
                                Fetching
                              </span>
                              {configuration ? (
                                <>
                                  <GitCheckboxField compact>
                                    <Checkbox
                                      checked={configuration.fetchPrune}
                                      onCheckedChange={(value) =>
                                        void onAction({
                                          kind: "config.set",
                                          expectedToken: configuration.token,
                                          fetchPrune: value === true,
                                        })
                                      }
                                      disabled={disabled}
                                    />
                                    Prune deleted branches on fetch
                                  </GitCheckboxField>
                                  <p>
                                    Removes remote-tracking branches that no
                                    longer exist upstream whenever fetching.
                                  </p>
                                </>
                              ) : remotesQuery.isError ? (
                                <GitError role="alert">
                                  Repository settings unavailable.{" "}
                                  <Button
                                    disabled={busy}
                                    onClick={() => void remotesQuery.refetch()}
                                  >
                                    Retry
                                  </Button>
                                </GitError>
                              ) : null}
                              <div>
                                <Button
                                  disabled={disabled}
                                  onClick={() =>
                                    void submit({
                                      kind: "fetch",
                                      remote: remote.name,
                                      expectedToken: remote.token,
                                    })
                                  }
                                >
                                  <Download size={13} aria-hidden="true" />{" "}
                                  Fetch {remote.name}
                                </Button>
                              </div>
                            </section>
                        <details className="[&_summary::-webkit-details-marker]:hidden border-t border-border pt-3">
                          <summary className="flex cursor-pointer items-center gap-2 text-[12px] font-medium">
                            <ArrowDownUp size={13} aria-hidden="true" />{" "}
                            Transfer a branch
                          </summary>
                          <div className="grid gap-3 pt-3">
                            <label className="grid gap-[6px] text-[12px]">
                              Remote branch
                              <Input
                                value={branch}
                                onChange={(event) =>
                                  setBranch(event.target.value)
                                }
                                disabled={busy || !attached}
                              />
                            </label>
                            {attached ? (
                              <p>
                                Pull {remote.name}/{branch || "…"} into{" "}
                                {current}, or push {current} to {remote.name}/
                                {branch || "…"}.
                              </p>
                            ) : (
                              <p>
                                Switch to a local branch with a commit before
                                pulling or pushing.
                              </p>
                            )}
                            <div className="git-remote-actions flex flex-wrap gap-[8px]">
                              <Button
                                disabled={
                                  disabled || !attached || !branch.trim()
                                }
                                onClick={() =>
                                  void submit({
                                    kind: "pull.fast_forward",
                                    remote: remote.name,
                                    expectedToken: remote.token,
                                    remoteBranch: branch.trim(),
                                  })
                                }
                              >
                                Pull fast-forward
                              </Button>
                              <Button
                                disabled={
                                  disabled || !attached || !branch.trim()
                                }
                                onClick={() => {
                                  if (current && repository.head.oid)
                                    void submit({
                                      kind: "push",
                                      remote: remote.name,
                                      expectedToken: remote.token,
                                      branch: current,
                                      expectedOid: repository.head.oid.hex,
                                      destinationBranch: branch.trim(),
                                    });
                                }}
                              >
                                Push branch
                              </Button>
                            </div>
                            <p>
                              Pull stops if branches have diverged. Push does
                              not overwrite remote history.
                            </p>
                          </div>
                        </details>
                      </>
                    )}
                    <section
                      aria-label="Commit identity"
                      className="grid gap-[8px]"
                    >
                      <span className="text-[12px] font-medium">
                        Commit identity
                      </span>
                      {configuration ? (
                        <>
                          <div className="grid gap-[8px]">
                            <GitField>
                              Name
                              <Input
                                value={authorName}
                                onChange={(event) =>
                                  setAuthorName(event.target.value)
                                }
                                placeholder="Your name"
                                autoComplete="off"
                                disabled={disabled}
                              />
                            </GitField>
                            <GitField>
                              Email
                              <Input
                                value={authorEmail}
                                onChange={(event) =>
                                  setAuthorEmail(event.target.value)
                                }
                                placeholder="you@example.com"
                                autoComplete="off"
                                disabled={disabled}
                              />
                            </GitField>
                          </div>
                          <p>
                            Signed on commits you create in this repository.
                          </p>
                          <div>
                            <Button
                              disabled={
                                disabled ||
                                !authorName.trim() ||
                                !authorEmail.trim() ||
                                (authorName.trim() ===
                                  (configuration.userName ?? "") &&
                                  authorEmail.trim() ===
                                    (configuration.userEmail ?? ""))
                              }
                              onClick={() => {
                                if (!configToken) return;
                                void onAction({
                                  kind: "config.set",
                                  expectedToken: configToken,
                                  userName: authorName.trim(),
                                  userEmail: authorEmail.trim(),
                                });
                              }}
                            >
                              Save identity
                            </Button>
                          </div>
                        </>
                      ) : remotesQuery.isError ? (
                        <GitError role="alert">
                          Repository settings unavailable.{" "}
                          <Button
                            disabled={busy}
                            onClick={() => void remotesQuery.refetch()}
                          >
                            Retry
                          </Button>
                        </GitError>
                      ) : null}
                    </section>
                  </>
                )}
              </>
            )}
      </div>
    </GitInspectorSection>
  );
}
