# Backend query state

Current verification: [GPUI state-management completion audit](../../../../docs/audits/gpui-query-state-management.md).
The incremental audit sections below preserve implementation history; their
pending page-audit and native-interaction notes are superseded by that record.
Native comparative CPU/FPS improvement remains unproven.

Application-owned query state in `newport_backend::query`, using the backend’s
existing Tokio and Serde dependencies. This is a backend module, not a separate crate. Inspired by React
Query's stable keys, subscriptions, stale data during refresh, deduplication,
and invalidation. It has no UI dependency and does not own mutations or live
resources such as tunnels, terminal sessions, or Needle installation.

## Ownership

A `Query<T, E = String>` is one typed cache entry. The owning service supplies the key by
where it stores the query. Connections uses `(server session incarnation,
listening ports)`. Parameterized services should store entries under typed
keys including the repository, path, or arguments that affect the result.
Never reuse an entry across credentials or different connection identities.

Each entry has its own coordinator and at most one fetch. Network work runs
outside shared state locks. Two observers of the same entry share work; two
entries do not serialize each other. Loaders must be asynchronous and bounded;
blocking work belongs on the blocking executor. An installation or destructive
operation must never be a query loader.

## API

- `observe(false)` returns cached data immediately and renews demand. It starts
  a refresh when missing/stale, subject to the retry delay after failure.
- `observe(true)` requests a refresh; an in-flight fetch satisfies that request.
- `snapshot()` reads without creating demand.
- `subscribe()` provides watch notifications. Consumers still renew demand
  while active; a forgotten receiver cannot keep background polling alive.
- `read().await` shares a running fetch or returns a fresh cached value. It waits
  for the new epoch after invalidation; structured errors retain their protocol codes.
- `refresh().await` explicitly waits for a fresh result. Dropping the waiter
  does not abandon the shared fetch.
- `invalidate()` retains stale data, cancels the old request, and prevents its
  result from being accepted. Active entries fetch again; inactive entries wait
  for demand. Call this after a mutation that affects the query.
- `close()` immediately hides the retired scope's data and signals cancellation.
  Dropping the query aborts its coordinator and any child fetch.

Snapshots distinguish missing data, fetching, stale data, errors, successful
revision, and the last successful update time. A failed refresh retains data and
its timestamp. Fetch panic/timeout becomes an error with a bounded retry delay.

## Connections

The backend server session owns the port query, using a shared SSH channel;
discovery does not install Needle. Native windows observe the cache through
`connections_query` every second while visible. The backend independently
refreshes every 30 seconds while demand exists, retries errors after 10 seconds,
and bounds each fetch at 35 seconds. Demand expires five seconds after the last
observation. A fetch already running can finish and populate the cache.

The native view keeps a scope-checked presentation copy for immediate return
navigation. It preserves errors and successful timestamps from the backend;
reading a cached value does not make it fresh. Connection replacement and
deletion retire the backend query; old native replies are generation-fenced.
Forward configuration remains committed profile state; a dedicated forwarding
owner publishes live tunnel status and controls listener lifetimes.

## Native read models

- **Overview:** one `Query<Arc<Value>>` in each server session, refreshed every
  three seconds while the application renews background demand. Opening a page
  returns the last observation immediately; multiple first reads/refreshes share
  one fetch. Failed background refreshes retain the last sample timestamp and
  expose `collectionError`; explicit refresh returns its error. Setup/consent
  changes discard pre-install failures; cosmetic and integration edits retain
  the cache. Server retirement, cache clear and worker shutdown close the query.
  History writes check the captured owner and cache generation under the
  per-server history gate, so cancelled pre-clear work cannot repopulate history.
- **Services:** a snapshot and unit-file keys (unit + system/user scope), capped
  at 40 entries per server. Snapshot freshness is 5 seconds; unit files 10 seconds.
- **Containers:** snapshot, inspect by container, disk usage, and Compose project
  keys, capped at 40 entries per server. Freshness is 5 seconds. Explicit refresh
  bypasses freshness. Actions invalidate the affected feature and port discovery,
  including uncertain failures. Cursor logs and operation receipts remain uncached.
- **Files:** directory listings keyed by exact path in a 16-entry server cache,
  fresh for 5 seconds. Navigation shares reads; Refresh and Retry force a reading.
  File operation registration, expected-connection checks and cancellation still
  run on cache hits. Upload completion/cancellation invalidates listings. Previews
  and transfers retain their bounded operation owners rather than a second copy
  of potentially large binary data.
- **Git/Projects:** read models use the exact serialized request (repository,
  cursor, snapshot token, filters and limits) in the owning Git session. Metadata
  and history lanes have 32 entries each; the diff/blob-page lane has 8. Results
  are fresh for 3 seconds. Repository writes invalidate that repository before
  and after execution. Opens/closes, mutations, remote advertisements, legacy
  unpaged blobs/diffs, and durable receipts are not cached. Read cancellation
  detaches its waiter; dropping shared I/O always discards a partially read stream.
  Explicit repository refresh first invalidates that checkout across all cache
  lanes, then dispatches replacement reads after acknowledgement. It requires
  neither SSH startup nor configuration control. Other repositories keep their
  cached pages, and a late refresh reply cannot reload a replacement checkout.
- **Integration/Settings/shell:** permissions, lifecycle operations, saved
  configuration, and live tunnel status remain authoritative state. They must
  not be served from a stale observation cache. Their presentation resources use
  the same data-revision mechanism as query-backed pages.

Native resources distinguish request completion, data projection, and settled
presentation revisions. A repeated cached observation advances polling state
without changing presentation identity. Presentation identity includes errors and
protocol codes, stale/fetching state, manual refresh completion, and the backend's
original sample timestamp. It does not compare reconstructed monotonic timestamps
or treat every background request as visible loading. Shell snapshots and
Connections use that identity to skip redundant window notifications. Snapshot
acknowledgements after settings mutations still reconcile controls even if the
payload is unchanged; snapshot retries preserve an unavailable state until a
successful response. Other controllers still own their operation completion
effects and must not substitute presentation equality for accepting a response.
Services and Containers share resource settlement and distinguish initial/manual
loading from cached background reads. Services snapshots and unit files, and
Container inspect/Compose details/disk readings, notify only when their settled
presentation changes. Restoring Services action capabilities still notifies even
when a post-action snapshot is identical. Container statistics, log sequence
positions and Compose/action receipts retain their completion effects.
Files keeps separate completion and data revisions: identical folder listings
reuse filtering/sorting projections, while pagination callbacks still validate
the latest completion revision.
Git status polling and visible project-library checks also retain cached controls
without showing background activity or notifying for identical observations.
Git observations still reconcile an older displayed page and HEAD even when the
resource itself is unchanged. Opening a transfer still requests a fresh status
review and remote configuration before writing. Expired comparison errors
(`STALE_SNAPSHOT`/`STALE_ENTRY`) invalidate all read lanes of their repository
before returning, so diff recovery cannot reload the still-fresh cache that
supplied its expired token. Other repositories and server sessions remain cached;
transport/authentication errors do not trigger this invalidation or replay writes.
Settings keeps checking updates while hidden, but ordinary read/loading changes
do not notify an unrelated page. Update-ready notifications and explicit metrics
cache clearing still propagate to the shell and affected view state.

