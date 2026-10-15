# Newport native migration

Native client built with pinned `gpui-kit = 0.7.3`. It uses Newport's
existing Rust backend through private local IPC; SSH, secure storage,
monitoring, and the remote agent remain owned by that backend.

The frontend now lives in the root Cargo workspace at `crates/newport-gpui`.
Its shared lockfile and build output are at `Cargo.lock` and `target/`.
Client and host transport
use Unix sockets on macOS and protected named pipes on Windows. The RPC server,
agent relay, consent progress, and terminal leases live in `newport-backend`
without Tauri dependencies. Metric history also uses a directly owned backend
SQLite pool, preserving the shipping database format. Encrypted profiles and
migration policy also live in the backend, using the same Stronghold engine
directly and the extracted OS credential adapters. The `newport-app` package can
compose this UI and backend in one process. The separate client remains available
for native IPC diagnostics; the Tauri source host has been removed. Complete
cross-platform desktop and production release acceptance remain open gates.

This is an incomplete migration, not an accepted replacement release. The pinned
[released v0.3.0 app](../../resources/test-fixtures/legacy-desktop-release.json) is the desktop
reference for logical pixel dimensions, Retina rendering, typography, icons and
compact controls. The retired React frontend and browser tests have been removed.

## Run

For the standalone application, follow
[`newport-app`](../newport-app/README.md). The instructions below run this crate
as a separate UI client against an already-running backend.

Full Xcode and its Metal toolchain are required on macOS. Run the standalone
`newport-app` first, using an explicit isolated `NEWPORT_DATA_DIR`. For this optional
separate diagnostic client, use that same directory:

```sh
node scripts/build-grammars.mjs
NEWPORT_DATA_DIR=/absolute/path/to/test-profile cargo run --locked \
  --manifest-path crates/newport-gpui/Cargo.toml
```

For a read-only connectivity check, append `-- --check-backend`.

For a macOS development bundle with Newport's icon and version metadata, quit
the native test client, then run:

```sh
NEWPORT_DATA_DIR=/absolute/path/to/test-profile node crates/newport-gpui/scripts/bundle.mjs
```

Open `target/Newport Native Test.app`. The bundle has a separate
`app.newport.native-test` identity and embeds the supplied isolated profile path.
It is a development artifact, not a signed distribution release. Re-run the
script after source changes; its executable is a copy, not a symlink.
The macOS bundle also includes the existing terminal symbol font and its license.
Its `ATSApplicationFontsPath` entry exposes that font to the native text fallback
system. Use the bundle when checking prompt icon fidelity; a bare Cargo executable
does not receive application-bundled fonts.

Do not launch a second backend against a profile already in use. The standalone
host owns the private native endpoint; the released baseline is a separate
acceptance reference, not the backend for this client.

The logical endpoint is `<profile>/native-client/desktop.sock`. On macOS its
directory is owner-only and peers must share the user ID. Windows derives a
named pipe from the profile path and user, restricts its DACL to the logon
session, and verifies peer identity. Requests use bounded length-prefixed
MessagePack frames. Independent calls run off the UI thread.
Writes are never automatically replayed after a transport interruption.

## Implemented so far

- Real server picker and Add/Edit server modals using backend secure storage,
  with Kit authentication/key selectors, agent discovery and scrolling forms.
- Live resource cards, per-core graphs, selectable history ranges, network rates,
  storage, interfaces, and process list following the current main interface.
- Discovered ports, creating forwards, and connecting/disconnecting tunnels.
- Services with grouped, searchable units, timer relationships, scoped actions,
  live filtered journals with selectable text and sticky dates, localized times,
  and searchable unit files. Requests and action ownership live in the shared
  client; native panels retain presentation across navigation and reconnects.
- Container lists with start/stop/restart controls; Compose project
  grouping, service inspection, configuration file viewing, guarded project
  controls, and cursor-based project/container logs.
- Compose up/down/pull/build and service scaling controls, retained-job discovery,
  bounded output polling, and cancellation. Project headers show pending work;
  native confirmation dialogs retain exact targets and protect focus for Down,
  Scale, Stop/Restart and removal. A disposable local Docker fixture
  verified Up, scale to two replicas and Down through the shared backend/agent.
  Final live visual and accessibility comparison remains open.
