# Newport terminal engine

This crate owns terminal emulation and protocol encoding using the public
`libghostty-vt` C API through pinned safe Rust bindings. It has no GPUI, window,
PTY, or SSH dependencies. GPUI paints owned viewport snapshots; the existing
Newport SSH session sends and receives bytes.

## Build

```sh
node scripts/with-ghostty.mjs cargo test --locked -p newport-terminal
node scripts/with-ghostty.mjs cargo clippy --locked -p newport-terminal --all-targets -- -D warnings
```

For an optimized CPU stream benchmark:

```sh
node scripts/with-ghostty.mjs cargo run --locked --release -p newport-terminal --example benchmark_stream
```

It reports three runs each of ASCII, ANSI-styled, and Unicode output, at least
4 MiB per run in 4 KiB chunks, capturing a 160×48 viewport after every chunk.
CSV results include throughput and batch-duration percentiles. Engine creation,
warm-up, and the final responsiveness assertion are outside the measured loop.
The loop includes parsing, history pruning, render-state updates, and owned
viewport data; it excludes font shaping, GPUI painting, GPU presentation, IPC,
and SSH. These numbers are not physical input-to-display latency. Run it after
builds finish, and record the source/binary hashes with measured results.

The wrapper uses Zig 0.16.0 from PATH when available, or downloads its official
archive and verifies the pinned SHA-256. It caches dependencies in
`target/ghostty`, without installing system software. A first build needs
network access for Rust, Ghostty, and Zig package dependencies. Later builds can
use the caches. `GHOSTTY_SOURCE_DIR` can point to an existing checkout, but its
revision must match the pin. The wrapper archives that pinned tree and applies
Newport's small compatibility patches into a separate, content-addressed cache.
It never edits the supplied checkout. See [patches/README.md](patches/README.md).

On Windows, the wrapper also copies Zig 0.16.0's standard library into the build
cache and adds the missing alignment assertion for dynamically loaded ntdll
function pointers. This fixes ARM64 compilation of the stack-trace code while
retaining `ReleaseSafe`. Both the original and patched file have pinned hashes;
an unexpected source or modified cache fails the build. The installed Zig
standard library is not changed.
The [verified alignment report](../../docs/prototypes/zig-windows-alignment/README.md)
contains an independent reproducer, comparisons across three Zig versions, and
a Windows ARM64 stack-trace runtime check.

The ARM64 Zig 0.16.0 host executable crashes in the Parallels Windows acceptance
VM, including with an independent `zig init` project. The acceptance build uses
the official x64 Zig 0.16.0 executable under emulation and the x64 Rust 1.98.0
host toolchain with `CARGO_BUILD_TARGET=aarch64-pc-windows-msvc`. Both host tools
must be x64: the bindings emit Zig's explicit ARM64 target only when Rust's
host and target differ. Merely replacing Zig in an ARM64-host Cargo invocation
can compile the library for the wrong architecture.

- Rust bindings: `Uzaaft/libghostty-rs` at
  `d2036baeb605d31d09a9e2ba6d0c923e04c34d54`.
- Ghostty: `3425025e585a3403340d4dc6d65132f42e05e605`.
- Static linkage, portable baseline CPU, Kitty graphics disabled until there is
  a renderer for them. No Ghostty desktop runtime is embedded.
- The wrapper uses `ReleaseSafe` for Ghostty even in a Cargo development build;
  its full debug invariant checks are too expensive for interactive use.
  `LIBGHOSTTY_VT_SYS_OPTIMIZE` can explicitly override this.

Update the bindings, Ghostty source pin, and build checks together: the C ABI is
not stable across arbitrary upstream revisions. Update the corresponding native
dependency notices in `resources/licenses/terminal` when changing these pins or
enabling additional features. Desktop packaging includes them under `Licenses`.

## Boundary and behavior

- `Engine` owns the parser, primary/alternate screen, selection, and input
  encoders. Its thread confinement follows libghostty; do not share it with SSH
  workers. Pass incoming bytes to the UI thread instead.