`Registry<K,T,E>` bounds parameterized caches, reuses entries by key, evicts the
least recently touched idle/unreferenced entry under pressure, and refuses new
keys if every slot is active. Closing a registry cannot be undone by a late
lookup. Locks protect lookup/invalidation only; no network operation runs under
one. Git and file demand leases expire before their freshness interval, so hidden
historical pages and directories are not periodically fetched.

## Presentation and rendering

The client `Resource` separates request completion (`revision`) from changed
content (`data_revision`). Identical responses still settle operations and update
freshness without invalidating memoized table/chart projections. UI state such
as selection, drafts, focus and scroll position remains in GPUI.

Files filters and sorts the complete listing, then constructs **40 rows per
page**. Overview processes page before rendering and reserve fixed row heights
inside the page's existing scroll container. During prepaint, only rows that
intersect its content mask are constructed and laid out, including partially
visible rows. Sort/filter/pagination controls remain in the normal flow; there is
no nested process scroller. Per-core readings are projected once per server and
query data revision. The core grid reserves its complete height but lays out only
intersecting rows, retaining partial rows and the existing six-pixel grid gaps.
Projection and graph caches are local to the UI owner and require no global lock.
Projects, Connections,
Services, Containers, Git lists/diffs and logs use viewport virtualization. Keep
these paths: do not build every row and hide off-page elements afterward.
Container relative-time labels are calculated only for rendered rows; a new
polling timestamp does not rebuild the entire container projection.

Overview retains chart geometry by data, dimensions and display scale, with at
most 128 chart slots. Scroll frames translate existing vertices instead of
repeating path tessellation; fully clipped canvases skip painting. Theme colors
remain paint-time values. Dashed outgoing network lines are split directly into
segments, avoiding the Lyon measured-path panic on discontinuous runs. Offscreen
process and core layout now follow the viewport; native scrolling still needs another profile;
geometry reuse is not evidence that the entire frame cost is resolved.
The process-row pass reduced the existing 1100×720 offline whole-page benchmark
from 33.1/34.2 ms to 13.0/14.0 ms per CPU frame for 50/5,000 process records.
Both cases retain the same 20-row page size. This fixture has no per-core charts
or metrics history, so these figures isolate unnecessary process-row layout;
they do not establish real-device scrolling CPU usage or chart-heavy performance.
A viewport regression checks exact clipping boundaries, partial rows, footer
geometry, page replacement, narrow resize and empty results. The 27
Overview-filtered and 19 page-state tests pass (overlapping filters, with manual
benchmarks excluded); strict GPUI Clippy passes. The development app was rebuilt,
relaunched and its Overview window inspected. Its selected server remains
unreachable, so live populated scrolling verification is still outstanding.

The populated core/history benchmark (1100×720, 300 history samples, 5,000
process records) measures the following CPU frame times in the Linux test runner:

| Core count / viewport | Before core projection and viewport layout | After |
| --- | --- | --- |
| 32 / top | 52.8 ms | 33.5 ms |
| 32 / bottom | 61.4 ms | 30.5 ms |
| 256 / top | 327.7 ms | 63.1 ms |
| 256 / bottom | 238.7 ms | 32.7 ms |

The benchmark verifies that the top contains core zero and the bottom reaches the
last process on the current page. Tests cover projection identity across query
revisions/server changes, grid spacing, partially visible rows, resize, updated
accessible readings and empty results. All 29 Overview-filtered and 19 page-state
tests pass (overlapping filters); strict GPUI Clippy passes. These are debug/test
CPU frame measurements, not native FPS or Activity Monitor CPU percentages.

The `newport-gpui` example `offline_ui` provides the real Metal-backed UI against
the existing `newport-client` example `needle_ui_probe`, using a disposable
profile with an explicit `offlineFixture: true` marker. Both the fixture file and
live IPC response must identify themselves as offline. A 32-core, 300-sample,
5,000-process fixture was inspected in the native app: charts rendered, scrolling
removed offscreen core tiles, and process pagination advanced. The fixture's
accepted sockets now explicitly use blocking mode so macOS cannot truncate large
responses inherited from a nonblocking listener. The native app was rebuilt and
relaunched, and the disposable probe/IPC processes were stopped afterward.
A native stack sample captured layout/render activity during synthetic scrolling;
comparative native CPU/FPS measurements and the remaining page audit are still
outstanding. The real server’s SSH approval remains separate from this fixture.

There is no new crate, disk query persistence, optimistic mutation layer, or
subscription RPC transport. Local bookmarks/preferences keep their existing
storage transactions.

## Lock migration audit (2026-10-14)

Feature dispatch now resolves the published `ServerSession` through
`manager::SharedState`, independently of the configuration writer. The registry
holds a short synchronous lookup lock, never across network, database, or profile
storage I/O. Each entry retains the latest committed profile metadata separately
from its immutable connection owner. Metadata/setup changes preserve that owner;
credential/destination revisions retire its queries and cancel in-flight work.
File registration subscribes to that retirement before starting work, including
cache hits, so an edit cannot strand work registered during a profile save.

| Surface | Current owner and coordination | Remaining migration |
| --- | --- | --- |
| Overview | Session-owned metrics query; background demand keeps 3-second refresh active. Health and history coordination are scoped per server. Chart geometry and per-core projections are cached; process rows and core tiles are laid out only inside the viewport. | Audit remaining sections/presentation subscriptions and measure native scrolling again. |
| Connections | Port query and shell snapshot resolve without the configuration writer. A dedicated forwarding owner admits starts/stops, saved-forward discovery/batches, conditional browser opens, listener reservations, and retired cleanup generations. | Creation/edits/deletion use optimistic profile commits. Configuration-outage cache retention is verified below; finish the remaining presentation/native audit. |
| Services | Server-owned snapshot/unit queries and a dedicated Services monitor channel. Hidden reads and action receipts settle without notifying the panel; retained changes publish once on return. List elements are built only for GPUI's visible range. | Time-label behavior is verified below; finish native performance verification. |
| Containers / Compose | Server-owned queries with separate Containers and Compose monitor channels. Hidden reads and action receipts settle without notifying the panel; retained changes publish once on return. List elements are built only for GPUI's visible range. | Time-label behavior is verified below; finish native performance verification. |
| Files | Server/path query plus operation-owned cancellation; no manager lock for registration or reads. Recovery journal I/O and local staging cleanup run on owned blocking workers, outside the operation/progress registry. Upload and download journal writers are independent per server; each transfer retains its journal owner. Hidden browser completions drain without page invalidation; tree updates notify their own view, and filters fence their captured connection. | Preview/transfer lifetime and presentation checks are recorded below; finish populated native scrolling/transfer performance verification. |
| Projects / Git | Session/repository/request queries; connection and completion checks use published owners. Explicit refresh and expired comparisons invalidate the repository cache before replacement reads. Cached status polling is quiet; transfer review remains fresh. | Identity lookup is asynchronous and cached after durable publication; there is no shared Sessions identity lock. Finish presentation and native scrolling verification. |
| Terminal | Acquires its published server owner; PTY lifetime stays operation-owned. Retirement cleanup matches the exact connection owner, preserving already-open replacement tabs. Hidden PTYs consume output without notifying GPUI unless tab presentation changes; the workspace shares that presentation selector. | Finish native background-output performance verification. |
| Integration / setup | Inspection, recovery, removal, periodic checks and integration startup/resume use published owners. Identity file I/O runs on a blocking worker; atomic publication handles concurrent starts. Permission gates remain immediate. | Known runtime snapshot fields now invalidate only their selected server and visible Integration/setup surface; lifecycle reads use published immutable snapshots and removal admission is scoped during pending publication. Journal writes run on owned blocking workers with per-server writers, snapshots and failure isolation. Finish the presentation audit and native verification. |
| Settings / shell | Shell snapshots read committed profile publications and captured session owners independently of the configuration writer. Preferences retain dedicated storage ownership; explicit clear-all metrics takes an exclusive history gate. | Profile commits are coordinated only for revision checks and atomic publication. Snapshot acceptance and window invalidation are separated below. Settings now uses shared resource completion revisions for quiet automatic reads; finish the remaining native/presentation audit. |