- File navigation with lazy Home/Server folder branches, filtering, hidden-file
  controls, 100-item pages, and inline project/download actions. Scoped tree reads
  use the shared client with bounded concurrency and cancellation. Kit preview
  modals provide metadata, focus containment, selectable read-only text with
  wrapping, raster images, and native macOS PDF pages with local navigation and
  image-aligned text selection/copy, including rotated pages and ligatures.
  Text selection and scroll position survive ordinary redraws. The shared client
  owns transfers, progress and cancellation. GPUI owns open/save dialogs and
  captures their server, connection and destination; selection cancellation sends
  no transfer request. Both UI and backend reject changed connections before
  starting a transfer. Native process exit closes its chooser.
  Active file transfers are tied to the native request socket: a native-client
  crash cancels unfinished work and runs staging cleanup. Live upload/download
  crash checks on the disposable SSH fixture passed. Network-loss checks also
  passed: failed upload staging cleanup retries on the next connection to the
  same saved server, and interrupted downloads preserve the prior destination.
  Upload staging paths are durably recorded before creation; after a backend
  restart, cleanup resumes on the next matching authorized file connection.
  A real process-kill fixture verifies abandoned-upload cleanup without deleting
  unrelated staging-like files or replaying the upload. Interrupted OPEN requests
  remain excluded from cleanup until restart; active uploads are never cleaned.
  Local downloads also journal their exact staging path and file identity before
  writing payload bytes. Opening the profile after a backend restart removes
  recorded abandoned files, preserves replacement files/symlinks, and never
  replays transfers or touches the selected destination. A real process-kill
  fixture covers partial download recovery and a deliberate retry. A crash in
  the narrow allocation-before-journaling window can still leave an empty file;
  recovery deliberately does not scan for unrecorded files.
- Retained SSH terminal tabs with search, mouse/focus reporting, connection
  recovery, and immersive mode. Tabs rename inline and retain stable action
  targets; text supports word/line/rectangular selection, selection autoscroll,
  copy/select-all and local paging. Find retains its query and case setting when
  closed and reopened in the same active session. OSC 8 HTTP(S) links support
  Command/Ctrl-click through the shared link policy. Terminal parity remains incomplete.
- Create, clone and add existing remote projects, saved project navigation,
  paginated Git views, history details and snapshot-guarded working diffs.
  Repository writes include whole-file staging, commits, branch creation and
  integration, guarded local branch deletion and upstream editing, conflict
  resolution, stashes, worktrees and remote management. Completed repository
  views retain their selection and scroll position across tab/tool navigation;
  writes invalidate suspended reviews. Kit split panes are resizable, with a
  compact list/detail layout and explicit pagination recovery controls. Whole-file,
  hunk and selected-line staging/discard, configurable selectable diffs, branch
  search with bounded prefetch, tag push and interrupted-operation review are
  implemented. Historical inspection supports parent selection, stash views and
  tag annotation recovery without resetting the loaded diff.
- Settings appearance, startup, update progress/restart review and metrics-cache
  controls, with shared request ownership that survives navigation. Production
  update/login acceptance remains gated in the standalone native host.

Remaining Git work includes full source/behavior coverage and same-data visual,
compact, dark and accessibility acceptance; the implementations above do not
establish complete parity. Terminal now groups common grapheme clusters in its grid, with xterm fixture
comparisons and live SSH verification. Grapheme REP, synchronized-output control boundaries and resize continuity have
regression coverage; live OSC 8 opening through the shared backend is verified.
Cell widths and cluster boundaries use the original app's pinned Unicode provider,
with generated property/state comparisons and fragmented-input grid fixtures.
Keydown input is compared against 844 macOS and 1,120 Windows combinations
generated from the original xterm browser/evaluator source, covering modifiers,
US-layout Control input and cursor mode, also checked with LNM enabled. Generate
the Windows reference with `node crates/newport-gpui/scripts/terminal-keyboard-fixtures.cjs --windows`;
omit the flag for macOS. Windows Ctrl+Alt text remains in the platform AltGr
input path; Ctrl+Shift letters do not send unintended remote control bytes.
Remaining gaps include equal-theme glyph comparison, width-shrink reflow of output
below the cursor and same-data visual checks. Cursor paragraphs now retain their
layout, and height growth pulls history only when the cursor is at the bottom,
matching the original. A live SSH width change between
emoji fragments is verified. Native PTYs now use socket ownership leases; twelve
real client crash/reopen cycles preserved another live terminal without leaking
backend slots. The opt-in Rust harness is crates/newport-client/tests/terminal_lifecycle.rs.
ANSI colors, bold/dim/inverse handling, contrast and gray selection now follow
the original xterm source, checked against its 256-color palette and 2,072
contrast fixtures and compared in both live apps. Cell dimensions now use
xterm's device-pixel rounding of measured font width/height; live Retina spacing
and selection align with the original. Complete terminal pixel parity remains open.
Find now draws light/dark match borders, including wrapped and wide-cell matches;
the original terminal does not enable an overview ruler. Files navigation, transfers and previews still need full visual,
accessibility and lifecycle acceptance. Production Settings/host packaging and
remaining interaction parity are also open. Pixel parity is being checked screen
by screen and is not complete.

