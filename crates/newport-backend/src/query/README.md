# Backend query state

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
  and after execution. Opens/closes, mutations, remote advertisements,
  unpaged blobs/diffs, and durable receipts are not cached. Dropping a waiter
  detaches it; dropping shared I/O always discards a partially read stream.
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

Overview retains bounded chart geometry by data, dimensions, and display scale.
Scroll frames reuse it; fully clipped canvases skip painting. Hidden pages retain
results while deferring visible projections and notifications until needed.
These bounds reduce redundant work but do not establish real-device CPU/FPS.

## Coordination

Feature dispatch resolves a published `ServerSession` through
`manager::SharedState`. Registry locks protect short lookups, never network,
database, or profile I/O. Credential/destination changes retire that server's
connection owner and fence late results; metadata-only changes preserve it.

Overview, Services, Containers, and Compose use separate monitor channels per
server. Ordering is retained within a feature stream without queuing unrelated
servers or features. Setup recovery/removal deliberately drains the affected
server's channels. PTYs, transfers, forwards, and installations retain their own
operation lifetimes rather than becoming cache entries.

Profile persistence still serializes the final revision check, prepared-file
rename/sync, and publication for the single encrypted profile. Preparation and
remote work happen outside that coordinator. Scoped history gates coordinate
server deletion/reuse; explicit clear-all history coordinates all history users.
Cancellation of a response does not abandon cleanup after a durable commit.

Tests cover deduplication, bounded entries, scope retirement, mutation
invalidation, cancellation, retained stale data, stable presentation revisions,
and visible-row construction. Use the [testing guide](../../../../tools/xtask/testing.md)
for current checks; historical benchmark totals are not release acceptance.

There is no separate query crate, persistent disk query cache, optimistic
mutation layer, or subscription RPC transport. Bookmarks and preferences retain
their existing storage transactions.