Overview, Services, Containers and Compose use separate monitor channels per
server. Ordering is retained within each feature's protocol stream; no global
metrics semaphore or cross-feature monitor mutex queues unrelated servers/pages.
An Overview retry resets only its own channel. Setup recovery/removal deliberately
drains all feature channels. The updated Needle starts CPU/process sampling
lazily on the first metrics request, so other feature workers do not duplicate
background sampling; this agent-side optimization applies after Needle is upgraded.

The shell snapshot projection never enters `SharedState::lock`. A publication
pairs profile data with its session owners; runtime collection uses those owners
instead of repeatedly looking up a changing registry. The short publication lock
is released before collecting feature state. A profile publication or newer
accepted observation fences an older collection before assigning its UI revision.
Identical payloads retain their revision. Pending profile writes leave the last
published configuration readable while health and installation progress advance.
Forward precondition checks use the connection identity directly rather than
collecting a whole application snapshot under mutation admission.

Live forwarding commands no longer acquire configuration control. Forwarding
publication atomically reconciles saved tunnel definitions and server credential
revisions with active owners. An edit restarts only forwards that are still active
at commit time; a stop during storage is not undone by an earlier activity check.
Deleted or replaced owners are fenced before releasing their listeners. A short
registry lock reserves conflicting local ports; successor tasks wait outside it
for prior cleanup, including predecessor chains. Another server or nonconflicting
forward can start immediately. Failed profile writes do not publish definitions
or retire active forwards. Persisted configuration edits prepare encrypted files independently, then rebase
their patches if a competing commit changed the configuration.

Integration preparation captures committed metadata with its session owner, reads
the stable client identity outside all owner locks, and rejects retirement before
service admission. Startup uses the owner's latest desired permissions. Setup
inspection and installation completion validate the captured owner, including
credential-only revisions; they do not re-enter profile storage after SSH work.
Explicit approval and setup/preference commits still require durable profile writes.

Production feature dispatch has no general `SharedState::lock` API; that API is
compiled only for tests proving independence. The private configuration coordinator
still serializes the final revision check, prepared-file rename/directory sync and
publication for the single encrypted profile. It is not a per-feature I/O gate.
Encryption, verification, SSH, database cleanup and listener draining run outside
it. This remaining commit boundary and native presentation costs still need an
end-to-end audit; this is not a claim that the overall goal is complete.

Server editing preserves the latest integration/setup consent when rebasing
metadata changes. Credential/destination changes retire only that server's owner
and clear its consent. Existing-server edits do not hold the history gate during
encryption; captured-owner validation rejects a stale edit after deletion or
replacement. Only creation/reuse waits for old history cleanup. File operations
already subscribe to owner retirement;
server edits no longer cancel work broadly by server ID. Terminal cleanup targets
the retired owner, not tabs opened on its replacement. Server deletion registers
owned database/terminal/tunnel cleanup inside the commit worker, so cancelling the
response cannot abandon cleanup after the file was replaced. Its per-server
history barrier remains held until local history deletion completes. This permits
unrelated server saves while preventing UUID reuse from inheriting old cleanup.
The commit callback executes once even if preparation rebases. A failure before
rename cannot run that callback or retire live resources. The explicit clear-all history operation coordinates with
all history users; ordinary server recording/deletion/recreation does not block
another server's history. Short lookup locks are not I/O admission gates.

Tests cover deduplication, bounded keys, scope retirement, mutation invalidation,
structured errors, cancelled protocol streams, stable presentation revisions,
40-row pages over 400 files, global filtering, and retained keyboard navigation.

### Verification limits

The client regression suite passes all 355 tests. GPUI notification measurements
changed from three window notifications for three identical shell readings to
zero; changed snapshot data still notifies the window. Scheduled Connections
admission and repeated cached observations also produce zero notifications.
The 19 page-state tests pass (one manual benchmark remains ignored). The retained
panel pass also passes 22 Services-filtered tests, 115 Containers-filtered tests,
and 94 Files-filtered tests, with their manual benchmarks ignored. Strict
client/GPUI Clippy checks pass. Client regressions verify quiet cached admissions,
unchanged detail readings, manual refresh coalescing, error/recovery updates,
post-action capability restoration and Container statistics advancement. These
are deterministic fixture measurements, not a measured reduction in native
scrolling CPU use.
The subsequent Git pass passes 197 Git-filtered tests and 44 project-filtered
tests (one manual benchmark ignored in each filter), plus two project-check
tests after adding the batch-notification regression. Git status observations
and cached library checks produce zero redundant entity notifications in their
fixtures. Four backend Git query tests cover cache scope, explicit refresh and
expired-token recovery across all four lanes and another server. Backend,
client and GPUI Clippy pass with warnings denied.

The configuration coordinator's five regression tests pass: independent server
and feature edits, preserved newer consent/passwords, metrics-history admission
during metadata preparation, no resurrection by a stale editor, retirement and
shutdown rejection, and owned publication/cleanup after response cancellation.
The deletion/history barrier test and terminal owner-cleanup tests pass. Fresh
discovered/batch creation retains stable IDs and avoids duplicate destinations;
the real SSH forwarding edit/delete/listener-release test passes on the migrated
commit path. Backend Clippy passes with warnings denied, including the production
build without the legacy manager-lock and persistence APIs used by test fixtures.

The Linux verification environment passed the backend suite across the main run,
the corrected permission-persistence timeout test, and the newer-Git CLI fixture.
A real SSH fixture also verified forwarding traffic after repeated saved edits,
listener replacement, deletion, and port release. Backend and GPUI Clippy passed
with warnings denied. The Overview geometry tests and native page-state tests
passed, including repository refresh ordering and stale checkout replies.

