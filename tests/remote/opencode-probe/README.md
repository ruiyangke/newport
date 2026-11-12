# OpenCode clipboard compatibility probe

This fixture imports unmodified clipboard adapters from released OpenCode versions:

| File | Release | Source commit |
| --- | --- | --- |
| `v1.ts` | v1.19.4 | `c0c7bae726798d02592f8d3305891ba2f0cd880a` |
| `v2.ts` | v2.1.3 | `bb25a2f4a3f3f04d7c344bec0c0574ea3510b3b1` |

Both are `packages/tui/src/clipboard.ts` from
<https://github.com/anomalyco/opencode>; their MIT license is in `LICENSE-opencode`.
Do not change the vendored adapters to accommodate Needle.

Dependencies match the releases: clipboardy 4.0.0 for v1; OpenTUI 0.6.6 for v2,
running on Bun 1.4.4. The v2 native backend is real OpenTUI, not a mock.
The type-only context import in v2 is erased by Bun. The v1 copy-only `which`
dependency is unnecessary for these read tests.

`main.ts` accepts `v1` or `v2`, then newline-delimited `read` and `quit` commands.
The SSH suite tests text, PNG bytes, changes, and empty clipboards using one
adapter instance per sequence. It isolates X11 and Wayland for v2 and the two
image command paths for v1. These exercise the released clipboard integration,
not the full interactive TUI or an LLM session.