## Validate

```sh
cargo test --locked --manifest-path crates/newport-gpui/Cargo.toml
cargo clippy --locked --manifest-path crates/newport-gpui/Cargo.toml --all-targets -- -D warnings
```

The focused tests cover shared protocol decoding, request generation isolation,
and pagination validation. These do not replace end-to-end feature testing or
the main application's test suite. Release packaging, cross-platform parity,
and performance comparisons remain outside the current verified scope.

### Native input accessibility check

Run the offline input fixture (no backend, profile, or user data):

```sh
cargo run --locked -p newport-gpui --example input_focus_probe
```

Use an accessibility client to inspect the window, then click and type into
both Fixture search and Fixture preview. The console reports keyboard focus
and the preceding frame's accessibility focus whenever they change. Each
focused input should be reported as the focused text field, with its label;
focus on the whole window or a null accessibility focus is a failure. Merely
finding the two fields in the accessibility tree is insufficient. Closing the
window ends the probe. This manual check complements keyboard and dialog tests.

### Terminal performance checks

The terminal renderer batches backgrounds, shaped glyphs and decorations into
separate paint layers, splitting glyph layers at monochrome/color transitions to
preserve overhang paint order. Character positioning and font fallback still use GPUI's
text system. Interactive input flushes to the bounded transport queue immediately;
the periodic tick retries queue pressure and receives output. Cell-only redraws do
not invalidate the surrounding terminal tab bar. Remote title changes settle for
150 ms before updating the tab label, suppressing brief command/prompt title
flashes without delaying terminal output or input.

Run the offline native visual fixture (no backend or SSH session required):

```sh
cargo run --locked --manifest-path crates/newport-gpui/Cargo.toml --example terminal_render_probe
```

Check the terminal face against the real OS text shaper, then exit:

```sh
cargo run --locked -p newport-gpui --example terminal_render_probe -- --check-metrics
```

This requires a desktop session and checks equal advances for all 95 printable
ASCII glyphs in regular, bold, italic, and bold italic. Run on both macOS and
Windows: GPUI's unit-test text system cannot detect a missing platform font
silently falling back to proportional text.

Run the repeatable CPU frame benchmark:

```sh
cargo test --locked --manifest-path crates/newport-gpui/Cargo.toml --bin newport-gpui \
  benchmark_terminal_frames -- --ignored --nocapture
```

On the development machine, the October 11, 2026 debug-build comparison at
1100 × 650 logical pixels, 60 redraws per case, measured sparse/dense/styled
frames at 120.54/636.72/502.72 ms before batching and 7.58/12.90/11.77 ms afterward.
These measurements include the test renderer's CPU layout and paint work; they
exclude GPU presentation and SSH latency and are not production FPS claims.
The native fixture checks light/dark text, styles, wide characters, combining
marks, emoji, search borders and the wide block cursor. Full grapheme fidelity
and end-to-end performance acceptance remain separate work.

### Page performance review (completed 2026-10-11)

Review order: Overview, Connections, Services, Containers, Terminal, Files,
Projects, Integration, Settings. This is a performance review, not a visual
redesign. Source inspection alone does not certify a page as fast.

Overview: chart preparation already uses the history resource revision, and
process rendering is paginated to 20 rows. Found that chart-hover redraws still
cloned the entire current reading and cloned/filtered/sorted every process.
The page now borrows its reading, and per-server process preparation is retained
by successful response revision, normalized query, sort column and direction.
New readings, invalidation, search and sort replace the cache; ordinary redraws
and pagination reuse it. Failed refreshes retain the current successful reading.

A debug-build synthetic fixture with 5,000 processes measured 27.319 ms per
redraw before the change. The same-run comparison measured 26.567 ms for repeated
preparation and less than 0.001 ms for cached preparation, over 60 repetitions.
These are CPU preparation measurements, excluding layout, paint, GPU presentation,
transport and the first cache fill; they are not end-to-end frame times.
Reproduce with:

```sh
cargo test --locked --manifest-path crates/newport-gpui/Cargo.toml --bin newport-gpui benchmark_process_preparation -- --ignored --nocapture
```

Validation: 19 Overview-related tests passed (two manual benchmarks excluded),
including cache reuse and invalidation on changed data, query, direction, column,
and cleared data. Strict all-target Clippy passed. Existing chart point/gap,
process ordering and pagination regressions passed. Page-content frame measurements
are recorded below; all nine page reviews are recorded below.

Connections: found grouping of discovered listeners and saved forwards repeated
on every redraw. A debug fixture with 500 listeners, 100 forwards and 50 projects
measured 83.970 ms per redraw before caching. The same-run comparison measured
83.868 ms regrouping versus 0.585 ms using the cache, including the owned project
copies still passed to row construction. The cache is per server and keyed by
successful discovery, saved-project and snapshot resource revisions. Query,
category filters and runtime row states are applied after grouping. Action
planning still regroups fresh data. Initial grouping also avoids redundant JSON
copies of the page, projects and saved tunnels.

Reproduce with `cargo test --locked --manifest-path crates/newport-gpui/Cargo.toml
--bin newport-gpui benchmark_connection_grouping -- --ignored --nocapture` (one
shell command). These measurements exclude row construction, layout and paint.
All 22 Connections-filtered regressions passed, plus the manual benchmark; strict
all-target Clippy passed. The new regression checks reuse and replacement for
each input revision and resource invalidation. Grouped and table rows now use bounded virtual lists. Grouped projects are
split into twelve-row chunks, including a single large project; prepared visible
rows are retained by revisions, query and filter. Focused chunks remain mounted,
and row positions refer to the full logical section. Tests cover scrolling to
the last row, focus retention, compact resize, collapse, search and refresh.
Services: the master list was virtualized, but every redraw still rebuilt all
rows and recomputed the default selection. These now share a per-panel cache
keyed by successful snapshot revision, normalized query and System expansion.
Elapsed service and timer labels (including timer accessibility text) are
calculated for visible rows at render time, so cache reuse does not freeze clocks.
A 1,000-unit debug fixture at 898 × 720 measured 26.496 ms/CPU frame before and
19.640 ms afterward (30 forced frames). A later size comparison measured 21.835
ms with 50 units and 21.978 ms with 1,000; timings vary with machine load.

Containers: applied the same retained preparation to rows and default selection,
keyed by snapshot revision, query, project/job expansion and snapshot time.
A fixture containing 1,000 containers across 100 projects at 980 × 720 measured
43.687 ms/CPU frame before and 40.603 ms afterward. A size comparison measured
38.674 ms with 50 containers and 41.042 ms with 1,000. This is a modest improvement;
most remaining cost is outside list preparation. The selected project contains
10 service rows in both fixtures; the separate large-table check is recorded below.

Reproduce the CPU frame benchmarks (each includes layout/paint, excludes GPU
presentation and SSH; fixture RPC uses an isolated local fake backend):

```sh
cargo test --locked --manifest-path crates/newport-gpui/Cargo.toml --bin newport-gpui benchmark_services_frames -- --ignored --nocapture
cargo test --locked --manifest-path crates/newport-gpui/Cargo.toml --bin newport-gpui benchmark_containers_frames -- --ignored --nocapture
```

Validation: cache regressions cover refresh, search, expansion, and current
service/timer labels. The final native suite passes 573 tests (48 library,
523 binary, two integration); 12 manual benchmarks are ignored by the default
suite and were run explicitly. Strict all-target Clippy passes. An unrelated
scale-dialog test expected one running worker despite its exited fixture. The
shared `service_replicas` contract counts running replicas, so its description
expectation now correctly says zero; the input still defaults to one. Production
scaling behavior was not changed.

Files: directory rows cache their filter/sort projection by listing revision,
query and hidden-file setting. The current table keeps its bounded 100-row page
in the focus tree so Tab can reach offscreen files and reveal their rows. This
preserves keyboard access. Each row now owns a GPUI cache with explicit display
and action-target props, so unrelated parent redraws reuse its layout and paint.
For 300 Kit controls, the same-run debug CPU benchmark measures 56.416 → 20.042 ms
for steady redraws and 56.312 → 33.673 ms while scrolling (means over 30 two-frame
samples). GPUI 0.3.8 does not replay native accessibility nodes from cached views;
rows therefore render normally while accessibility is active. That performance
path remains open, and these measurements are not a claim of 60 fps.
Run `benchmark_file_page_frames -- --ignored --nocapture` to reproduce; this
excludes GPU presentation and SSH. Folder navigation retains its row projection
and virtualizes long lists.
Text previews retain a read-only editor without replacing the document on
redraw. Offline CPU frame checks measured 11.796/11.724 ms for 50/2,000 folders
and 2.962/3.435 ms for 50/5,000 text lines. Folder tests also prove offscreen rows
stay unmounted and redraws make no extra directory requests. No new optimization
was justified for these measured paths.