The broader GPUI run passed 647 tests initially. Ten syntax/diff failures were
resolved by staging the required Linux grammar library and rerunning those tests.
Three remaining Linux terminal shortcut failures reflect the Windows-only
non-Mac clipboard shortcut branch; they are not a clean cross-platform pass.
The native build with the consent/server-scoping and retained-presentation fixes
completed and was relaunched through the signed development runner. Native UI
inspection of the development bundle confirmed the Overview window renders.
The selected server is waiting for SSH approval, so this verifies launch and
rendering, not loaded live data or scrolling performance. Earlier host loader
stalls are no longer preventing launch. An after-change scrolling profile with
populated metrics remains pending.
Git identity startup now has no shared runtime mutex. The preference clone family
caches only a successfully persisted identity; cold initialization runs on a
blocking worker and cached lookups never enter preference storage. Cancelling a
request leaves its owned persistence worker able to finish. Per-server startup
checks the captured owner and disconnect epoch after that wait, before acquiring
SSH. Ten preference tests cover concurrent durable identity creation, malformed
storage, failed-save retry, cancellation and executor responsiveness. The native
Git suite passes 103 tests (five environment-gated tests ignored), and the two
Git connection lifecycle tests pass separately against a disposable Linux
OpenSSH server. The older Linux container's Git 2.39 cannot run three CLI tests;
those pass with the native Git 2.54 runner. Strict backend/client/GPUI Clippy passes.
The identity refactor was built and relaunched through the signed native runner;
the development window was verified. Its selected remote server currently reports
connection failure, so this confirms startup but does not establish populated
remote-page behavior or an improved scrolling CPU profile.

Lifecycle journal reads now retain an immutable committed snapshot independently
of the file writer. Filtering clones only matching records. While a removal is
being saved, a short publication update closes admission for that server and
connection; unrelated server reads and feature admission continue. A restoration
opens admission only after its completed state is durably saved. All mutation
callers now await owned blocking workers; cancellation cannot leave a persisted
intent without its corresponding publication. Repeated/no-op updates preserve
the snapshot and do not rewrite storage. Invalid requests do not poison valid
committed observations.

Fifteen lifecycle storage tests pass, including blocked publication on a single
async executor, scoped removal barriers, cancelled callers, concurrent server
updates, no-op identity, failed replacement and restart recovery. Five session,
eight control-dispatch and four lifecycle-task tests pass; environment-gated
cases are excluded from those counts. Strict backend/client/GPUI Clippy passes.
Two real SSH Git connection tests and the owned removal/service-restoration test
pass. All nine installation/helper/recovery/release scenarios pass with a fresh
Linux OpenSSH fixture per scenario; sharing one fixture was invalid because
some scenarios intentionally leave tampered metadata or restoration history.
The lifecycle publication refactor was built and relaunched through the signed
native runner, and its Overview window was verified. The selected remote still
reports connection failure; populated native scrolling remains unverified.

Lifecycle persistence is now partitioned into one journal and writer per server.
A stalled save or uncertain replacement quarantines only that server. The shared
operation-ID index holds a short in-memory lock for reservations; it is released
before file I/O or publication. The original journal path becomes a version-two
layout descriptor, with journals under `<path>.servers/<server UUID>/journal.json`.
Existing array-only and version-one records migrate on the first mutation; reads
do not rewrite storage. The migration stages and syncs the complete tree before
publishing its descriptor, preserving the exact source bytes in `legacy.json`.
Only this one-time conversion uses a shared migration gate. Later writes bypass
it. Interrupted migration resumes only from an exact matching source/tree; a
missing or rolled-back descriptor cannot silently reopen stale admission.

All 21 lifecycle storage tests pass. New regressions prove an unrelated server
can finish a durable write before a stalled server is released; operation-ID
reservations remain responsive; invalid shards and uncertain writes isolate
failures; and migration, cancellation and restart preserve recovery barriers.
All nine installation/helper/recovery/release scenarios also pass against fresh
Linux OpenSSH fixtures. The owned-removal SSH scenario and 17 session/control/
lifecycle-task regressions pass after partitioning. Strict backend/client/GPUI
Clippy passes. The 31 Integration-filtered, three setup and 19 page-state GPUI
tests pass (some filters overlap; one manual benchmark remains ignored).
Repeated identical snapshots still produce zero window notifications, while
configuration acknowledgements and lifecycle recovery retain their effects.
The signed development app was rebuilt and relaunched; Overview and Integration
were inspected in the native window. The selected server still reports a
connection failure, so this does not verify populated scrolling performance.
This verifies the scoped persistence boundary, not completion of the full
presentation audit.

These results do not establish completion of the full state-management migration.

## Files recovery ownership (2026-10-14)

File recovery does not hold the operation registry during staging allocation,
journal serialization, fsync, or local staging removal. Each saved server has
independent upload and download journal owners. The shared registry only clones
an owner slot under a short memory lock; initialization and writes run outside
it on blocking workers. Transfers capture their journal owner, so completion
never resolves a different server's records by operation ID.

Each former monolithic journal is now a version-2 `server-journals` descriptor.
Its adjacent `.servers` directory contains `<server UUID>/journal.json` files.
Conversion runs once at profile startup, before transfers exist: stage and sync
the entire tree, preserve the exact former bytes as `source.json`, then publish
the descriptor. A matching interrupted conversion can resume; a changed staged
tree or missing/rolled-back descriptor preserves evidence and refuses writes.
Old monolithic readers reject the descriptor instead of reopening stale records.

Old download records lacked a server identity. They remain in a separate
`unassigned/journal.json`, recovered only at startup; new transfers never write
that journal. Recovery still checks exact staging paths and file identities,
never scans destination directories or replays transfers. New downloads are
recovered from their own server journals at startup, before live transfers.
Upload records additionally validate their server identity and retain the full
connection scope, preventing cleanup on a replacement endpoint or credential.

A corrupt, missing-with-evidence, or unwritable server shard affects only that
server and direction. Temporary journal files stay inside the owner's directory,
so an interrupted write cannot introduce ambiguous files into the shared index.
An invalid top-level layout or legacy source cannot be safely attributed to a
server and blocks that direction while preserving its evidence. The other
transfer direction and file browsing remain independent.

Staging results own cleanup until the transfer accepts them. Cancelling before
worker completion, or abandoning a completed reply, cannot strand a new local
staging file or unused upload registration. Shutdown drains follow-up cleanup
workers as well as the first batch. Normal download errors await staging cleanup;
cancellation releases the caller immediately. An uncertain remote OPEN remains
excluded from retries until restart, preserving creation/removal ordering.

Verification: 43 Files unit tests pass (two separate fixture-dependent tests
remain ignored), including stalled-server isolation, abandoned worker replies,
legacy migration, interrupted conversion, shard failures, and restart recovery.
Backend lib/tests Clippy passes with warnings denied. Six additional tests against
a disposable OpenSSH server pass: upload/download cancellation, upload/download
process-crash recovery, and local/remote source changes during transfer. These
checks cover Files recovery ownership; the remaining native presentation and
all-pages audit are still unfinished.


## Hidden Files and Terminal presentation (2026-10-14)

Terminal I/O remains a retained stream, not a cached request. Each PTY consumes
its bounded output batch, advances Ghostty, answers device queries, and handles
connection events even while hidden. The tick compares the same title, status,
input warning, and active-session presentation used by the tab workspace; plain
hidden output no longer notifies GPUI. Visible output still invalidates its
surface. Debounced titles, reconnect/ready/exit/failure, and input-warning changes
remain observable without selecting the tab.

