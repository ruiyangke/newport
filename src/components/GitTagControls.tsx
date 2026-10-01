import { Tag as TagIcon, Plus, Search } from "lucide-react";
import { GitRemotePicker } from "./GitRemotePicker";
import { useGitRemoteSelection } from "../hooks/useGitRemoteSelection";
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { gitPath, type GitWriteAction } from "../domain/git";
import { type GitRepository, type GitTags } from "../domain/gitResponses";
import { useGitPageLoader } from "../hooks/useGitPageLoader";
import { gitErrorMessage } from "../git/errors";
import { gitProjectsFor } from "../git/registry";
import { GitLoadMore } from "./GitLoadMore";
import { gitKeys, gitQueries } from "../query/git";
import { useCurrentServerScope } from "../query/keys";
import { Button, Input } from "./controls";
import { GitInspectorSection } from "./GitInspectorSection";
import { Textarea } from "./ui/textarea";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";

/** Helper copy in the tags dialog: small and muted, as the design has it. */
const note = "text-[12px] text-muted-foreground";

type Tag = GitTags["entries"][number];
type Editor = { kind: "create" } | { kind: "delete" | "push"; tag: Tag };
function usable(tag: Tag) {
  try {
    return (
      !!tag.oid &&
      !tag.symbolicTarget &&
      gitPath(tag.name.display).bytesB64 === tag.name.bytesB64
    );
  } catch {
    return false;
  }
}
export function GitTagControls({
  repository,
  snapshot,
  busy,
  blockedReason,
  open: controlledOpen,
  onOpenChange,
  hideTrigger = false,
  error,
  onAction,
}: {
  /** Still passed by the page; reads go through the session's registry. */
  repository: GitRepository;
  snapshot?: string;
  busy: boolean;
  blockedReason?: string;
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
  return (
    <>
      {!hideTrigger && (
        <Button
          disabled={busy}
          onClick={() => {
            setOpen(true);
          }}
        >
          Tags
        </Button>
      )}
      {/* Mounted per disclosure, whether opened by the trigger or the actions
          menu: every opening starts from the list and reads it afresh. */}
      {open && (
        <TagsDialog
          repository={repository}
          snapshot={snapshot}
          busy={busy}
          blockedReason={blockedReason}
          error={error}
          onAction={onAction}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}

function TagsDialog({
  repository,
  snapshot,
  busy,
  blockedReason,
  error,
  onAction,
  onClose,
}: {
  repository: GitRepository;
  snapshot?: string;
  busy: boolean;
  blockedReason?: string;
  error: string;
  onAction: (action: GitWriteAction) => Promise<boolean>;
  onClose: () => void;
}) {
  const scope = useCurrentServerScope();
  const queryClient = useQueryClient();
  const repoId = repository.repoId;
  const [editor, setEditor] = useState<Editor | null>(null);
  const [name, setName] = useState("");
  const [filter, setFilter] = useState("");
  const [target, setTarget] = useState("");
  const [annotation, setAnnotation] = useState("");
  const [annotated, setAnnotated] = useState(false);
  const [remoteName, setRemoteName] = useState("");
  const first = useQuery({
    ...gitQueries.tags(scope, repoId),
    refetchOnMount: "always",
  });
  // The page re-reads status after anything that may have moved a tag; a new
  // snapshot re-reads the list, which also starts it over at the first page.
  const seenSnapshot = useRef(snapshot);
  useEffect(() => {
    if (seenSnapshot.current === snapshot) return;
    seenSnapshot.current = snapshot;
    void queryClient.invalidateQueries({
      queryKey: gitKeys.tags(scope, repoId),
    });
  }, [queryClient, scope, repoId, snapshot]);
  const page = first.data;
  const loading = first.isFetching;
  const [selected, setSelected] = useState<string | null>(null);
  const pages = useGitPageLoader({
    queryKey: gitQueries.tags(scope, repoId).queryKey,
    page: page ?? null,
    enabled: !first.isFetching && !first.isError && !editor,
    prefetch: false,
    entryKey: (tag: Tag) => tag.reference.bytesB64,
    read: (cursor, signal) =>
      gitProjectsFor(scope)
        .repositories.withSignal(signal)
        .tags(repoId, cursor),
  });
  const pushing = editor?.kind === "push";
  const remoteSelection = useGitRemoteSelection(
    repoId,
    remoteName,
    setRemoteName,
    pushing,
  );
  const remote = remoteSelection.remote;
  const readError =
    (!first.isFetching && first.error ? gitErrorMessage(first.error) : "") ||
    remoteSelection.error;
  const preview = page?.entries.find((tag) => tag.name.bytesB64 === selected);
  const detail = useQuery({
    ...gitQueries.tag(scope, repoId, preview?.oid?.hex ?? ""),
    enabled: !!preview?.oid && !!preview.messageTruncated,
  });
  const selectedTag =
    preview && detail.data ? { ...preview, ...detail.data } : preview;
  const disabled =
    busy || loading || first.isError || !!blockedReason || !snapshot;
  async function submit(action: GitWriteAction) {
    if (disabled) return;
    if (await onAction(action)) {
      setEditor(null);
      onClose();
    }
  }
  function reload() {
    setSelected(null);
    void first.refetch();
    if (pushing) remoteSelection.refresh();
  }
  return (
    <GitInspectorSection
      title={
        editor?.kind === "create"
          ? "Create tag"
          : editor?.kind === "delete"
            ? "Delete local tag"
            : editor?.kind === "push"
              ? "Push tag"
              : "Tags"
      }
      busy={busy}
      onClose={onClose}
    >
      <div className="git-tag-body min-h-0 px-[16px] pb-[16px] [overflow-wrap:anywhere]">
        {blockedReason && (
          <p role="status" className={note}>
            {blockedReason}
          </p>
        )}
        {(readError || error) && (
          <div role="alert">
            <p className={note}>{readError || error}</p>
            {readError && (
              <Button disabled={busy || loading} onClick={reload}>
                Reload tags
              </Button>
            )}
          </div>
        )}
        {editor ? (
          <form
            className="git-project-form"
            onSubmit={(event) => {
              event.preventDefault();
              if (disabled) return;
              if (
                editor.kind === "create" &&
                name.trim() &&
                /^[0-9a-f]{40}$/i.test(target.trim()) &&
                (!annotated || annotation.trim())
              )
                void submit({
                  kind: "tag.create",
                  name: name.trim(),
                  targetOid: target.trim().toLowerCase(),
                  ...(annotated ? { annotation: { message: annotation } } : {}),
                });
              else if (editor.kind === "delete" && usable(editor.tag))
                void submit({
                  kind: "tag.delete",
                  name: editor.tag.name.display,
                  expectedOid: editor.tag.oid!.hex,
                });
              else if (editor.kind === "push" && remote && usable(editor.tag))
                void submit({
                  kind: "tag.push",
                  name: editor.tag.name.display,
                  expectedOid: editor.tag.oid!.hex,
                  remote: remote.name,
                  expectedToken: remote.token,
                });
            }}
          >
            {editor.kind === "create" ? (
              <>
                <label>
                  Tag name
                  <Input
                    autoFocus
                    required
                    value={name}
                    onChange={(event) => setName(event.target.value)}
                    disabled={busy}
                    placeholder="v1.0.0"
                  />
                </label>
                <label>
                  Target object ID
                  <Input
                    aria-label="Target object ID"
                    aria-describedby="git-tag-target-hint"
                    required
                    value={target}
                    onChange={(event) => setTarget(event.target.value)}
                    disabled={busy}
                    pattern="[0-9a-fA-F]{40}"
                    spellCheck={false}
                  />
                  <small id="git-tag-target-hint">
                    Defaults to the current commit. Enter a full 40-character
                    Git object ID.
                  </small>
                </label>
                <label>
                  Tag type
                  <Select
                    value={annotated ? "annotated" : "lightweight"}
                    onValueChange={(value) =>
                      setAnnotated(value === "annotated")
                    }
                    disabled={busy}
                  >
                    <SelectTrigger aria-label="Tag type">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="lightweight">Lightweight</SelectItem>
                      <SelectItem value="annotated">Annotated</SelectItem>
                    </SelectContent>
                  </Select>
                </label>
                {annotated && (
                  <label>
                    Tag message
                    <Textarea
                      aria-label="Tag message"
                      aria-describedby="git-tag-message-hint"
                      required
                      value={annotation}
                      onChange={(event) => setAnnotation(event.target.value)}
                      disabled={busy}
                    />
                    <small id="git-tag-message-hint">
                      Uses the Git identity configured on the server.
                    </small>
                  </label>
                )}
                <p className={note}>
                  Creates a local tag. Push it separately to share it with a
                  remote.
                </p>
              </>
            ) : (
              <>
                <p className={note}>
                  <strong>{editor.tag.name.display}</strong> ·{" "}
                  {editor.tag.oid?.hex.slice(0, 12)}
                </p>
                {editor.kind === "delete" ? (
                  <p className={note}>
                    Remove this tag from the server repository? Commits and tags
                    on other remotes will remain.
                  </p>
                ) : (
                  <>
                    <p className={note}>
                      Push this exact tag to the selected remote. An existing
                      remote tag will not be overwritten.
                    </p>
                    {remoteSelection.loading && (
                      <p role="status" className={note}>
                        Loading remotes…
                      </p>
                    )}
                    {remoteSelection.empty && (
                      <p className={note}>
                        No remotes configured. Add one in Remotes first.
                      </p>
                    )}
                    {!remoteSelection.empty && (
                      <label>
                        Remote
                        <GitRemotePicker
                          repoId={repoId}
                          value={remoteSelection.selected}
                          onChange={setRemoteName}
                          disabled={busy}
                        />
                      </label>
                    )}
                    {remote && (
                      <p className={`git-tag-address ${note}`}>
                        {remote.pushUrl ??
                          remote.url ??
                          "Remote URL unavailable"}
                      </p>
                    )}
                  </>
                )}
              </>
            )}
            <footer>
              <Button disabled={busy} onClick={() => setEditor(null)}>
                Back
              </Button>
              <Button
                type="submit"
                loading={busy}
                variant={editor.kind === "delete" ? "destructive" : undefined}
                disabled={
                  disabled ||
                  (editor.kind === "push" && !remote) ||
                  (editor.kind === "create" &&
                    (!name.trim() ||
                      !/^[0-9a-f]{40}$/i.test(target.trim()) ||
                      (annotated && !annotation.trim())))
                }
              >
                {editor.kind === "create"
                  ? "Create tag"
                  : editor.kind === "delete"
                    ? "Delete local tag"
                    : "Push tag"}
              </Button>
            </footer>
          </form>
        ) : (
          <>
            <div className="git-inspector-toolbar">
              <span className="git-inspector-count">
                {page?.entries.length ?? 0} tags
                {page?.nextCursor ? " loaded" : ""}
              </span>
              <Button
                disabled={disabled}
                onClick={() => {
                  setName("");
                  setTarget(repository.head.oid?.hex ?? "");
                  setAnnotation("");
                  setAnnotated(false);
                  setEditor({ kind: "create" });
                }}
              >
                <Plus size={13} aria-hidden="true" /> New tag
              </Button>
            </div>
            <div className="git-inspector-search">
              <Search size={13} aria-hidden="true" />
              <Input
                aria-label="Filter loaded tags"
                placeholder="Filter loaded tags"
                value={filter}
                onChange={(event) => setFilter(event.target.value)}
              />
            </div>
            {loading && (
              <p role="status" className={note}>
                Loading tags…
              </p>
            )}
            {page?.entries.length === 0 && (
              <p className={note}>No tags in this repository.</p>
            )}
            {filter &&
              page &&
              !page.entries.some((tag) =>
                tag.name.display.toLowerCase().includes(filter.toLowerCase()),
              ) && <p className={note}>No matching loaded tags.</p>}
            <ul className="git-tag-list git-inspector-list">
              {page?.entries
                .filter((tag) =>
                  tag.name.display.toLowerCase().includes(filter.toLowerCase()),
                )
                .map((tag) => (
                  <li
                    key={tag.name.bytesB64}
                    className="git-inspector-list-item"
                  >
                    <Button
                      variant="ghost"
                      className="git-inspector-row"
                      title={tag.name.display}
                      aria-pressed={selected === tag.name.bytesB64}
                      onClick={() => setSelected(tag.name.bytesB64)}
                    >
                      <TagIcon
                        size={14}
                        className="git-inspector-row-icon"
                        aria-hidden="true"
                      />
                      <span className="git-inspector-row-copy">
                        <strong className="git-inspector-row-title">
                          {tag.name.display}
                        </strong>
                        <span className="git-inspector-row-meta">
                          {tag.annotated ? "Annotated tag" : "Lightweight tag"}
                        </span>
                      </span>
                      <code className="git-inspector-row-meta">
                        {tag.oid?.hex.slice(0, 7) ?? "—"}
                      </code>
                    </Button>
                    {selectedTag && selected === tag.name.bytesB64 && (
                      <div className="git-tag-details git-inspector-details">
                        <strong>{selectedTag.name.display}</strong>
                        <p className={`mt-[6px]! ${note}`}>
                          Object: {selectedTag.oid?.hex ?? "Unavailable"}
                        </p>
                        {selectedTag.peeledOid && (
                          <p className={`mt-[6px]! ${note}`}>
                            Target: {selectedTag.peeledOid.hex} (
                            {selectedTag.peeledType})
                          </p>
                        )}
                        {selectedTag.tagger && (
                          <p className={`mt-[6px]! ${note}`}>
                            {selectedTag.tagger.name} &lt;
                            {selectedTag.tagger.email}
                            &gt;
                          </p>
                        )}
                        {selectedTag.message && (
                          <pre className="mt-[12px] max-h-[160px] overflow-y-auto [font:inherit] whitespace-pre-wrap">
                            {selectedTag.message.display}
                          </pre>
                        )}
                        {selectedTag.messageTruncated && (
                          <p className={`mt-[6px]! ${note}`}>
                            {detail.isFetching
                              ? "Loading full annotation…"
                              : "The annotation is truncated."}
                          </p>
                        )}
                        {detail.isError && !detail.isFetching && (
                          <div role="alert" className={`mt-[6px] ${note}`}>
                            {gitErrorMessage(detail.error)}
                            <Button onClick={() => void detail.refetch()}>
                              Retry annotation
                            </Button>
                          </div>
                        )}
                        {selectedTag.detailsOmitted && (
                          <p className={`mt-[6px]! ${note}`}>
                            Some tag details could not be loaded.
                          </p>
                        )}
                        {!usable(selectedTag) && (
                          <p className={`mt-[6px]! ${note}`}>
                            This tag cannot be changed because its name or
                            direct object ID is unavailable.
                          </p>
                        )}
                        <div className="git-tag-actions mx-0 my-[12px] flex items-center gap-[8px]">
                          <Button
                            disabled={disabled || !usable(selectedTag)}
                            onClick={() => {
                              setRemoteName("");
                              setEditor({ kind: "push", tag: selectedTag });
                            }}
                          >
                            Push…
                          </Button>
                          <Button
                            disabled={disabled || !usable(selectedTag)}
                            onClick={() =>
                              setEditor({ kind: "delete", tag: selectedTag })
                            }
                          >
                            Delete local tag…
                          </Button>
                        </div>
                      </div>
                    )}
                  </li>
                ))}
              {page && (
                <li className="list-none">
                  <GitLoadMore
                    scrollOnly
                    cursor={page.nextCursor}
                    loading={pages.loading}
                    error={pages.error}
                    disabled={loading || first.isError}
                    onLoad={pages.load}
                    label="Load more tags"
                    endLabel="All tags loaded"
                  />
                </li>
              )}
            </ul>
          </>
        )}
      </div>
    </GitInspectorSection>
  );
}
