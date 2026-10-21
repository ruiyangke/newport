# Newport native UI

This library renders Newport with GPUI. The [application host](../newport-app/README.md)
owns process startup, the backend, tray, OS callbacks, and update installation.
It opens the UI through `run_with_options_and_ready`.

## Responsibilities

- Page composition, navigation, dialogs, and keyboard/focus behavior.
- Query subscriptions and presentation state; remote work runs outside rendering.
- Bounded table/list rendering, terminal drawing, and text/image/PDF previews.
- Theme, icons, and lazy syntax highlighting using per-language grammar libraries.

Shared feature rules belong in [newport-domain](../newport-domain/README.md).
Cached server data and refresh coordination belong in the
[backend query layer](../newport-backend/src/query/README.md). OS operations belong
in [newport-platform](../newport-platform/README.md).

## Shared connectivity

`connectivity::Connectivity` is the application-wide GPUI global, keyed by server
ID. The application host subscribes once to `Application::connections()` and
pushes updates into this store. SSH connection state and approval changes do not
wait for page polling. Pages and retained components use `connectivity::observe`
and read the store; they never infer server connectivity from terminal events,
feature request errors, or cached health probes.

The backend registry owns the canonical store. Its current server session
publishes transport transitions, fenced by an owner token. The UI rejects older
revisions and retired backend instances. Components also check the connection
scope before refreshing a retained read after recovery. Server deletion removes
the entry, and replacement immediately invalidates the old connection state.

Service readiness remains distinct: an installed agent, an open shell, and
acknowledged sharing permissions describe their own resources. They cannot
declare the server connected. Enabled sharing follows the shared transport
through explicit authentication retries; it does not cache another SSH error or
initiate repeated consent requests. Recovery may refresh reads, but never replays
a service action, Git mutation, or terminal command.

## Run and verify

Run from the repository root with an isolated development profile:

```sh
cargo xtask dev
cargo xtask env -- cargo test --locked -p newport-gpui --lib
```

The wrapper supplies the pinned Ghostty build environment. On Windows, stage
PDFium with `cargo xtask pdfium` before preview checks.

For rendering benchmarks:

```sh
cargo xtask grammars --release
cargo xtask env -- cargo test --locked --release -p newport-gpui --lib benchmark_ -- --ignored --nocapture --test-threads=1
```

The `input_focus_probe` and `terminal_render_probe` examples are manual native
input/rendering diagnostics. Run them through the same wrapper with
`cargo run --locked -p newport-gpui --example NAME`. Optional offline asset and
terminal-fixture generators are documented in [scripts/README.md](scripts/README.md).

Unit tests and benchmarks do not replace native interaction checks. See the
[testing guide](../../docs/testing.md) for lifecycle and release acceptance.