Files continues polling every retained browser so completed reads and cancellation
work can drain. Only an active browser for the selected server on the Files page
invalidates the page or hands a preview to native preparation. Accepted listings
still update retained canonical paths and data. The folder tree notifies its own
entity without also invalidating the page, and late cancellation replies stay
quiet after deactivation. Deactivation itself still notifies once to disable
mounted controls. Filter callbacks check their captured server and connection
before resetting pagination or notifying the root.

Strict local IPC tests measured three separate hidden terminal output batches:
three entity notifications before the change, zero after, with all text retained.
They also verify device replies, visible-output notifications, debounced titles,
connection transitions and input-warning recovery. Delayed Files replies previously
invalidated unrelated pages; tests now verify quiet background/server/inactive
completions, visible errors, retained data, cancellation draining and stale filter
callbacks. Folder-tree tests verify both disabled controls and quiet late replies.

Verification: 91 Terminal-related tests pass on macOS (one manual benchmark
ignored); 97 Files-related tests pass on Linux (three manual benchmarks ignored).
GPUI lib/tests Clippy passes with warnings denied. The Linux Terminal run passes
84 tests with three clipboard shortcut tests excluded: those tests assume Windows
shortcuts on every non-macOS target while production enables them only on Windows.
The complete macOS Terminal run includes and passes those clipboard checks. These
are notification/correctness measurements, not an Activity Monitor CPU or FPS
comparison. The full all-pages audit remains incomplete.


## Retained Services and Containers presentation (2026-10-14)

Both panels continue draining their scoped client while hidden. They retain read
results, reconcile selection and logs, and deliver action/Compose receipts to the
host immediately. Panel invalidation is deferred with one local pending flag.
On return the flag publishes the latest retained presentation even when the query
observation itself is unchanged. Repeated visible polling then stays quiet. This
uses no shared lock and does not serialize either feature with another server.

Before this change, the delayed-read fixtures each emitted two panel notifications
while hidden. Afterward both emit zero and exactly one on return, retain the changed
snapshot, and do not issue a replacement snapshot request merely for becoming
visible. Separate held-action fixtures verify that a hidden completion delivers
one receipt immediately and never repeats or replays the action on return. Existing
connection-retirement, configuration-outage and dialog-ownership tests also pass.

The audit confirms that Services and Containers request server-owned queries via
published session resolution. Agent exchanges serialize only within that server's
feature channel; Services, Containers, Compose and Overview use separate channels.
The native master lists cache their projections and construct elements only for
GPUI's visible range. The time-dependent label audit below verifies clock behavior
separately from data freshness; native performance measurements remain pending.

Verification on macOS: 44 Services-related tests and 117 Containers-related tests
pass, with one manual benchmark ignored in each filter. Strict GPUI lib/tests Clippy
passes. The subsequent complete native GPUI suite passes 693 tests with no failures;
its 14 ignored tests are explicitly manual CPU benchmarks. This is broad regression
coverage, not completion evidence for all remaining backend/native audit items.


## Service display clocks and container observation time (2026-10-14)

Quiet Services queries exposed a presentation bug: a recent service's elapsed label
stopped advancing when no remote result changed. A native regression test recorded
zero notifications after time advanced, while one visible update was required.
Services now owns a local display clock, independent of query data and revisions.
The last rendered view registers relative labels; each visible poll compares their
formatted text at most once per second. It notifies the Services panel only when
text changes. Rendering a new view drops the previous label set. The clock uses no
shared/global lock, does not spawn a timer, and never requests remote data.

The existing virtual list registers only constructed rows. A 1,000-service native
fixture with elapsed times on the last 500 rows records zero notifications while
those rows remain offscreen. A separate visible-label fixture verifies zero hidden
notifications and one update on return, with no additional snapshot RPC. Pure clock
tests verify minute-level suppression, deadlines, backward time, duplicate labels,
and removal of labels from the current view. Service details and timer accessibility
labels use the same frame timestamp as the list.

Container ages deliberately describe the latest observed snapshot rather than
advancing independently of a disconnected server. Its snapshot completion path
already publishes observations and records statistics even when values match. A
native test disables live logs, receives an identical background snapshot, and
verifies the visible lifecycle label advances from `Running for 1m` to `Running for
2m` with one panel notification and no mutation. This behavior is preserved.

Verification on macOS: 46 Services tests, 118 Containers tests and three display
clock tests pass. The Services and Containers filters each retain one ignored manual
benchmark. Strict GPUI lib/tests Clippy and the native application build pass. These
checks verify notification behavior and virtualized registration, not native FPS or
Activity Monitor CPU usage.


## Scoped shell snapshot presentation and outage retention (2026-10-14)

The backend snapshot revision advances whenever any server's runtime projection
changes. Treating that global revision as window presentation invalidated a page
for another server's agent activity. A native Integration fixture measured three
window notifications for three unrelated-server observations before this change.
The same fixture now records zero, while retaining the entire latest snapshot.

`snapshot_presentation` compares borrowed fields without cloning a second snapshot
or adding synchronization. Known server-keyed runtime maps and SSH approval waits
are compared for the selected server. The global revision is observation metadata,
not displayed state. Configuration, backend identity, load errors, tunnel state
(the sidebar shows the global connected count), and unknown protocol fields remain
observable. This is deliberately conservative for shared UI. The feature-scope refinement below
also separates Integration/setup runtime interests without adding shell entities.

All accepted snapshots still run owner reconciliation. Mutation acknowledgements
and snapshot loading/error changes still notify; equal payloads cannot hide recovery
from a failed request. The native fixture verifies selected permission and health
changes, transport failure, and recovery using identical data. Pure selector tests
cover every known server map, removal, shared fields, and approval-wait membership.

A second regression exposed pruning of cached Connections state before checking
whether the profile was available. An empty server list accompanying a load error
is not deletion. Pruning now follows successful configuration validation. The native
recovery test preserves the filter and cached port reading through an outage,
restores it for the same owner, and refuses it after a connection revision changes.

Verification: the complete native GPUI suite passes 704 tests; its 14 ignored tests
are manual benchmarks. The extended snapshot error/recovery check and strict GPUI
lib/tests Clippy pass as well. This establishes notification and cache-lifetime
behavior, not the remaining native scrolling/performance audit.


## Feature-scoped snapshots and quiet Settings reads (2026-10-14)

The shell selector now distinguishes general pages, the visible setup wizard, and
Integration. Shared server health and connection identity remain observable on all
surfaces. Integration observes its selected server's agent state; the setup wizard
observes installation progress. Other pages retain these runtime updates without
requesting a window redraw. Navigation reads the retained snapshot, and action
handlers continue checking current permissions and lifecycle state. Shared config,
tunnels, errors, acknowledgements, and unknown protocol fields retain the previous
conservative notification behavior.

A native fixture renders each of the nine page content surfaces and delivers three
installation/permission observations for the selected server. Before this change,
the completed Overview emitted three notifications. Afterward, all unrelated pages
emit zero; Integration emits three. Every page retains the final progress. A
separate active-wizard fixture emits all three progress updates while preserving
its installation phase and inspected CPU count. These minimal fixtures exercise
state subscriptions, not populated-list scrolling performance.