- `frame()` updates dirty rows in a retained render state. Its owned cell strings
  never borrow memory that a subsequent terminal write can invalidate.
- Protocol replies, title updates, and bell events are returned as `Effects`.
  Resizing can produce replies too; callers must dispatch its effects.
- Synchronized output captures the complete pre-hold frame in the callback and
  expires after one second. Poll `flush_expired()` even when no SSH output arrives.
- Grapheme clustering is a reset default. Theme defaults preserve remote OSC
  color overrides. Reconnection aborts incomplete control sequences and clears
  transient TUI modes while retaining the primary buffer.
- OSC clipboard handlers are deliberately absent. Clipboard access comes from
  explicit UI copy/paste actions. OSC links are filtered through Newport's
  shared HTTP(S) URL policy.
- Primary-screen resizing preserves output, including rows below the cursor;
  this is a user-approved difference from the old xterm edge-case behavior.
- Scrollback uses Ghostty's page-based 5,000-line retention policy. Pruning can
  remove a complete page, so the retained count is approximate. Resize tests
  separately require that every already-retained row survives growth.
- A cell retains at most 64 combining suffix codepoints, following Ghostty's
  resource bound. Additional marks are discarded; subsequent text, selection,
  search, and cursor movement must continue normally.

## Migration status

The production GPUI terminal view, painter, input dispatch, selection, find,
and Git log text renderer now use this engine. Desktop acceptance is not yet
complete. The compatibility suite checks the original inputs with individually
reviewed Unicode/reflow contracts; passing it does not establish native desktop
input, rendering, packaging, or lifecycle acceptance on every platform.

Remaining acceptance gates:

- Alacritty/VTE coupling and local forks are removed. Keep the original fixtures
  and migrated contract tests as release gates when updating the engine.
- Preserve Unicode/case-sensitive find, drag/word/line/block selection,
  autoscroll, IME, platform shortcuts, links, and accessibility.
- Wire key press/repeat/release and mouse events to the Ghostty encoders without
  double-sending native text-input events.
- Verify the renderer's colors, cursor, decorations, fonts, fixed-cell geometry,
  scrollback, resize, and detached-window behavior on macOS and Windows.
- Run SSH lifecycle tests and real native-app acceptance, including reconnect,
  close-to-tray/restore, and output retention.
- Verify packaging, CI, and clean builds on macOS ARM64, Windows ARM64, and
  Windows x64; record terminal throughput and input latency.

### Local verification — 2026-10-13

- All 61 core tests pass, including independent Unicode contracts, fragmented
  control sequences, clipped clusters, search, selection, and resize retention.
- The macOS offline renderer was visually checked in light and dark themes.
  Block-cursor text now reverses the theme colors with a contrast fallback for
  remote OSC cursor overrides; the previous fixed black text was unreadable
  against a dark cursor. The five color tests pass.
- The native app builds, and an isolated ad-hoc macOS development package passes
  bundled grammar/PDF checks, signature verification, and archive integrity.
  This is not release signing/notarization or Windows packaging acceptance.
- Packaging includes the Ghostty, Rust binding, Unicode, and SIMD dependency
  notices. Seven source-preparation/packaging tests pass; all 11 terminal notice
  files were also checked byte-for-byte inside the isolated macOS archive.
- All five underline styles render distinctly, retain SGR colors, and avoid
  duplicate strokes on wide cells. Geometry tests cover 1×/1.5×/2× scales;
  the native light/dark preview was visually verified at 2×.
- The disposable SSH fixture passes input/resize/interrupt/close, interrupted
  session reconnection, and shared-transport terminal tests. These are backend
  lifecycle checks, not proof of complete native-window input acceptance.
- The scoped GPUI terminal run passes 78 tests (one manual benchmark excluded).
  Option/AltGr and Shift-page navigation no longer intercept releases belonging
  to keys already forwarded to a terminal application. Native composition and
  local scroll handling remain in place for presses and repeats.
  Focus loss and tab deactivation synthesize releases with the window’s current
  modifiers; an IPC regression verifies both transitions and rejects duplicate
  releases from late key-up events.
