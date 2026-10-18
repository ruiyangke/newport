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
`.cache/newport/ghostty` (outside Cargo's pruned target cache), without installing
system software. A first build needs network access for Rust, Ghostty, and Zig
package dependencies. Later builds can
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

## Validation

The GPUI terminal, painter, input dispatch, selection, find, and Git log text
renderer use this engine. Retain the independent Unicode/reflow contracts and
generated terminal fixtures when updating it.

Test native keyboard layouts, IME composition, shortcuts, links, accessibility,
selection, scrollback, resize, and detached windows on macOS and Windows. Key
press/repeat/release handling must not duplicate native text input. Stable
physical-key identity pairs releases with forwarded presses even when modifiers
change; focus loss and tab deactivation release forwarded keys.

Engine tests and CPU benchmarks exclude the OS event loop, GPU presentation,
SSH, and signed packaging. Verify those separately through the
[desktop testing procedure](../../docs/testing.md). Record measured results with
the exact source, binary, platform, and workload outside this source guide.