Settings previously marked every automatic read admission and completion changed,
even when cached update/cache data was identical. It now settles through the common
`Resource::finish_read` path. Completion identity and freshness still advance while
unchanged data/presentation revisions remain quiet. Read activity is observable for
initial/manual loads and error retries, so Retry controls disable and re-enable
correctly. Writes always publish their own receipts; metrics-clear and update-ready
effects remain independent of presentation suppression. No new lock or worker was
introduced.

The deterministic Settings regression checks three automatic refresh cycles: all
six admission/completion effects stay quiet, all requests run, and freshness advances.
Additional tests cover repeated failures, recovery with unchanged data, explicit
refreshes, and repeated successful mutation receipts. All 16 Settings controller
tests pass, including existing ordering, deduplication, notification and outage
coverage. The complete native GPUI suite passes 706 tests (14 ignored manual
benchmarks), and strict client/GPUI lib/tests Clippy passes.


After rebuilding and relaunching the native application, Settings displayed its
Appearance, Startup, Updates and Cache controls while the selected server was
waiting for SSH approval. The development build correctly reported updates as
unavailable. Returning to Overview preserved the selected server and route. No
preference or remote permission was changed during this UI check.


## Files transfer presentation and completion retention (2026-10-14)

Transfer progress already rejects identical, malformed and regressive readings in
its operation-owned client. The GPUI host nevertheless invalidated other pages
whenever valid progress changed. Its poll now publishes in-flight progress and
cancellation state only while Files is visible. The retained transfer continues
polling; navigating back reads its latest state. Restoring a hidden window notifies
once so unchanged retained progress is painted even without another remote update.
Completed transfers still publish their result and release the UI's busy state.

A separate lifetime bug consumed transfer completion during a configuration outage,
then discarded the receipt because the connection could not be validated. The host
now defers consumption until configuration is available. It first reconciles the
captured connection, then drains the original worker. Same-owner recovery delivers
success exactly once; a replaced or deleted owner suppresses the stale result. The
worker/result channel remains operation-owned and bounded. There is no global lock,
new worker, or replay of the transfer.

Strict local IPC fixtures reproduce both failures before the change. Updated tests
verify off-page and hidden-window progress retention, visible Files updates, retained
cancellation errors, one redraw on window restoration, one completion receipt, and
outage recovery for unchanged/replaced/deleted connections. They exchange protocol
messages only and do not upload, download, or modify actual files.

The preview audit also checked the existing separation of selection token and render
revision, cancellation of superseded reads, and generation rejection for prepared
media. Navigation/connection replacement invalidates preview resources and retires
the owned dialog; late A→B→A responses cannot fill a new selection. The existing
client and native tests cover these paths; no additional preview change was needed.

Verification on macOS: 39 Files client tests and 101 native tests selected by `file`
pass (three native tests are ignored manual benchmarks). Strict GPUI lib/tests Clippy
passes. The native filter includes preview, PDF/text selection, pagination and the
new transfer fixtures; this is not a populated native CPU/FPS measurement. The UI
still retains one foreground transfer at a time; this pass changes its presentation
and result lifetime rather than transfer admission or backend concurrency.

## Populated rendering audit and chart-cache eviction (2026-10-14)

All 14 existing manual CPU benchmarks passed on macOS in the debug test build.
These exercise preparation and GPUI CPU layout/paint, not GPU frame presentation.
Representative small/large dataset results in milliseconds per CPU frame:

| Surface | Small dataset | Large dataset |
| --- | --- | --- |
| Overview processes | 50: 12.22 | 5,000: 13.35 |
| Connections grouped | 50: 27.19 | 500: 28.01 |
| Services | 50: 19.02 | 1,000: 19.11 |
| Containers, 10 per project | 50: 30.86 | 1,000: 33.04 |
| Files folder tree | 50: 9.06 | 2,000: 9.01 |
| Text preview | 50 lines: 2.48 | 5,000 lines: 2.77 |
| Git diff, split | 50 lines: 14.21 | 5,000 lines: 14.25 |

The Files page remains bounded at 40 rows. Its keyboard-reachable controls cost
12.87 ms in the cached scrolling fixture; the synthetic 100-row comparison cost
30.78 ms. The latter exceeds the shipped page size and does not establish a
production 100-row rendering cost. Terminal dense
and emoji fixtures measured 25.20 and 30.22 ms per CPU frame. These figures are
diagnostic debug-build measurements, not release frame-rate guarantees.

Overview's bounded graph cache had a different defect: it retained the first 128
keys indefinitely and refused all later keys. A viewport beyond that prefix rebuilt
its tessellated chart geometry on every redraw. A regression traversing 384 cores
failed on core 128 before the fix. Retention now follows least-recently-used order,
admitting newly visible charts and evicting older ones. The cache stays local to
the Overview state, uses no mutex, retains at most 128 entries, and preserves the
query-driven data invalidation and geometry invalidation on resize/scale changes.
Already-created canvases retain their own graph references through eviction.

The added 256-core scrolling benchmark visits enough charts to exhaust the cache.
Its first, last and return viewports measured 13.55, 12.47 and 12.86 ms per CPU frame.
Regression checks verify reuse beyond capacity, recent-entry retention, safe
eviction, changed readings and layout/scale invalidation. The full native suite
passed 710 tests, with 15 manual benchmarks ignored; after the typed-entry cleanup,
23 Overview tests and strict GPUI lib/tests Clippy passed. App and offline probe
builds both succeeded.

The rebuilt native Metal probe also displayed 256 cores and 5,000 process records
using isolated local IPC. Repeated up/down scrolling reached cores 171–255 with
correct clipped chart rendering. A 20-second, 5 ms-interval macOS `sample` capture
included native `MetalRenderer`, core-grid layout/paint and geometry preparation.
It included idle intervals, so it does not establish sustained CPU percentage or
FPS, nor a before/after native CPU improvement. Capture: `/tmp/newport-chart-audit-scroll.sample`.
Remaining work includes controlled native timing, current-page Files control cost,
and populated presentation/performance checks for the other feature surfaces.

## Files row targets use the current query projection (2026-10-14)

The actual Files rendering path still performed work proportional to off-page
data: each of its 40 rows independently resolved a connection and searched the
entire directory listing for its action target. The keyboard-control benchmark
above did not exercise this path. At the last page of a 5,000-file directory,
the isolated target-preparation benchmark measured 36.90 ms per preparation,
compared with 0.32 ms for 40 files and 2.95 ms for 400 files.

Rendering now validates the directory/connection once and derives row identities
directly from the displayed entries in the retained query projection. It also
borrows the current page slice instead of cloning it before creating row props.
The resulting preparation measured 0.049, 0.039 and 0.032 ms for 40, 400 and 5,000
files respectively. These are preparation timings, not whole-frame or FPS results.
No lock, additional crate, background read or extra retained listing was added.

Click handlers continue to validate against the current connection and listing.
Tests cover malformed entries, transfer capability, changed type, removed files,
pending refresh, inactive browser, page/server switch and replaced connection.
The native `file` test filter passes 103 tests (four manual benchmarks ignored),
including Tab/Shift-Tab reachability of every control on the current page, and
strict GPUI lib/tests Clippy passes.