- In the isolated physical-key candidate, native macOS Wubi input passes a
  compose/cancel/compose/commit check in both legacy and Kitty keyboard modes.
  Marked text and cancellation send no IPC bytes; the committed UTF-8 character
  matches an independent Cocoa text-view control and is not duplicated. This
  verifies one native IME path, not all layouts or Windows input. The initial
  Option+E dead-key fixture was inapplicable to the active Wubi input source;
  Cocoa reproduced its immediate accent commit, so it was not counted as a
  Newport failure or a successful dead-key acceptance check.
- Native US Option+E, E dead-key input also passes in both legacy and Kitty
  modes with the final fixture. It waits for the painted frame to install the
  text-input handler and checks the active input source before injection.
  Preedit sends no bytes, committing sends exactly `é`, and no duplicate arrives
  during the 500 ms observation interval. These are isolated-candidate checks;
  other layouts and Windows input still require acceptance.
- The original grapheme fixture JSON is unchanged and every input is exercised.
  All 17 compatibility tests pass with explicit expectations backed by independent
  REP/Unicode/control/resize contracts. Reflow contracts check logical paragraphs,
  including spaces and history, before new input and verify cursor/write behavior
  afterward. The Prepend engine defect is fixed by patch 005; its original
  expected cells are retained. The keyboard fixture suite also passes.
- The debug GPUI test renderer measured 15.54–31.75 ms/frame over 60 frames per
  sparse/dense/emoji/styled case. This includes CPU layout and paint recording,
  excludes GPU presentation and SSH, and is not a production latency result.
- A fresh optimized macOS ARM64 run measures 1.58–3.27 ms per CPU frame across
  those fixtures. The separate streaming benchmark measures 11.905–15.499 MiB/s
  including a viewport capture after each 4 KiB chunk, with batch p99 below
  0.5 ms in these runs. See the [source-hashed release report](../../docs/benchmarks/ghostty-macos-arm64-2026-10-13/README.md)
  for raw results, scope, and limits; neither result measures GPU/SSH latency.

The GPUI dependency now exposes physical-key identity independently of normalized
shortcut text. Changing Shift before key-up can change that text on both macOS
and Windows; the stable identity pairs the release with its forwarded press
without assuming a US keyboard layout. The original event patch
passes isolated GPUI dispatch, three Newport IPC regressions, and a native macOS
build. Native macOS AppKit events also retain identity across press, repeat,
and release; targeted CGEvents verify punctuation and key code zero. These
controlled events do not establish complete IME/layout acceptance. The isolated
terminal suite passes 78 tests (before the underline changes).
The patch is now pinned through `ruiyangke/gpui-kit` on `newport/compat-main`
(Kit 0.7.3 with the patched GPUI 0.3.9 snapshot). Newport uses the native metadata
adapter in normal builds. Native Windows acceptance remains outstanding; source
and historical validation records are in `docs/prototypes/gpui-physical-key`.

The expanded isolated candidate also preserves native unmodified layout text
and raw modifiers independently of GPUI shortcut normalization. Its scoped
terminal suite passes 85 tests (one manual benchmark excluded). Actual macOS
Shift+1 press/release events produce the expected Kitty base/shifted identities,
and US dead-key composition still passes legacy and Kitty modes without
duplicates. The patched GPUI Windows backend compiles for both ARM64 and x64;
this is not a full Windows app build or desktop acceptance. Focus-loss and tab
deactivation releases now query current Windows Num Lock state; the regression
covers on/off changes since the press and duplicate late key-up prevention.
Strict Clippy including dependencies passes with pinned Rust 1.98.0, as do two
upstream event/query tests and the repeated native macOS fixtures. These
candidate changes are not yet wired into production dependencies.

A Windows ARM64 cross-build attempted on macOS stops at missing MSVC C/C++
headers (`cstring` and `stdlib.h`), before the Rust terminal crate can complete.
This is not a Windows acceptance pass; native Windows builds remain required.
