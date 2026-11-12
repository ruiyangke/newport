# Newport protocol reference

This is the developer reference for Newport's shared wire contracts and admission
sequences. It describes the protocol implemented by this source tree.

**Generated document.** Run `cargo xtask protocol-reference` from the Newport
repository root. `cargo xtask protocol-reference --check` compares the complete
output and exits unsuccessfully if it is stale, without rewriting documentation.
The command uses locked, cached Cargo dependencies offline. It needs neither a
system `protoc` nor agent binaries, a running Newport, Docker, or a desktop.

The generator is [examples/protocol_reference.rs](examples/protocol_reference.rs).
This introduction is maintained in
[reference-introduction.md](reference-introduction.md); the schema inventory is
generated from the same protox descriptor set used by
[build.rs](build.rs) and [codegen.rs](codegen.rs). Descriptor source information
supplies the `.proto` comments. Regeneration checks that every Newport schema
under `proto/` is represented. There are no timestamps or machine-specific paths
in the output.

## Ownership and layers

`newport-protocol` owns semantic Rust contracts, structural validation, schemas,
and optional protobuf encoding. Protobuf is the transport representation;
ordinary calls between Rust components can use the semantic values directly.
`newport-domain` independently owns application models and persisted-profile
rules. Neither crate depends on the other. Transport adapters own IO, OS peer
authentication, timeouts, cancellation and application policy. `newport-portal`
is an application-facing interface, not a tunnel for every internal protocol.

The public runtime and private Full product share these contracts. Full adds
Remote Apps and display behavior; it does not introduce a second wire crate.
Default crate features remain empty. `protobuf` enables schema compilation and
encoding, `tokio` adds asynchronous framing, and `trace` adds descriptor-driven
inspection. See the [crate overview](README.md) for the module boundaries.

## Wire framing, limits and evolution

Each Newport protobuf frame is an unsigned varint body length followed by one
protobuf message. Direction and connection phase determine the top-level message
type; the prefix does not carry a schema name. The decoder checks the declared
length before allocating the body. Protocol-specific readers also enforce
metadata, recursion, collection, text, direction and phase limits.

The generic metadata budget counts tags, lengths, scalar values and strings,
excluding only the contents of `bytes` fields, including nested fields. Generated
layouts bound traversal; `bytes::Bytes` permits payloads to borrow their frame.
See [encoding::protobuf](src/encoding/protobuf/mod.rs) and each boundary's `wire`
module for the actual limits and additional validation. Advertised limits never
permit a receiver to exceed its own limits.

New fields can be additive when their absence has a defined meaning. Never reuse
field numbers or change their wire meaning. Unknown fields are skipped where the
boundary permits them; an unknown required operation is not an authorization to
execute anything. The typed boundary rejects invalid known operations, duplicate
singular fields or ambiguous cases as specified by its scanner. Proto3 defaults
alone do not establish semantic validity or successful admission.

Every contract uses version 1 and one protobuf codec per boundary. A protocol
mismatch stops before credentials or mutations where the handshake permits
preparation. Transport loss after an operation was sent can mean an unknown
outcome; it is not permission to replay a mutation.

`newport.wire.Error` carries a code, human-readable message and explicit retry
decision. `Present` represents a parameterless case; `Secret` redacts its bytes
from Rust Debug and descriptor traces. Neither redaction nor protobuf framing
provides encryption or OS authentication. SSH encrypts and authenticates the
network transport. Local transport adapters enforce OS peer identity and endpoint
access checks; local IPC does not imply encryption.

For a synthetic or appropriately handled capture, run:

```sh
cargo xtask trace newport-protocol newport.runtime.ServerFrame capture.bin
```

Supply concatenated length-delimited protobuf frames, without outer transport
envelopes. The trace prints one JSON value per frame, replaces byte payloads with
lengths, and redacts `Secret`. Ordinary string fields may contain user data. Use
the directional message type of the captured connection phase.

## Runtime control

Schema: [runtime.proto](proto/newport/runtime.proto).
Contract: [runtime](src/runtime/mod.rs).

The Linux runtime speaks first with `ServerFrame.hello`: supported versions,
runtime instance, executable build hash, product, worker socket, service versions,
dependency availability and limits. The desktop validates these before sending
`ClientFrame.request.initialize` with the selected version, session and transport
generation. The runtime acknowledges it before ordinary control requests.

Every request has a request ID and the announced runtime ID; replies echo both.
Permissions, worker preparation and clipboard route updates apply to their exact
runtime and admitted worker. Product identity describes compiled support;
dependency availability and account permission are separate checks. A newer
runtime does not silently acquire authority over an older runtime still draining.

## Worker admission and lease channel

Schemas: [runtime.proto](proto/newport/runtime.proto) and
[clipboard.proto](proto/newport/clipboard.proto).
Contracts: [worker admission](src/worker/admission.rs) and
[worker](src/worker/mod.rs).