The rebuilt Metal fixture loaded 5,000 files and navigated through all 125 pages.
Its last page exposed exactly files 4960–4999 with their matching download/actions
labels and disabled Next. Filtering for the final filename produced one matching
row and reset pagination. This used isolated local IPC, with no remote transfer.
App and offline-probe builds passed. Remaining performance work includes sustained
native CPU/frame measurements and the other surfaces listed in the audit table.

## Projects query presentation is scoped to visible rows (2026-10-14)

The Projects search subscribed to every input notification. Focus/caret updates
therefore discarded the list's measured viewport and cached projection, resetting
its scroll anchor without a search edit. It now subscribes to `InputEvent::Change`
for the active library. Actual edits reset the projection and pending check batch;
presentation-only notifications do neither. The regression first reproduced three
unnecessary notifications and an anchor reset, then verified quiet updates on both
Projects and Overview and a single invalidation/reset for a real edit.

Accepted project-status replies previously invalidated the whole shell whenever
their contents changed, even after the row scrolled away. Completion now always
settles the existing query resource, but only invalidates the displayed server's
visible library rows. Hidden windows, other pages/servers, an open repository and
unavailable configuration retain results without a presentation notification.
Identical observations remain quiet; visible errors and changed readings still
notify. A real completion-queue regression failed on an offscreen notification
before the fix and now verifies each of these cases, including hidden recovery.

Polling and completion selection share the library-visibility predicate. Viewport
selection borrows the measured row slice; only the single admitted request clones
its project identity. No new mutex, global lock, crate or query store was added.
The native GPUI suite passes 714 tests, with 16 manual benchmarks ignored.
This establishes the tested presentation behavior, not a native CPU percentage or
completion of the broader Projects/Git state audit. Library retention across
navigation and server-specific search state still need review.

## Per-server Projects library retention (2026-10-14)

The follow-up audit found that route resets discarded the bookmark list, status
reads and measured viewport, while one shell input shared search text across
servers. `project_library_state` now owns each server's input/subscription, list
projection/viewport and suspended resource groups. Ordinary navigation and the
actual server-picker activation path move query resources out before route reset
and restore them only after validating the saved connection identity.

Fresh library reads are reused for 30 seconds. A visible library revalidates in
the background; identical completions do not invalidate the shell, and a cached
background read does not insert a refreshing banner. Explicit Refresh and bookmark
writes retain their visible acknowledgement. Returning from a repository keeps
the last bookmark list while refreshing it. The library remains a virtual GPUI
list; data projection is cached by data revision, search and preferences.

Suspension retires in-flight generations but preserves their last successful
value and errors, including dynamic project-status keys. An abandoned library
refresh is reissued once on return, and an abandoned first status check remains
retryable. Failed observations respect the polling interval. Bookmark saves from
Files mark that server's retained library for revalidation; active bookmark writes
also fence list reads started before the write.

Configuration outages preserve suspended state, including navigation through an
unavailable Projects route. Connection replacement discards cached remote status,
list data and geometry; deletion removes that server's library owner. Search intent
remains local to its server. This adds no mutex, global lock or crate, and reuses
the existing `Resources` generation and presentation rules.

The native GPUI suite passes 718 tests (16 manual benchmarks ignored); all 28
shared-resource tests and strict GPUI lib/tests Clippy pass. Added tests cover
real navigation/server-picker events, distinct inputs and anchors, fresh reuse,
pending reads, late generations, outage/recovery, connection replacement/deletion,
bookmark invalidation and retry timing. The remaining page audit and native
performance measurements are still separate requirements.

Following review of test pacing, the navigation/server-picker regression now
completes GPUI layout/paint and deferred work between each action. It compares
rendered row labels before and after navigation, verifies the other server's
loading state does not display old rows, and checks independently scrolled/search
states after switching back. Controller-only generation tests remain separate.
The strengthened rendering regression and strict Clippy pass.

The rebuilt native Metal fixture loaded 1,000 projects. Search reduced this to
100; after scrolling and visiting Integration, returning restored the search text
and the viewport beginning at Project 0002. Screenshots verified both the
intermediate Integration page and the settled returned Projects page, with pauses
between actions. This is an isolated local IPC smoke check, not a remote-server
or sustained performance measurement. The fixture was stopped afterward.

### Connections configuration recovery and scoped input events

Connections now waits for valid configuration before polling or requesting its
port and project metadata reads. A queued manual refresh survives that pause.
Opening the page during an outage no longer consumes its suspended query group;
leaving again cannot overwrite the saved connection identity and readings with
an empty route. Snapshot recovery restores the matching cache before background
revalidation. Replacement connections still discard the old readings.

Search edit events invalidate only the matching server's visible Connections
page. These changes use the existing per-server state and `Resources` generations;
they add no manager lock or new crate.

Two regression checks failed before the fix (outage polling and reads dispatched
when reopening an unavailable route). The GPUI suite now passes 721 tests, with
16 manual benchmarks ignored; strict lib/tests Clippy passes. Controller tests
cover outage visits, recovery while Connections is open, replacement ownership,
and queued manual refresh. The navigation regression now renders and checks each
intermediate page, including disappearance of old rows after a connection change.
Those rendered assertions are separate from the fast controller-only checks.

The rebuilt development app was relaunched and checked against the configured
server: Connections populated, searching for port 5432 showed the expected
Postgres row, and visiting Integration and returning preserved the search and
row. Each destination was inspected after rendering, with pauses between actions.
The temporary search was cleared afterward. This native smoke check did not
induce a real configuration outage; outage coverage is from the isolated tests.

That audit also identified full-list preparation on redraw: grouped view rebuilt
chunk keys and cloned focus handles, while table view flattened and sorted every
row before its virtual viewport. The follow-up below addresses those paths.

### Connections retains its rendered data projection

Each server now retains one prepared Connections projection, keyed by port and
project-metadata data revisions, a scoped forwarding revision, normalized search, filter,
view mode, sort and collapse settings. Group chunks or sorted table rows and
their keys are shared by `Rc`; a redraw clones those handles, not the full row
collection. Summary counts are computed with the grouped data revision. The
viewport shares its focus-handle collection and reuses it on ordinary redraws
and resize. A new projection invalidates measured row heights even if stable
keys are unchanged; keys still preserve keyboard focus and the scroll anchor.
Only visible rows (plus the existing overscan/focused items) build UI elements.
This is presentation caching over existing query resources, with no new lock or
crate. Actual actions continue to validate fresh server data.

The manual debug CPU-frame fixture, with service-name sorting, measured:

| Total rows | Table before | Table after |
|---|---:|---:|
| 100 | 22.868 ms | 24.313 ms |
| 1,000 | 25.977 ms | 25.028 ms |
| 5,000 | 40.793 ms | 25.806 ms |

Grouped frames remained roughly flat (25–28 ms); no grouped speedup is claimed.
These are ten cached CPU layout/paint iterations per case, excluding native GPU
presentation, not release FPS or sustained scroll measurements.