Projects: the bookmark list was virtualized but cloned, filtered, sorted and
compared all bookmark data on every redraw. Its projection is now retained by
successful resource revision, query and preferences. Resize only remeasures row
geometry, and the Check status callback copies bookmarks only when invoked.
The 5,000-bookmark benchmark measured 32.376 ms before; the same-run comparison
measured 33.355 ms versus less than 0.001 ms cached. Actual page-content CPU frames
with 5,000 bookmarks measured 24.758 ms. Tests cover data refresh/failure,
favorites, sorting, search, resize, scroll anchors and virtualized long names.
Git diff presentations/documents already retain decoded data and virtualize rows:
50 versus 5,000 context lines measured 8.296/8.217 ms unified and 13.990/14.251 ms
split. These warm-frame measurements exclude first-load decoding and text width
measurement. Git listing/pagination regression coverage passed in the full suite.

Integration: source review found two fixed feature controls and bounded status
content, with mutations sent only from actions. Whole page-content CPU frames
measured 4.458 ms with both features enabled and a connected status. No change
was justified. Settings uses retained inputs and shared bounded polling; its
loaded-page benchmark measured 3.446 ms and verified redraws dispatch no actions.
Settings regressions cover polling ownership, update/restart and cache controls.

The reusable `page_perf_tests` fixture constructs an offline page owner without
startup requests, periodic timers or persistence hooks. Its clients point at a
private nonexistent socket. It measures the actual page-content renderers at
1100 × 720, excluding navigation chrome, GPU presentation and remote latency.
Overview measured 31.906/33.093 ms for 50/5,000 processes with empty history; chart
preparation and point/gap handling have separate coverage. Integration and the
project library results above come from this fixture.

Connections whole-page CPU frames originally measured 77.393 ms for 50 listeners
and 733.659 ms for 500. With virtual rows and retained presentation, the same
fixtures measured 30.057/31.895 ms; a single 500-listener project measured 19.589 ms,
and table mode measured 23.085 ms. Toolbar and table headers stay above the
scrolling row viewport. These are debug-build CPU timings, not production FPS.

The targeted Compose baseline found a single 1,000-service project took
1,980.838 ms per CPU frame, versus 41.207 ms for 1,000 containers spread over
100 projects. Its service table built every row and reshaped all column values
on every redraw. The current fix retains row/column preparation and virtualizes
large tables within a 300-pixel body; tables of twelve rows or fewer retain their
natural height. The project header and action controls also counted missing definitions by
constructing every replica row; they now compare defined service names with the
existing service set directly. The final 1,000-service fixture measures 39.594 ms
per CPU frame (about 98% less than the 1,980.838 ms baseline). The final smaller
fixtures measured 40.403 ms (50 containers) and 41.235 ms (1,000 across projects).

Regression coverage verifies bounded mounted rows, reaching the last service,
focus retention across scrolling, retained row identity on redraw, and refresh
from 100 rows down to two. Existing tests verify missing definitions, profiles,
replicas, orphans, natural column widths, horizontal scrolling, scoped actions,
and keyboard navigation. All 573 default tests and all 12 manual benchmarks
passed, as did strict all-target Clippy. The native test bundle was rebuilt.

Review coverage matches all nine entries in `Page::ALL`: Overview, Connections,
Projects (including Git diffs), Services, Containers (including large Compose
tables), Terminal, Files (tree and text preview), Integration and Settings.
These checks exercise synthetic fixtures and current regression coverage;
they do not certify every remote workload or measure GPU presentation latency.

Run all current manual benchmarks sequentially to reduce contention:

```sh
cargo test --locked --manifest-path crates/newport-gpui/Cargo.toml --bin newport-gpui benchmark_ -- --ignored --nocapture --test-threads=1
```

The Terminal benchmark was re-run: sparse/dense/styled frames measured
7.73/13.18/12.07 ms. All measurements here are debug-build CPU measurements on the
development machine, not production FPS or end-to-end latency claims.