The desktop first requests `PrepareWorkerStream` on the initialized runtime.
Its response supplies a private endpoint and single-use ticket. The connection
to that endpoint sends `AdmissionClientFrame.open`, binding version, runtime,
worker, build, service/version and arguments. The runtime returns matching
`Started` identities or a typed refusal before service traffic begins.

The worker's separate registration-authenticated lease connection sends
`LeaseWorkerFrame.attach` and receives `LeaseRuntimeFrame.attached`. Its lease
may include runtime-scoped clipboard access. Permission updates and their
acknowledgments remain on this lease channel, independent of service stdin.
The consumers enforce socket ownership, admission deadlines and revocation.

## Worker relay and external streams

Schema: [worker.proto](proto/newport/worker.proto).
Contract: [worker wire](src/worker/wire.rs).

After admission, registration selects the service transport. Native protobuf
services retain their own directional frames through the relay. Field 127 is
reserved for worker controls: input completion and signals toward the worker;
stderr and exit status toward the desktop. Child service output may not forge
these controls. EOF without the required completion is an incomplete stream.

Explicit opaque registrations use `OpaqueClientFrame` and `OpaqueServerFrame`.
Their bounded byte chunks preserve external framing, including partial headers.
SSH, SFTP and external language-server payloads are not redefined as Newport
semantic protobuf records merely because a Newport stream carries them.

## Monitor and service operations

Schema: [monitor.proto](proto/newport/monitor.proto).
Contract: [monitor](src/monitor/mod.rs).

Runtime admission selects the monitor service and version; there is no second
service Hello. Numbered calls receive matching typed replies. Metrics, ports,
systemd, containers, Compose and watches use their operation-specific records
and limits. A direct service endpoint rejects relay-only controls. Watch
cancellation and service scheduling belong to the consumers.

## Git

Schema: [git.proto](proto/newport/git.proto).
Contract: [git](src/git/mod.rs).

Runtime admission selects Git. Configuration binds the client's durable journal
identity; requests and results remain operation-specific. The semantic validation
preserves receipt identities and durable journal hashes. Diff streams retain
explicit acknowledgment and completion, rather than treating EOF as success.

## Terminal lifecycle and retained holder

Schemas: [terminal.proto](proto/newport/terminal.proto) and
[terminal_holder.proto](proto/newport/terminal_holder.proto).
Contracts: [terminal](src/terminal/mod.rs) and
[holder](src/terminal/holder.rs).

The runtime-admitted terminal worker handles Start, List, Rename, End and Watch.
The resulting holder endpoint is a distinct long-lived connection. Its server
sends `Hello` containing version and terminal ID before the client sends `Attach`
with size, control/opened state, computer identity and colors. The reply confirms
attachment before screen snapshots, output, activity and exit events.

A holder that speaks a different protocol is never attached to or ended to make a
new connection succeed.

## Integration and clipboard export

Schemas: [integration.proto](proto/newport/integration.proto) and
[clipboard.proto](proto/newport/clipboard.proto).
Contracts: [integration](src/integration/mod.rs) and
[clipboard exports](src/clipboard/exports.rs).

Runtime admission selects integration. Correlated calls and responses carry
permission updates, host clipboard offers, browser replies and heartbeat
readiness. Unsolicited clipboard requests, browser requests, GUI notices and
export events share that stream. Export records are nested typed messages.

Export opt-in alone grants no clipboard write authority. A matching live display
route and focus/permission grant admit an offered native source; Commit produces
an exact copy lease before native publication and demand reads. Absence of an
offer, rejection of a copy, failed data and an empty completed value remain
distinct. Accepted-copy lifetime, focus changes and revocation are enforced by
the runtime and desktop router, not inferred from a successful decode.

## Runtime clipboard service

Schema: [clipboard_service.proto](proto/newport/clipboard_service.proto).
Contract: [clipboard service](src/clipboard/service/mod.rs).

The private Linux service sends `Hello(version, runtime)` first. A client checks
both before transmitting `Authenticate` with worker-scoped access. It then sends
one request at a time and receives one corresponding response. Authentication
does not itself assign the native provider or authorize host export.

Native publication uses a provider lease and compare-and-set against the observed
runtime revision. Routes bind the provider worker and export/focus state. Range
reads carry explicit offset and limit; copies carry their committed lease.
Authentication, reads and mutations use bounded exchanges. Do not reconnect and
replay a publication or copy operation after an uncertain exchange.

## Clipboard source socket

Schema: [clipboard_source.proto](proto/newport/clipboard_source.proto).
Contract: [clipboard source](src/clipboard/source.rs).

The local source sends Hello first. After verifying its version, the client sends
one Offer or revision-bound Read and closes its write side. The server requires
the complete request and EOF before serving it. It returns metadata, bounded
chunks ending with `eof`, or a typed failure; trailing output is invalid.
The managed runtime source delegates to the runtime clipboard broker. The
standalone snapshot-file fallback is local storage, not another stream codec.

## Browser helper

Schema: [browser.proto](proto/newport/browser.proto).
Contract: [browser](src/browser/mod.rs).

