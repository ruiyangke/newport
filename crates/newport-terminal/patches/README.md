# Ghostty compatibility patches

Base: Ghostty `3425025e585a3403340d4dc6d65132f42e05e605` (MIT).
These are temporary source fixes, not a separate terminal parser. Keep them
small enough to review upstream and remove each when the pinned release
contains an equivalent correction.

`cargo xtask env` applies the numbered patches to an archive
of the pinned Git tree. The original checkout stays unchanged. The patch hashes
form part of the output directory name, so changing a patch also invalidates
the Rust binding's native build through `GHOSTTY_SOURCE_DIR`. Nested CI and
packaging wrappers reuse the same prepared source. Integration tests verify
actual patch application, cache invalidation, and rejection of modified files.

- **001 — Resize output:** keep styled blank rows on history-enabled screens;
  preserve the current and saved cursor's column when their row is clamped at
  the top. Alternate screens retain upstream trimming behavior. Includes
  upstream Zig tests, plus Newport's height/reflow regression tests.
- **002 — Clipped graphemes:** when a wide character cannot fit at a no-wrap
  margin, discard its combining suffix instead of attaching it to an unrelated
  neighboring cell. Uses Ghostty's existing grapheme state machine. Includes
  Zig and byte-fragmented Rust regressions.
- **003 — DEL control:** ignore U+007F in scalar, batched, and vectorized input
  paths. It must neither occupy a cell nor replace the previous character used
  by REP. Includes parser tests and chunk/batch-boundary Rust regressions.
- **004 — Clipped resize continuations:** when a non-reflowing resize removes
  the previous grapheme, discard its remaining suffix rather than modifying a
  surviving neighbor. A surviving base still accepts its combining marks.
  Includes upstream and Rust regressions for both cases.
- **005 — Prepend graphemes:** apply GB9b to ASCII after a Prepend prefix and
  derive the cluster's width from its base. Ordinary ASCII still uses bulk
  printing. Includes scalar/batched, right-margin, one-column, cursor movement,
  style, and fragmented-input regressions.

The contract tests in `tests/` are an acceptance gate. Modern Unicode widths,
complete-paragraph reflow, and Ghostty's bounded combining storage are
intentional policies; they do not excuse unrelated data corruption. Passing
these focused patch tests alone is not desktop acceptance.
