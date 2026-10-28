# Newport platform adapters

This crate owns OS-specific primitives shared by the UI and the backend. It
has no GPUI dependency.

`ipc::{Stream, Listener}` provides blocking byte streams for client worker
threads and local contract fixtures. macOS retains standard Unix sockets.
Windows uses Tokio's overlapped named pipes behind a blocking worker interface:

- Names are derived from the canonical endpoint directory, filename, and user.
- A protected logon-session DACL and rejection of remote clients restrict access.
- The client checks the server's OS-reported process user and logon SID before
  sending data, using identification-only security quality of service.
- First-instance ownership prevents replacing an active listener. Each accept
  creates the next listening instance before handing off the connected one.
- Read/write deadlines, nonblocking reads, clone cancellation, and peer EOF
  support the existing consent and terminal-lease contracts. Half-close is
  unsupported on Windows; current callers use full shutdown only.
- A busy pipe waits up to ten seconds for an instance **before** any request is
  sent. Request bytes and uncertain operations are never replayed.

The implementation follows [Microsoft's named-pipe security guidance](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)
and [Tokio's pipe options](https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/struct.ServerOptions.html).
Call blocking APIs only on worker threads, outside an async executor.

`filesystem` creates new private fixture directories and file options. Windows
directories use the persistent user SID; pipe access uses the transient logon
SID. These policies intentionally have different lifetimes.

`secrets::SystemStorage` supplies protected macOS Keychain and Windows
Credential Manager access to the shared backend. It preserves the device-local
key policy, secure random generation and explicit access errors. The backend
supplies stable namespaces/account hashes and owns key verification and
recovery markers. The adapters do
not select profiles, replace missing keys or fall back to plaintext storage.

`clipboard` supplies native format offers and revision-checked, on-demand reads.
Call it through the application's main-thread dispatcher. The backend retains
the synchronization protocol, deadlines and cancellation; the OS adapter never
pushes clipboard data on its own. The macOS test uses a private named pasteboard,
preserving the user's general clipboard. Windows HTML offset parsing is shared
and tested independently of the OS clipboard APIs.

`events::install` registers process-lifetime sleep/wake and network observers
on the application's main thread. macOS uses AppKit and SystemConfiguration;
Windows uses native power and IP-interface notifications. Only recovery hints
cross the callback boundary; the backend owns backoff and connection state.
The standalone GPUI application owns registration of this adapter.
Unsupported platforms report an error instead of pretending to observe events.

On macOS, `cargo run --locked -p newport-platform --example system_events_check`
checks real observer registration and delivery of process-local synthetic
sleep/wake notifications. It rejects installation from a worker thread and does
not sleep the computer or change network settings. Physical sleep/network and
Windows callback acceptance remain separate checks.

Run `cargo test --locked -p newport-platform -p newport-client` on each native
architecture. Tests cover reply preservation at EOF, deadlines, concurrent
connections, duplicate listeners, and cancellation under backpressure. Windows
also attempts an anonymous connection against the actual pipe's DACL.

`ipc::asynchronous` supplies the shared backend listener on both platforms.
The standalone application composes these adapters on macOS and Windows.
Windows ARM64 shared/platform runtime checks have passed; native x64 hardware,
full desktop workflows and signed release acceptance remain separate gates.