The local broker sends Hello first. The helper checks it before sending the URL,
then sends one Open and write EOF. Only a complete request can be forwarded to
the authorized computer. The response is Opened, optionally with a warning, or a
typed failure. Only accepted HTTP(S) URLs are allowed. A timeout after forwarding
can have an unknown outcome and must not cause automatic duplicate opening.

## Remote Apps

Schema: [remote/apps.proto](proto/newport/remote/apps.proto).
Contract: [remote_apps](src/remote_apps/mod.rs).

After runtime admission, the viewer sends Hello with versions, requested
features, performance/display settings and its license. The relay completes
display preparation before forwarding that service traffic; the viewer does not
perform the relay's Ready exchange itself. The display validates admission and
replies with Welcome and the granted feature tree, or Rejected. Requested support
is not a grant. Session, window, surface and sequence identities fence later
input, frames and acknowledgments.

Frame acknowledgment returns credit. Launch acknowledgment does not mean a
window was presented; lifecycle events report the later outcome. Takeover and
disconnection have explicit session semantics. Payloads such as images, audio,
file chunks and external drag representations remain byte data inside typed
messages.

## Display preparation and environment

Schema: [remote/display.proto](proto/newport/remote/display.proto).
Contracts: [attachment](src/display/attachment.rs) and
[environment](src/display/environment.rs).

The relay sends a credential-free Envelope with `prepare-clipboard-attach`,
revision, diagnostic build identity and empty body. It must receive Ready with
the display identity before sending typed Attach, including clipboard access,
on the same connection. The display prepares and authenticates that access,
then returns Accepted or Refused. An ordinary Accepted cannot replace Ready.

One admission deadline covers preparation and authentication. Remote Apps Hello
and license admission still precede viewer replacement; failed preparation must
preserve the current viewer and applications. No reconnect/replay is allowed.
Environment contracts identify and validate the published display endpoints;
they are semantic Rust values, not an additional protobuf channel.

## Desktop application catalogue

Schema: [remote/catalogue.proto](proto/newport/remote/catalogue.proto).
Contract: [remote_apps::apps](src/remote_apps/apps.rs).

Runtime admission selects catalogue discovery. It requires no further service
input and returns one Catalogue or typed failure. The reader bounds metadata
and icons, rejects duplicate application IDs and rejects trailing output.
Worker completion remains the relay's responsibility.

## Desktop IPC and opened streams

Schemas: [desktop.proto](proto/newport/desktop.proto),
[desktop/common.proto](proto/newport/desktop/common.proto),
[desktop/stream.proto](proto/newport/desktop/stream.proto) and the imported
operation-family schemas below. Contract: [desktop](src/desktop/mod.rs).

The authenticated local client sends Hello(version); the backend replies Ready
with its instance identity or Refused. Only then can a typed request run.
Requests, progress and responses use matching IDs. Account, configuration,
connection, file, Git, metric, service, container, preference, application, host,
event and terminal families have typed operations and replies; their imported
schemas do not add independent handshakes.

A successful stream-open response identifies the selected stream before the
connection switches from desktop request framing. Terminal leases use typed
frames with credit and epoch accounting. Command and LSP streams use bounded
opaque data and explicit completion. Remote Apps carries its directional service
frames after the open result. Stream failure cannot silently return to request
mode. Platform adapters enforce peer identity, connection/admission slots,
permissions and absolute deadlines.

## macOS clipboard helper

Schema: [clipboard_helper.proto](proto/newport/clipboard_helper.proto).
Contract: [clipboard helper](src/clipboard/helper.rs).

The owner sends Hello with version and the selected general or isolated test
pasteboard. The helper replies Ready before ordinary commands. Snapshots describe
logical revisions and optional imported origin; publication generations fence
delayed-rendering demands, replies, cancellation and file attachment. Payload
reads remain separate from metadata. The executable and pasteboard are chosen
by the platform owner; protobuf does not select arbitrary native resources.

## Windows clipboard provider

Schema: [clipboard_provider.proto](proto/newport/clipboard_provider.proto).
Contract: [clipboard provider](src/clipboard/provider.rs).

The parent opens the private named pipe and starts its owned helper on the same
window station/desktop. Parent Hello carries version and nonce; Ready must echo
both. Native clipboard work begins only after that exchange. Snapshot/read
results include native identity. Publication admission, completion, delayed
demands and source revocation remain distinct phases. A failed or partial native
write is not reported as a complete unchanged clipboard.

File attachment paths use exact little-endian UTF-16 code units, including
unpaired surrogates, without normalization. The provider's platform adapter
enforces process identity, pipe security, native ownership and deadlines.

## Application stub

Schema: [desktop/stub.proto](proto/newport/desktop/stub.proto).
Contract: [desktop stub](src/desktop/stub.rs).

The local stub sends Hello with version, launch reason and bundle identity. The
provider replies Welcome before ordinary application/window traffic; an Alert
can explain refusal, followed by Quit.
The state machine rejects duplicate welcomes and operations in the wrong phase.
Window generations fence input and geometry updates. The macOS Mach transport
retains its platform envelopes and authentication while using this shared typed
payload; there is no additional MessagePack payload codec.