The GPUI suite passes 724 tests (17 manual benchmarks ignored), and strict
lib/tests Clippy passes. Added rendered tests verify projection and focus-array
reuse across redraw/scroll/resize; query and mode replacement; and table reorder
after project metadata alone changes, then after sort direction changes. Existing
virtualization, collapse, focus retention, width and navigation tests also pass.
The shell snapshot itself survives navigation. Connections' forwarding revision
compares only that server's saved forwards and their transport/health states when
a snapshot is accepted. It preserves cache reuse across unrelated clipboard and
other-server changes, including when the page is reopened. A rendered regression
accepts a forwarding update on Integration, returns to Connections, verifies new
counts/projection, then verifies reuse for unrelated updates and invalidation for
changed health. The wider page audit and sustained native performance checks
remain open.

### Connections project metadata freshness

Connections now revalidates project bookmarks every 60 seconds while visible,
independently of the port query. Its per-server state records bookmark-write
invalidation, including writes completed while another page/server is selected.
Reopening retains the cached labels and refreshes an invalidated list immediately.
An active metadata read started before a bookmark write is fenced by the existing
resource generation; its late reply cannot acknowledge that write. Other servers'
metadata remains untouched. Failed reads retain their last value and use their
completion time for the retry interval, avoiding retry loops against an old
successful timestamp. Identical metadata completions no longer notify the window.
No lock or crate was added.

Two new controller regressions failed before the fix. They now cover independent
polling while port discovery is pending, visibility, request coalescing, failure
backoff, server-scoped invalidation, late replies, and hidden-page restoration.
A separate rendered regression completes layout/paint between replies and checks
both the visible renamed label and zero notifications for unchanged metadata.
The full GPUI suite passes 727 tests (17 manual benchmarks ignored); strict
lib/tests Clippy and the app build pass.

The development app was relaunched. Native checks waited at least 1.5 seconds
between actions and inspected both accessibility state and screenshots. Integration
rendered, then returning to Connections retained grouped rows and the scroll
position while counts refreshed. Earlier mismatched screenshot/accessibility
observations were not counted as passing checks. This smoke test did not rename
real bookmarks; mutation/failure coverage comes from the isolated regressions.
The broader page/lock audit and sustained native performance measurements remain
in progress.

### Overview shares chart series through rendering

The history inspection renderer previously cloned the entire `page` metrics
value just to read `cores`, including its process and per-core histories. It now
borrows that scalar. Prepared chart series are shared immutable `Rc<Vec<Point>>`
values through sparklines, history drawing, hover/keyboard inspection and core
tiles. Rendering clones handles rather than copying each point collection. A
replacement query projection owns new series; older frames keep valid data and
geometry. No synchronization lock or crate was added.

The existing populated debug fixture (5,000 processes, 300 history samples,
ten cached CPU layout/paint frames per case) measured:

| Cores / viewport | Before | After |
|---|---:|---:|
| 32 / top | 29.451 ms | 24.017 ms |
| 32 / bottom | 26.963 ms | 22.760 ms |
| 256 / top | 55.791 ms | 57.384 ms |
| 256 / bottom | 27.433 ms | 23.949 ms |

The dense 256-core top section did not improve; these figures do not establish
release FPS or a universal scrolling speedup. The full GPUI suite passes 728
tests (17 manual benchmarks ignored), including shared-series replacement
ownership, existing geometry invalidation and bounded retention, viewport core
layout, process pagination and history inspection tests. Strict lib/tests Clippy
and the app build pass.

The rebuilt development app was relaunched. Paced native checks confirmed live
32-core charts, history, the process section after scrolling, and resource cards
again after returning to the top. Some computer-use calls reported capture/window
errors; subsequent screenshots verified which actions actually completed. A
30-second native sample captured Overview work and the GPUI Metal renderer, but
includes idle/observation time and intermittent input errors. It is diagnostic
evidence only, not a controlled sustained-scroll benchmark. Native performance
and the broader page/lock completion audit remain open.

### Hidden Services and Containers defer presentation work

Services previously reconciled its selection whenever a query completed, even
after navigation hid the panel. An initial snapshot could therefore prepare the
list and start a new log or unit-file request for an invisible detail pane.
Selection reconciliation now waits until the retained panel is visible. The
client still accepts completed queries and delivers action receipts; returning
uses the cached snapshot and starts only the selected detail read. Existing
post-mutation refresh ownership and connection-retirement fencing are unchanged.

Containers already deferred selection reconciliation, but still projected log
and Compose output into selectable display rows while hidden. Those projections
now wait for visibility too. Completed output remains in the per-server client;
the existing pending-presentation flag publishes it when the user returns. The
redundant log synchronization after visible selection reconciliation was removed.
No lock, crate, polling interval or remote mutation behavior was added or changed.

Four new regressions reproduced the old behavior before the fixes: hidden initial
Services completion with either log or unit-file selection, hidden container log
completion, and hidden Compose output completion. They verify retained query
results, deferred projections/requests, and cached reuse on return. The 31 focused
hidden-page tests pass, including action receipts and credential/deletion fencing.
The full GPUI suite passes 732 tests (17 manual benchmarks ignored); strict
lib/tests Clippy and the app build also pass.

The rebuilt development app was relaunched and its process and agent connection
were verified. Native navigation verification remains incomplete: computer-use
returned only the window shell and a startup screenshot, and rejected a click
with `noWindowsAvailable`, even after rebinding. A process sample showed the main
thread servicing the event loop and periodic ticks, rather than an observed
blocking lock. Neither that sample nor the automated layout tests proves native
page rendering or a CPU improvement. The installed webview app was identified
separately and was not used as evidence for the GPUI build. The broader audit and
native Services/Containers verification remain open.

### Server lookup and idle panel ownership audit

The shell's selected-server lookup now borrows the published server array and
clones only the selected record. Services and Containers ownership checks also
borrow that array, rather than cloning every configured server on each tick.
Connection retirement and configuration-outage behavior are unchanged. This
removes unnecessary global data copying without introducing a cache or lock.

The manual debug benchmark runs 200 selected-server lookups and 200 paired idle
Services/Containers polls with the selected server last in the configuration:

| Configured servers | Lookup before / after | Idle ownership before / after |
| --- | --- | --- |
| 1 | 0.323 / 0.356 ms | 0.757 / 0.442 ms |
| 100 | 22.837 / 4.135 ms | 37.824 / 0.283 ms |
| 1,000 | 225.923 / 36.991 ms | 373.321 / 0.285 ms |

These are aggregate CPU measurements for the named paths, not native frame times.
There is no demonstrated selected-lookup speedup for a single-server profile.
The full backend suite passes 443 tests (106 ignored), and GPUI passes 732 tests
(18 manual benchmarks ignored). Strict GPUI lib/tests Clippy and the app build
pass. The five configuration transaction tests also pass: they cover concurrent
server/consent edits, deletion during preparation, independent encryption, stale
owners/shutdown, and cancellation during durable commit/publication. Feature
queries and Git refresh have separate tests proving progress while the profile
writer is occupied. Final atomic commits for the single encrypted profile remain
serialized; these checks do not claim that profile persistence is lock-free.

An isolated native fixture now verifies initial rendering of 400 Services and
400 Containers, including their selected detail and 120-line log panes. The
fixture socket directory now uses private permissions and the private socket
binder, matching the production client's endpoint validation. No remote calls or
real profile changes were involved. Native input still fails with
`noWindowsAvailable`; screenshots establish initial painting, not navigation,
sustained scrolling performance, or hidden-page behavior. Those interactive checks
and the remaining page audit are still open.
