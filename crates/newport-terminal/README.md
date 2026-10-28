# Newport terminal engine

This crate integrates the unmodified official `alacritty_terminal` source at
[`542147067dd016fc5f49120ef0ba605a98a4e3a7`](https://github.com/alacritty/alacritty/commit/542147067dd016fc5f49120ef0ba605a98a4e3a7)
(`0.26.1-dev`). There is no fork, source patch, build-time patching, or Cargo
patch override for Alacritty. Cargo.lock pins its transitive dependencies.
The upstream revision includes the keyboard-mode stack fix missing from
published 0.26.0.

Alacritty owns VT parsing, screen buffers, scrollback, terminal modes, reflow,
and selection. Newport's parser handler (`src/handler.rs`) wraps Alacritty's
`Term`: every sequence reaches it, and the handler adds grapheme clustering
and the modes listed below. Newport delivers SSH bytes to the engine and paints
owned snapshots in GPUI. This integration does not spawn Alacritty's PTY or
event loop.

Zed's [terminal integration](https://github.com/zed-industries/zed/tree/main/crates/terminal)
informed the separation between emulator events and UI responsibilities. Newport
uses official upstream rather than Zed's fork and contains no copied
Zed implementation.

## Build and validation

```sh
cargo test --locked -p newport-terminal
cargo clippy --locked -p newport-terminal --all-targets -- -D warnings
cargo test --locked -p newport-gpui --lib terminal
cargo run --locked --release -p newport-terminal --example benchmark_stream
```

The benchmark parses at least 4 MiB of ASCII, ANSI-styled, and Unicode output
per run in 4 KiB batches, taking a 160×48 snapshot after each batch. It measures
CPU parsing, history retention, and viewport conversion, not GPU presentation,
SSH, or input-to-display latency. Test native layouts, IME, mouse selection and
resize separately using the [desktop testing procedure](../../tools/xtask/testing.md).

## Integration boundary

- The UI thread owns `Engine`. SSH workers send bytes rather than sharing it.
- Damage tracking refreshes changed rows; selection masks are refreshed without
  rebuilding cell strings. Copied frames remain valid after later writes.
- `Effects` carries protocol replies, title changes, bells, and clipboard copies.
  Callers dispatch effects from writes, resize, and `flush_expired()`.
- Synchronized output uses VTE's buffer and its 150 ms timeout. Search sees the
  same completed grid as rendering. Poll `flush_expired()` while idle.
- Theme defaults are separate from application OSC overrides. Reconnect drops
  incomplete parser state and transient modes, retaining primary-screen output.
- OSC 52 permits nonempty UTF-8 copies to the standard clipboard, limited to
  8 MiB. Remote clipboard reads, clears, and primary-selection writes are ignored.
- Explicit OSC 8 links and detected URLs use Newport's shared HTTP(S) policy.
  Concealed cells expose neither accessible text nor actionable links.
- Host keyboard and mouse encoders (`src/input.rs`) use the negotiated modes;
  Alacritty's library does not encode platform key events. No app shortcuts
  are installed here. Native composition and minimal app shortcuts remain in
  GPUI.

## Terminal behavior

The unit and contract tests in `tests/` specify each of these.

- **Grapheme clusters.** Cells follow Unicode extended grapheme clusters: ZWJ
  emoji, skin tones, flags, keycaps, Indic conjuncts and Prepend characters
  each occupy one cell. A cluster is two columns wide when it contains a wide
  character, an emoji presentation selector (U+FE0F), regional indicators, or
  more than one spacing character; otherwise one. Sequences that do not move
  the cursor (SGR, titles, other OSC, unknown sequences) do not split a
  cluster that is still arriving. Mode 2027 is set by default; resetting it
  gives per-character widths, and a full reset restores it.
- **Resize.** Shrinking the height moves output below the cursor into history
  instead of dropping it; the cursor and saved cursor stay on their text, or
  move to the top row when their text no longer fits. A width change keeps the
  cursor on its own character and keeps lines above it on screen.
- **Screens and modes.** Modes 47, 1047 (clears the alternate screen on exit)
  and 1049 switch screens; 1048 saves and restores the cursor. Reverse video
  (mode 5) swaps the default colors. DECRQM reports these modes.
- **Keyboard.** Control bytes stay conventional. Without the Kitty protocol,
  Enter, Tab and Escape with Ctrl or Shift send `CSI 27;<mods>;<code>~`
  (Shift+Tab stays backtab), Ctrl chords without a control byte send
  `CSI <code>;<mods>u`, and F3 with modifiers sends `CSI 13;<mods>~`. xterm's
  modifyOtherKeys (`CSI > 4;1 m`, `CSI > 4;2 m`) selects the `CSI 27` form.
  Kitty reports carry no associated text for Ctrl, Alt or Super chords.
- **Mouse.** Normal, button-motion and all-motion tracking; X10 (mode 9,
  presses only); SGR, UTF-8, urxvt (1015) and SGR pixel (1016) coordinates.
- **Reports.** XTGETTCAP answers truecolor (`Tc`, `RGB`), clipboard (`Ms`),
  styled underlines (`Smulx`, `Setulc`), cursor style (`Ss`, `Se`),
  synchronized output (`Sync`) and color count. DECRQSS reports SGR and the
  cursor style, XTVERSION reports Newport, and DA1 reports a VT220 with ANSI
  color. A second, query-only parser (`src/queries.rs`) answers these in order
  with the rest of the output.

Limits:

- A cell holds up to nine zero-width characters; further marks are discarded.
- REP repeats the last character received, not a whole cluster; DEL clears it.
- A selection is cleared when the width changes.
- Left and right margins (DECSLRM) are not supported.

Keep adapter regressions (fragmented UTF-8, styles, links, selection, search,
clipboard policy, reconnect, keyboard identity, and owned snapshots) when
updating the pinned crate. The packaged notices are in
`resources/licenses/terminal`.
