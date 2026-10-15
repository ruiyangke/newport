import { expect, it } from "vitest";
import { gitPath } from "./git";
import {
  decodeGitBlob,
  decodeGitRemotes,
  decodeGitRemoteRefs,
  decodeGitStashes,
  decodeGitTags,
  decodeGitWorktrees,
} from "./gitResponses";
const oid = { format: "sha1", hex: "a".repeat(40) };
const page = (entries: unknown[], metadata: unknown = {}) => ({
  snapshot: "snapshot",
  nextCursor: null,
  entries,
  metadata,
});
it("preserves unavailable worktree data without treating it as unlocked or prunable", () => {
  const result = decodeGitWorktrees(
    page(
      [
        {
          name: gitPath("moved"),
          kind: "linked",
          path: null,
          gitDir: gitPath("/repo/.git/worktrees/moved"),
          current: false,
          state: "invalid",
          head: null,
          locked: null,
          lockReason: null,
          prunable: null,
          lockReasonUnavailable: true,
        },
      ],
      { listToken: "token" },
    ),
  );
  expect(result.entries[0]).toMatchObject({
    locked: null,
    prunable: null,
    path: null,
    head: null,
    lockReasonUnavailable: true,
  });
  expect(result.metadata.listToken).toBe("token");
});
it("accepts bounded tag metadata without inventing a missing target or annotation", () => {
  const result = decodeGitTags(
    page([
      {
        name: gitPath("release"),
        reference: gitPath("refs/tags/release"),
        oid,
        symbolicTarget: null,
        annotated: true,
        detailsOmitted: true,
        objectType: "tag",
      },
    ]),
  );
  expect(result.entries[0]).toMatchObject({
    detailsOmitted: true,
    message: null,
    targetOid: null,
    peeledOid: null,
    tagger: null,
  });
});
it("keeps stash identity and token independent of its changing list index", () => {
  const result = decodeGitStashes(
    page(
      [
        {
          index: 2,
          oid: oid.hex,
          previousOid: "0".repeat(40),
          message: "WIP",
          messageTruncated: true,
          time: -1,
        },
      ],
      { listToken: "stash-version" },
    ),
  );
  expect(result.entries[0]).toMatchObject({
    index: 2,
    oid: oid.hex,
    messageTruncated: true,
    time: -1,
  });
  expect(result.metadata.listToken).toBe("stash-version");
});
it("decodes remote configuration and advertised references separately", () => {
  expect(
    decodeGitRemotes({
      entries: [
        {
          name: "origin",
          url: "ssh://host/repo",
          pushUrl: null,
          token: "config",
        },
      ],
      authentication: { ssh: "server_agent", https: "anonymous" },
    }).entries[0].pushUrl,
  ).toBeNull();
  const refs = decodeGitRemoteRefs(
    page(
      [
        {
          reference: gitPath("HEAD"),
          kind: "head",
          oid,
          symbolicTarget: gitPath("refs/heads/main"),
        },
      ],
      {
        remote: "origin",
        remoteToken: "config",
        forPush: false,
        basis: "remote_advertisement",
        truncated: false,
      },
    ),
  );
  expect(refs.entries[0].symbolicTarget?.bytesB64).toBe(
    gitPath("refs/heads/main").bytesB64,
  );
  expect(() =>
    decodeGitRemoteRefs(
      page([], {
        remote: "origin",
        remoteToken: "config",
        forPush: false,
        basis: "stored_refs",
        truncated: false,
      }),
    ),
  ).toThrow(/basis/);
});
it("checks blob content length, canonical encoding, and truncation semantics", () => {
  expect(
    decodeGitBlob({ oid, size: 2, truncated: false, bytesB64: "/wA=" })
      .bytesB64,
  ).toBe("/wA=");
  expect(
    decodeGitBlob({ oid, size: 900000, truncated: true, bytesB64: null })
      .bytesB64,
  ).toBeNull();
  for (const content of [
    { size: 3, truncated: false, bytesB64: "/wA=" },
    { size: 2, truncated: true, bytesB64: "/wA=" },
    { size: 2, truncated: false, bytesB64: null },
    { size: 2, truncated: false, bytesB64: "/wA" },
  ])
    expect(() => decodeGitBlob({ oid, ...content })).toThrow();
});
