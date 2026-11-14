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

## Generated schema inventory

The following 35 schemas come from `encoding::protobuf::DESCRIPTORS`, compiled
by `build.rs` through `codegen.rs` with protox source information enabled.
Names, field numbers, types, presence, reserved message fields and comments are
read from that descriptor set; imported Google schemas and synthetic map-entry
messages are omitted. Each schema links to its authoritative source.

A field marked implicit uses protobuf's scalar default when absent. Message
presence and explicit `optional` preserve absence; a `oneof` permits one selected
case. Semantic Rust validators can impose stronger requirements. Empty comment
cells mean that the schema supplies no field comment.

## Schema `newport/browser.proto`

Source: [newport/browser.proto](proto/newport/browser.proto). Package: `newport.browser`.

Imports: `newport/wire.proto`.

### `newport.browser.Hello`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | implicit |  |

### `newport.browser.Open`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `url` | `string` | implicit |  |

### `newport.browser.Opened`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `warning` | `string` | optional |  |

### `newport.browser.ClientFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `open` | `newport.browser.Open` | message presence |  |

### `newport.browser.ServerFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `hello` | `newport.browser.Hello` | oneof `message` |  |
| 2 | `opened` | `newport.browser.Opened` | oneof `message` |  |
| 3 | `error` | `newport.wire.Error` | oneof `message` |  |

## Schema `newport/clipboard.proto`

Source: [newport/clipboard.proto](proto/newport/clipboard.proto). Package: `newport.clipboard`.

Imports: `newport/wire.proto`.

### `newport.clipboard.Access`

Worker admission credentials. Secret is omitted from Debug and wire traces.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `path` | `string` | implicit |  |
| 2 | `runtime` | `string` | implicit |  |
| 3 | `worker` | `string` | implicit |  |
| 4 | `token` | `newport.wire.Secret` | message presence |  |

### `newport.clipboard.NativeOrigin`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `worker` | `string` | implicit |  |
| 2 | `revision` | `uint64` | implicit |  |

### `newport.clipboard.ExportOffer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `runtime` | `string` | implicit |  |
| 2 | `revision` | `int64` | implicit |  |
| 3 | `origin` | `newport.clipboard.NativeOrigin` | message presence |  |
| 4 | `formats` | `string` | repeated |  |
| 5 | `route_revision` | `uint64` | implicit |  |

### `newport.clipboard.CopyLease`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |
| 2 | `offer` | `newport.clipboard.ExportOffer` | message presence |  |

### `newport.clipboard.Chunk`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `data` | `bytes` | implicit |  |
| 2 | `eof` | `bool` | implicit |  |

### `newport.clipboard.Enable`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | implicit |  |

### `newport.clipboard.Read`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `offer` | `newport.clipboard.ExportOffer` | message presence |  |
| 3 | `format` | `string` | implicit |  |

### `newport.clipboard.Cancel`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |

### `newport.clipboard.Commit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `offer` | `newport.clipboard.ExportOffer` | message presence |  |

### `newport.clipboard.Release`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `copy` | `newport.clipboard.CopyLease` | message presence |  |

### `newport.clipboard.ReadCopy`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `copy` | `newport.clipboard.CopyLease` | message presence |  |
| 3 | `format` | `string` | implicit |  |

### `newport.clipboard.ExportControl`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `enable` | `newport.clipboard.Enable` | oneof `command` |  |
| 2 | `disable` | `newport.wire.Present` | oneof `command` |  |
| 3 | `read` | `newport.clipboard.Read` | oneof `command` |  |
| 4 | `cancel` | `newport.clipboard.Cancel` | oneof `command` |  |
| 5 | `commit` | `newport.clipboard.Commit` | oneof `command` |  |
| 6 | `release` | `newport.clipboard.Release` | oneof `command` |  |
| 7 | `read_copy` | `newport.clipboard.ReadCopy` | oneof `command` |  |

### `newport.clipboard.Offered`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `offer` | `newport.clipboard.ExportOffer` | message presence |  |

### `newport.clipboard.Data`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `offset` | `uint64` | implicit |  |
| 3 | `chunk` | `newport.clipboard.Chunk` | message presence |  |

### `newport.clipboard.Committed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `copy` | `newport.clipboard.CopyLease` | message presence |  |

### `newport.clipboard.CopyRevoked`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |

### `newport.clipboard.ExportEvent`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `offered` | `newport.clipboard.Offered` | oneof `event` |  |
| 2 | `data` | `newport.clipboard.Data` | oneof `event` |  |
| 3 | `committed` | `newport.clipboard.Committed` | oneof `event` |  |
| 4 | `copy_revoked` | `newport.clipboard.CopyRevoked` | oneof `event` |  |

## Schema `newport/clipboard_helper.proto`

Source: [newport/clipboard_helper.proto](proto/newport/clipboard_helper.proto). Package: `newport.clipboard.helper`.

Imports: `newport/wire.proto`.

### `newport.clipboard.helper.Board`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `general` | `newport.wire.Present` | oneof `kind` |  |
| 2 | `test` | `string` | oneof `kind` |  |

### `newport.clipboard.helper.Hello`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | implicit |  |
| 2 | `board` | `newport.clipboard.helper.Board` | message presence |  |

### `newport.clipboard.helper.Publish`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `generation` | `uint64` | implicit |  |
| 2 | `expected_revision` | `int64` | implicit |  |
| 3 | `formats` | `string` | repeated |  |

### `newport.clipboard.helper.AttachFiles`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `generation` | `uint64` | implicit |  |
| 2 | `expected_revision` | `int64` | implicit |  |
| 3 | `folder` | `string` | implicit |  |

### `newport.clipboard.helper.Generation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `generation` | `uint64` | implicit |  |

### `newport.clipboard.helper.Data`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `generation` | `uint64` | implicit |  |
| 2 | `id` | `uint64` | implicit |  |
| 3 | `offset` | `uint64` | implicit |  |
| 4 | `eof` | `bool` | implicit |  |
| 5 | `data` | `bytes` | implicit |  |

### `newport.clipboard.helper.Demand`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `generation` | `uint64` | implicit |  |
| 2 | `id` | `uint64` | implicit |  |

### `newport.clipboard.helper.ReadLocal`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `revision` | `int64` | implicit |  |
| 3 | `format` | `string` | implicit |  |

### `newport.clipboard.helper.Id`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |

### `newport.clipboard.helper.Ready`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | implicit |  |

### `newport.clipboard.helper.Snapshot`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `int64` | implicit |  |
| 2 | `formats` | `string` | repeated |  |
| 3 | `origin` | `uint64` | optional |  |

### `newport.clipboard.helper.Published`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `generation` | `uint64` | implicit |  |
| 2 | `revision` | `int64` | implicit |  |

### `newport.clipboard.helper.Revision`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `generation` | `uint64` | implicit |  |
| 2 | `revision` | `int64` | optional |  |

### `newport.clipboard.helper.Read`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `generation` | `uint64` | implicit |  |
| 2 | `id` | `uint64` | implicit |  |
| 3 | `format` | `string` | implicit |  |

### `newport.clipboard.helper.LocalData`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `revision` | `int64` | implicit |  |
| 3 | `offset` | `uint64` | implicit |  |
| 4 | `eof` | `bool` | implicit |  |
| 5 | `data` | `bytes` | implicit |  |

### `newport.clipboard.helper.CommandFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `hello` | `newport.clipboard.helper.Hello` | oneof `command` |  |
| 2 | `publish` | `newport.clipboard.helper.Publish` | oneof `command` |  |
| 3 | `attach_files` | `newport.clipboard.helper.AttachFiles` | oneof `command` |  |
| 4 | `revoke` | `newport.clipboard.helper.Generation` | oneof `command` |  |
| 5 | `data` | `newport.clipboard.helper.Data` | oneof `command` |  |
| 6 | `failed` | `newport.clipboard.helper.Demand` | oneof `command` |  |
| 7 | `read_local` | `newport.clipboard.helper.ReadLocal` | oneof `command` |  |
| 8 | `cancel_local` | `newport.clipboard.helper.Id` | oneof `command` |  |
| 9 | `stop` | `newport.wire.Present` | oneof `command` |  |

### `newport.clipboard.helper.EventFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ready` | `newport.clipboard.helper.Ready` | oneof `event` |  |
| 2 | `snapshot` | `newport.clipboard.helper.Snapshot` | oneof `event` |  |
| 3 | `published` | `newport.clipboard.helper.Published` | oneof `event` |  |
| 4 | `attached` | `newport.clipboard.helper.Revision` | oneof `event` |  |
| 5 | `rejected` | `newport.clipboard.helper.Generation` | oneof `event` |  |
| 6 | `revoked` | `newport.clipboard.helper.Revision` | oneof `event` |  |
| 7 | `read` | `newport.clipboard.helper.Read` | oneof `event` |  |
| 8 | `cancelled` | `newport.clipboard.helper.Demand` | oneof `event` |  |
| 9 | `local_data` | `newport.clipboard.helper.LocalData` | oneof `event` |  |
| 10 | `local_failed` | `newport.clipboard.helper.Id` | oneof `event` |  |

## Schema `newport/clipboard_provider.proto`

Source: [newport/clipboard_provider.proto](proto/newport/clipboard_provider.proto). Package: `newport.clipboard.provider`.

Imports: `newport/wire.proto`.

### `newport.clipboard.provider.Greeting`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | implicit |  |
| 2 | `nonce` | `newport.wire.Secret` | message presence |  |

### `newport.clipboard.provider.Identity`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `sequence` | `uint32` | implicit |  |
| 2 | `owner` | `uint64` | implicit |  |

### `newport.clipboard.provider.PublicationRejection`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `stage` | `newport.clipboard.provider.PublicationStage` | implicit |  |
| 2 | `expected` | `int64` | implicit |  |
| 3 | `observed` | `int64` | optional |  |
| 4 | `native_error` | `uint32` | optional |  |

### `newport.clipboard.provider.Preparation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `int64` | implicit |  |
| 2 | `identity` | `newport.clipboard.provider.Identity` | message presence |  |

### `newport.clipboard.provider.Committed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `int64` | implicit |  |
| 2 | `complete` | `bool` | implicit |  |

### `newport.clipboard.provider.FileWrite`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `unchanged` | `newport.wire.Present` | oneof `outcome` |  |
| 2 | `committed` | `newport.clipboard.provider.Committed` | oneof `outcome` |  |
| 3 | `uncertain` | `newport.wire.Present` | oneof `outcome` |  |

### `newport.clipboard.provider.Snapshot`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `int64` | implicit |  |
| 2 | `identity` | `newport.clipboard.provider.Identity` | message presence |  |
| 3 | `formats` | `string` | repeated |  |
| 4 | `origin` | `uint64` | optional |  |

### `newport.clipboard.provider.ReadResult`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `int64` | implicit |  |
| 2 | `identity` | `newport.clipboard.provider.Identity` | message presence |  |
| 3 | `data` | `bytes` | implicit |  |

### `newport.clipboard.provider.Published`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `generation` | `uint64` | implicit |  |
| 2 | `owner` | `uint64` | implicit |  |
| 3 | `preparation` | `newport.clipboard.provider.Preparation` | message presence |  |

### `newport.clipboard.provider.Outcome`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `publication_rejected` | `newport.clipboard.provider.PublicationRejection` | oneof `result` |  |
| 2 | `snapshot` | `newport.clipboard.provider.Snapshot` | oneof `result` |  |
| 3 | `read` | `newport.clipboard.provider.ReadResult` | oneof `result` |  |
| 4 | `published` | `newport.clipboard.provider.Published` | oneof `result` |  |
| 5 | `attached` | `newport.clipboard.provider.FileWrite` | oneof `result` |  |

### `newport.clipboard.provider.Id`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |

### `newport.clipboard.provider.Read`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `revision` | `int64` | implicit |  |
| 3 | `format` | `string` | implicit |  |

### `newport.clipboard.provider.Publish`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `expected` | `int64` | implicit |  |
| 3 | `formats` | `string` | repeated |  |

### `newport.clipboard.provider.Attach`

Exact little-endian UTF-16 code units, including unpaired surrogates. No normalization.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `generation` | `uint64` | implicit |  |
| 3 | `folder_utf16le` | `bytes` | implicit |  |

### `newport.clipboard.provider.Done`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `outcome` | `newport.clipboard.provider.Outcome` | message presence |  |

### `newport.clipboard.provider.Admit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `source` | `uint64` | implicit |  |
| 3 | `preparation` | `bool` | implicit |  |

### `newport.clipboard.provider.Admitted`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `accepted` | `bool` | implicit |  |

### `newport.clipboard.provider.Demand`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `source` | `uint64` | implicit |  |
| 3 | `format` | `string` | implicit |  |

### `newport.clipboard.provider.Data`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `present` | `bool` | implicit |  |
| 3 | `data` | `bytes` | implicit |  |

### `newport.clipboard.provider.Source`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `source` | `uint64` | implicit |  |

### `newport.clipboard.provider.Frame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `hello` | `newport.clipboard.provider.Greeting` | oneof `message` |  |
| 2 | `ready` | `newport.clipboard.provider.Greeting` | oneof `message` |  |
| 3 | `snapshot` | `newport.clipboard.provider.Id` | oneof `message` |  |
| 4 | `read` | `newport.clipboard.provider.Read` | oneof `message` |  |
| 5 | `publish` | `newport.clipboard.provider.Publish` | oneof `message` |  |
| 6 | `attach` | `newport.clipboard.provider.Attach` | oneof `message` |  |
| 7 | `done` | `newport.clipboard.provider.Done` | oneof `message` |  |
| 8 | `admit` | `newport.clipboard.provider.Admit` | oneof `message` |  |
| 9 | `admitted` | `newport.clipboard.provider.Admitted` | oneof `message` |  |
| 10 | `demand` | `newport.clipboard.provider.Demand` | oneof `message` |  |
| 11 | `data` | `newport.clipboard.provider.Data` | oneof `message` |  |
| 12 | `consumed` | `newport.clipboard.provider.Id` | oneof `message` |  |
| 13 | `revoke` | `newport.clipboard.provider.Source` | oneof `message` |  |
| 14 | `revoked` | `newport.clipboard.provider.Source` | oneof `message` |  |
| 15 | `stop` | `newport.wire.Present` | oneof `message` |  |

### `newport.clipboard.provider.PublicationStage`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `SOURCE` |  |
| 1 | `QUEUE` |  |
| 2 | `OBSERVE` |  |
| 3 | `BASELINE` |  |
| 4 | `FORMATS` |  |
| 5 | `GENERATION` |  |
| 6 | `OPEN` |  |
| 7 | `VALIDATE` |  |
| 8 | `ADMISSION` |  |
| 9 | `EMPTY` |  |
| 10 | `DELAYED_FORMATS` |  |
| 11 | `CONCEALMENT` |  |
| 12 | `REPLY` |  |

## Schema `newport/clipboard_service.proto`

Source: [newport/clipboard_service.proto](proto/newport/clipboard_service.proto). Package: `newport.clipboard.service`.

Imports: `newport/wire.proto`, `newport/clipboard.proto`, `newport/integration.proto`.

### `newport.clipboard.service.Hello`

The server speaks first. A client must verify version and runtime before
 sending Authenticate. All protocol/capability versions remain 1.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | implicit |  |
| 2 | `runtime` | `string` | implicit |  |

### `newport.clipboard.service.Offer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `runtime` | `string` | implicit |  |
| 2 | `revision` | `int64` | implicit |  |
| 3 | `empty` | `newport.wire.Present` | oneof `origin` |  |
| 4 | `host` | `newport.wire.Present` | oneof `origin` |  |
| 5 | `native` | `newport.clipboard.NativeOrigin` | oneof `origin` |  |
| 6 | `formats` | `string` | repeated |  |

### `newport.clipboard.service.ProviderLease`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `runtime` | `string` | implicit |  |
| 2 | `worker` | `string` | implicit |  |
| 3 | `epoch` | `uint64` | implicit |  |

### `newport.clipboard.service.NativeRequest`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `lease` | `newport.clipboard.service.ProviderLease` | message presence |  |
| 2 | `id` | `uint64` | implicit |  |
| 3 | `revision` | `uint64` | implicit |  |
| 4 | `format` | `string` | implicit |  |

### `newport.clipboard.service.ProviderPoll`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `lease` | `newport.clipboard.service.ProviderLease` | message presence |  |
| 2 | `offer` | `newport.clipboard.service.Offer` | message presence |  |
| 3 | `requests` | `newport.clipboard.service.NativeRequest` | repeated |  |

### `newport.clipboard.service.Read`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `runtime` | `string` | implicit |  |
| 2 | `revision` | `int64` | implicit |  |
| 3 | `format` | `string` | implicit |  |
| 4 | `offset` | `uint64` | implicit |  |
| 5 | `limit` | `uint32` | implicit |  |
| 6 | `focused` | `newport.clipboard.ExportOffer` | oneof `export` |  |
| 7 | `committed` | `newport.clipboard.CopyLease` | oneof `export` |  |

### `newport.clipboard.service.Enabled`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `value` | `bool` | implicit |  |

### `newport.clipboard.service.NativePublish`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `lease` | `newport.clipboard.service.ProviderLease` | message presence |  |
| 2 | `expected_runtime` | `string` | implicit |  |
| 3 | `expected_revision` | `int64` | implicit |  |
| 4 | `revision` | `uint64` | implicit |  |
| 5 | `formats` | `string` | repeated |  |

### `newport.clipboard.service.NativeReply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `request` | `newport.clipboard.service.NativeRequest` | message presence |  |
| 2 | `chunk` | `newport.clipboard.Chunk` | message presence |  |

### `newport.clipboard.service.Requests`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `requests` | `newport.integration.ClipboardRequest` | repeated |  |

### `newport.clipboard.service.Published`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `accepted` | `bool` | implicit |  |

### `newport.clipboard.service.Export`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `offer` | `newport.clipboard.ExportOffer` | message presence |  |

### `newport.clipboard.service.Copy`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `copy` | `newport.clipboard.CopyLease` | message presence |  |

### `newport.clipboard.service.ClientFrame`

Exactly one request and reply at a time. Authenticate must be first and
 cannot be repeated. Range reads, publications and copy mutations are never
 replayed after an uncertain transport failure.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `authenticate` | `newport.clipboard.Access` | oneof `request` |  |
| 2 | `offer` | `newport.wire.Present` | oneof `request` |  |
| 3 | `read` | `newport.clipboard.service.Read` | oneof `request` |  |
| 4 | `claim` | `newport.wire.Present` | oneof `request` |  |
| 5 | `enabled` | `newport.clipboard.service.Enabled` | oneof `request` |  |
| 6 | `publish` | `newport.integration.ClipboardOffer` | oneof `request` |  |
| 7 | `poll` | `newport.wire.Present` | oneof `request` |  |
| 8 | `reply` | `newport.integration.ClipboardReply` | oneof `request` |  |
| 9 | `native_poll` | `newport.wire.Present` | oneof `request` |  |
| 10 | `native_publish` | `newport.clipboard.service.NativePublish` | oneof `request` |  |
| 11 | `native_reply` | `newport.clipboard.service.NativeReply` | oneof `request` |  |
| 12 | `export_offer` | `newport.wire.Present` | oneof `request` |  |
| 13 | `commit_copy` | `newport.clipboard.ExportOffer` | oneof `request` |  |
| 14 | `copy_status` | `newport.wire.Present` | oneof `request` |  |
| 15 | `release_copy` | `newport.clipboard.CopyLease` | oneof `request` |  |

### `newport.clipboard.service.ServerFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `hello` | `newport.clipboard.service.Hello` | oneof `response` |  |
| 2 | `ready` | `newport.wire.Present` | oneof `response` |  |
| 3 | `offer` | `newport.clipboard.service.Offer` | oneof `response` |  |
| 4 | `chunk` | `newport.clipboard.Chunk` | oneof `response` |  |
| 5 | `requests` | `newport.clipboard.service.Requests` | oneof `response` |  |
| 6 | `provider` | `newport.clipboard.service.ProviderPoll` | oneof `response` |  |
| 7 | `published` | `newport.clipboard.service.Published` | oneof `response` |  |
| 8 | `export` | `newport.clipboard.service.Export` | oneof `response` |  |
| 9 | `copy` | `newport.clipboard.service.Copy` | oneof `response` |  |
| 10 | `error` | `newport.wire.Error` | oneof `response` |  |

## Schema `newport/clipboard_source.proto`

Source: [newport/clipboard_source.proto](proto/newport/clipboard_source.proto). Package: `newport.clipboard.source`.

Imports: `newport/wire.proto`, `newport/clipboard.proto`, `newport/integration.proto`.

### `newport.clipboard.source.Hello`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | implicit |  |

### `newport.clipboard.source.Read`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `int64` | implicit |  |
| 2 | `format` | `string` | implicit |  |

### `newport.clipboard.source.ClientFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `offer` | `newport.wire.Present` | oneof `request` |  |
| 2 | `read` | `newport.clipboard.source.Read` | oneof `request` |  |

### `newport.clipboard.source.ServerFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `hello` | `newport.clipboard.source.Hello` | oneof `response` |  |
| 2 | `offer` | `newport.integration.ClipboardOffer` | oneof `response` |  |
| 3 | `chunk` | `newport.clipboard.Chunk` | oneof `response` |  |
| 4 | `error` | `newport.wire.Error` | oneof `response` |  |

## Schema `newport/desktop.proto`

Source: [newport/desktop.proto](proto/newport/desktop.proto). Package: `newport.desktop`.

Imports: `newport/desktop/account.proto`, `newport/desktop/applications.proto`, `newport/desktop/common.proto`, `newport/desktop/configuration.proto`, `newport/desktop/connection.proto`, `newport/desktop/containers.proto`, `newport/desktop/events.proto`, `newport/desktop/files.proto`, `newport/desktop/git.proto`, `newport/desktop/host.proto`, `newport/desktop/metrics.proto`, `newport/desktop/preferences.proto`, `newport/desktop/services.proto`, `newport/desktop/stream.proto`, `newport/desktop/terminal.proto`, `newport/wire.proto`.

### `newport.desktop.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 16 | `account` | `newport.desktop.account.Operation` | oneof `value` |  |
| 17 | `connection` | `newport.desktop.connection.Operation` | oneof `value` |  |
| 18 | `configuration` | `newport.desktop.configuration.Operation` | oneof `value` |  |
| 19 | `files` | `newport.desktop.files.Operation` | oneof `value` |  |
| 20 | `metrics` | `newport.desktop.metrics.Operation` | oneof `value` |  |
| 21 | `services` | `newport.desktop.services.Operation` | oneof `value` |  |
| 22 | `containers` | `newport.desktop.containers.Operation` | oneof `value` |  |
| 23 | `preferences` | `newport.desktop.preferences.Operation` | oneof `value` |  |
| 24 | `git` | `newport.desktop.git.Operation` | oneof `value` |  |
| 25 | `applications` | `newport.desktop.applications.Operation` | oneof `value` |  |
| 26 | `host` | `newport.desktop.host.Operation` | oneof `value` |  |
| 27 | `events` | `newport.desktop.events.Operation` | oneof `value` |  |
| 28 | `terminal` | `newport.desktop.terminal.Operation` | oneof `value` |  |
| 29 | `stream` | `newport.desktop.stream.Operation` | oneof `value` |  |

### `newport.desktop.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 16 | `account` | `newport.desktop.account.Reply` | oneof `value` |  |
| 17 | `connection` | `newport.desktop.connection.Reply` | oneof `value` |  |
| 18 | `configuration` | `newport.desktop.configuration.Reply` | oneof `value` |  |
| 19 | `files` | `newport.desktop.files.Reply` | oneof `value` |  |
| 20 | `metrics` | `newport.desktop.metrics.Reply` | oneof `value` |  |
| 21 | `services` | `newport.desktop.services.Reply` | oneof `value` |  |
| 22 | `containers` | `newport.desktop.containers.Reply` | oneof `value` |  |
| 23 | `preferences` | `newport.desktop.preferences.Reply` | oneof `value` |  |
| 24 | `git` | `newport.desktop.git.Reply` | oneof `value` |  |
| 25 | `applications` | `newport.desktop.applications.Reply` | oneof `value` |  |
| 26 | `host` | `newport.desktop.host.Reply` | oneof `value` |  |
| 27 | `events` | `newport.desktop.events.Reply` | oneof `value` |  |
| 28 | `terminal` | `newport.desktop.terminal.Reply` | oneof `value` |  |

Reserved: 29.

### `newport.desktop.Request`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | optional |  |
| 16 | `account` | `newport.desktop.account.Operation` | oneof `operation` |  |
| 17 | `connection` | `newport.desktop.connection.Operation` | oneof `operation` |  |
| 18 | `configuration` | `newport.desktop.configuration.Operation` | oneof `operation` |  |
| 19 | `files` | `newport.desktop.files.Operation` | oneof `operation` |  |
| 20 | `metrics` | `newport.desktop.metrics.Operation` | oneof `operation` |  |
| 21 | `services` | `newport.desktop.services.Operation` | oneof `operation` |  |
| 22 | `containers` | `newport.desktop.containers.Operation` | oneof `operation` |  |
| 23 | `preferences` | `newport.desktop.preferences.Operation` | oneof `operation` |  |
| 24 | `git` | `newport.desktop.git.Operation` | oneof `operation` |  |
| 25 | `applications` | `newport.desktop.applications.Operation` | oneof `operation` |  |
| 26 | `host` | `newport.desktop.host.Operation` | oneof `operation` |  |
| 27 | `events` | `newport.desktop.events.Operation` | oneof `operation` |  |
| 28 | `terminal` | `newport.desktop.terminal.Operation` | oneof `operation` |  |
| 29 | `stream` | `newport.desktop.stream.Operation` | oneof `operation` |  |

### `newport.desktop.Outcome`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 2 | `completed` | `newport.desktop.Reply` | oneof `value` |  |
| 3 | `failed` | `newport.desktop.common.Failure` | oneof `value` |  |
| 4 | `opened` | `newport.desktop.stream.Opened` | oneof `value` |  |

### `newport.desktop.Response`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | optional |  |
| 2 | `completed` | `newport.desktop.Reply` | oneof `outcome` |  |
| 3 | `failed` | `newport.desktop.common.Failure` | oneof `outcome` |  |
| 4 | `opened` | `newport.desktop.stream.Opened` | oneof `outcome` |  |

### `newport.desktop.ClientMessage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `hello` | `newport.desktop.common.Hello` | oneof `value` |  |
| 2 | `request` | `newport.desktop.Request` | oneof `value` |  |

### `newport.desktop.ServerMessage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ready` | `newport.desktop.common.Ready` | oneof `value` |  |
| 2 | `refused` | `newport.wire.Error` | oneof `value` |  |
| 3 | `progress` | `newport.desktop.common.Progress` | oneof `value` |  |
| 4 | `response` | `newport.desktop.Response` | oneof `value` |  |

## Schema `newport/desktop/account.proto`

Source: [newport/desktop/account.proto](proto/newport/desktop/account.proto). Package: `newport.desktop.account`.

Imports: `newport/wire.proto`.

### `newport.desktop.account.Notice`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | optional |  |
| 2 | `kind` | `newport.desktop.account.NoticeKind` | optional |  |
| 3 | `message` | `string` | optional |  |
| 4 | `action` | `newport.desktop.account.NoticeAction` | optional |  |

### `newport.desktop.account.License`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `state` | `string` | optional |  |
| 2 | `plan` | `string` | optional |  |
| 3 | `seat` | `string` | optional |  |
| 4 | `valid_until` | `string` | optional |  |
| 5 | `renews_at` | `string` | optional |  |
| 6 | `ends_at` | `string` | optional |  |
| 7 | `trial_used` | `bool` | optional |  |
| 8 | `message` | `string` | optional |  |
| 9 | `trial_used_here` | `bool` | optional |  |

### `newport.desktop.account.Status`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `state` | `newport.desktop.account.Phase` | optional |  |
| 2 | `email` | `string` | optional |  |
| 3 | `name` | `string` | optional |  |
| 4 | `license` | `newport.desktop.account.License` | message presence |  |
| 5 | `signing_out` | `bool` | optional |  |
| 6 | `notice` | `newport.desktop.account.Notice` | message presence |  |
| 7 | `license_revision` | `uint64` | optional |  |

### `newport.desktop.account.RemoteLicense`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `token` | `newport.wire.Secret` | message presence |  |
| 2 | `machine` | `string` | optional |  |
| 3 | `trusted_time` | `int64` | optional |  |
| 4 | `expires_at` | `int64` | optional |  |

### `newport.desktop.account.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `status` | `newport.wire.Present` | oneof `value` |  |
| 2 | `sign_in` | `newport.wire.Present` | oneof `value` |  |
| 3 | `cancel_sign_in` | `newport.wire.Present` | oneof `value` |  |
| 4 | `sign_out` | `newport.wire.Present` | oneof `value` |  |
| 5 | `open_portal` | `newport.wire.Present` | oneof `value` |  |
| 6 | `open_billing` | `newport.wire.Present` | oneof `value` |  |
| 7 | `refresh` | `newport.wire.Present` | oneof `value` |  |
| 8 | `start_trial` | `newport.wire.Present` | oneof `value` |  |
| 9 | `remote_license` | `newport.wire.Present` | oneof `value` |  |

### `newport.desktop.account.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `status` | `newport.desktop.account.Status` | oneof `value` |  |
| 2 | `sign_in` | `newport.desktop.account.Status` | oneof `value` |  |
| 3 | `cancel_sign_in` | `newport.desktop.account.Status` | oneof `value` |  |
| 4 | `sign_out` | `newport.desktop.account.Status` | oneof `value` |  |
| 5 | `open_portal` | `newport.wire.Present` | oneof `value` |  |
| 6 | `open_billing` | `newport.wire.Present` | oneof `value` |  |
| 7 | `refresh` | `newport.desktop.account.Status` | oneof `value` |  |
| 8 | `start_trial` | `newport.desktop.account.Status` | oneof `value` |  |
| 9 | `remote_license` | `newport.desktop.account.RemoteLicense` | oneof `value` |  |

### `newport.desktop.account.Phase`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `PHASE_UNSPECIFIED` |  |
| 1 | `PHASE_LOADING` |  |
| 2 | `PHASE_SIGNED_OUT` |  |
| 3 | `PHASE_WAITING` |  |
| 4 | `PHASE_SIGNED_IN` |  |

### `newport.desktop.account.NoticeKind`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `NOTICE_KIND_UNSPECIFIED` |  |
| 1 | `NOTICE_KIND_SUCCESS` |  |
| 2 | `NOTICE_KIND_INFO` |  |
| 3 | `NOTICE_KIND_WARNING` |  |
| 4 | `NOTICE_KIND_ERROR` |  |

### `newport.desktop.account.NoticeAction`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `NOTICE_ACTION_UNSPECIFIED` |  |
| 1 | `NOTICE_ACTION_MANAGE_COMPUTERS` |  |
| 2 | `NOTICE_ACTION_TRY_AGAIN` |  |

### `newport.desktop.account.RefusalReason`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `REFUSAL_REASON_UNSPECIFIED` |  |
| 1 | `REFUSAL_REASON_SIGNED_OUT` |  |
| 2 | `REFUSAL_REASON_TRIAL_ELSEWHERE` |  |
| 3 | `REFUSAL_REASON_FREE_SEAT` |  |
| 4 | `REFUSAL_REASON_FREE` |  |
| 5 | `REFUSAL_REASON_UNAVAILABLE` |  |
| 6 | `REFUSAL_REASON_CLOCK` |  |
| 7 | `REFUSAL_REASON_OFFLINE` |  |

## Schema `newport/desktop/applications.proto`

Source: [newport/desktop/applications.proto](proto/newport/desktop/applications.proto). Package: `newport.desktop.applications`.

Imports: `newport/desktop/configuration.proto`, `newport/remote/catalogue.proto`, `newport/runtime.proto`, `newport/wire.proto`.

### `newport.desktop.applications.Runtime`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `instance` | `string` | optional |  |
| 2 | `build_sha256` | `string` | optional |  |
| 3 | `transport_generation` | `uint64` | optional |  |

### `newport.desktop.applications.Identity`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `runtime` | `newport.desktop.applications.Runtime` | message presence |  |
| 2 | `session` | `string` | optional |  |
| 3 | `worker` | `string` | optional |  |
| 4 | `display_generation` | `uint64` | optional |  |

### `newport.desktop.applications.Command`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `argv` | `string` | repeated |  |

### `newport.desktop.applications.Desktop`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `uris` | `string` | repeated |  |
| 3 | `action` | `string` | optional |  |

### `newport.desktop.applications.Target`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `command` | `newport.desktop.applications.Command` | oneof `value` |  |
| 2 | `desktop` | `newport.desktop.applications.Desktop` | oneof `value` |  |

### `newport.desktop.applications.Outcome`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `accepted` | `newport.wire.Present` | oneof `value` |  |
| 2 | `rejected` | `string` | oneof `value` |  |
| 3 | `unknown` | `string` | oneof `value` |  |

### `newport.desktop.applications.Application`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `display` | `newport.desktop.applications.Identity` | message presence |  |
| 2 | `request` | `uint64` | optional |  |

### `newport.desktop.applications.Launched`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `application` | `newport.desktop.applications.Application` | message presence |  |
| 2 | `outcome` | `newport.desktop.applications.Outcome` | message presence |  |

### `newport.desktop.applications.OptionalDetail`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `detail` | `string` | optional |  |

### `newport.desktop.applications.Ending`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `failed` | `string` | oneof `value` |  |
| 2 | `no_window` | `newport.desktop.applications.OptionalDetail` | oneof `value` |  |
| 3 | `handed_off` | `newport.wire.Present` | oneof `value` |  |
| 4 | `display_stopped` | `newport.wire.Present` | oneof `value` |  |
| 5 | `shown_elsewhere` | `newport.desktop.applications.OptionalDetail` | oneof `value` |  |

### `newport.desktop.applications.Observation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `application` | `newport.desktop.applications.Application` | message presence |  |
| 2 | `state` | `newport.desktop.applications.State` | optional |  |
| 3 | `error` | `string` | optional |  |
| 4 | `ending` | `newport.desktop.applications.Ending` | message presence |  |

### `newport.desktop.applications.Observations`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `observations` | `newport.desktop.applications.Observation` | repeated |  |

### `newport.desktop.applications.Shown`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `app_id` | `string` | optional |  |
| 2 | `windows` | `uint32` | optional |  |

### `newport.desktop.applications.ShownOn`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `computer` | `string` | optional |  |
| 2 | `session` | `string` | optional |  |

### `newport.desktop.applications.Display`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `shown_on` | `newport.desktop.applications.ShownOn` | message presence |  |
| 2 | `applications` | `newport.desktop.applications.Shown` | repeated |  |

### `newport.desktop.applications.Stopped`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `stopped` | `bool` | optional |  |

### `newport.desktop.applications.Finding`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `missing` | `newport.wire.Present` | oneof `value` |  |
| 2 | `outdated` | `string` | oneof `value` |  |
| 3 | `inactive` | `newport.desktop.applications.Inactive` | oneof `value` |  |
| 4 | `unverified` | `newport.wire.Present` | oneof `value` |  |

### `newport.desktop.applications.Gap`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `requirement` | `newport.desktop.applications.Requirement` | optional |  |
| 2 | `finding` | `newport.desktop.applications.Finding` | message presence |  |

### `newport.desktop.applications.Host`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `elevation` | `newport.desktop.applications.Elevation` | optional |  |
| 2 | `updater` | `newport.desktop.applications.Updater` | optional |  |
| 3 | `user_services` | `bool` | optional |  |

### `newport.desktop.applications.LegacyAgent`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `string` | optional |  |
| 2 | `agent` | `string` | optional |  |
| 3 | `built_at` | `uint64` | optional |  |

### `newport.desktop.applications.SelectedRuntime`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `build_sha256` | `string` | optional |  |
| 2 | `product` | `newport.runtime.Product` | optional |  |

### `newport.desktop.applications.Availability`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `agent` | `string` | optional |  |
| 2 | `legacy_agent` | `string` | optional |  |
| 3 | `legacy_revision` | `string` | optional |  |
| 4 | `legacy_agents` | `newport.desktop.applications.LegacyAgent` | repeated |  |
| 5 | `system` | `string` | optional |  |
| 6 | `architecture` | `string` | optional |  |
| 7 | `gaps` | `newport.desktop.applications.Gap` | repeated |  |
| 8 | `host` | `newport.desktop.applications.Host` | message presence |  |
| 9 | `os_release` | `string` | optional |  |
| 10 | `expected_server` | `newport.desktop.configuration.Server` | message presence |  |
| 11 | `bundled_variant` | `newport.runtime.Product` | optional |  |
| 12 | `bundled_revision` | `string` | optional |  |
| 13 | `total` | `uint64` | optional |  |
| 14 | `installed_variant` | `newport.runtime.Product` | optional |  |
| 15 | `revision` | `string` | optional |  |
| 16 | `build_relation` | `newport.desktop.configuration.BuildRelation` | optional |  |
| 17 | `installation_error` | `string` | optional |  |
| 18 | `selected_runtime` | `newport.desktop.applications.SelectedRuntime` | message presence |  |
| 19 | `agent_error` | `string` | optional |  |
| 20 | `automatic_update` | `newport.desktop.configuration.UpdateOutcome` | message presence |  |

### `newport.desktop.applications.StatusArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `refresh` | `bool` | optional | Absence uses the source API default. |

### `newport.desktop.applications.CatalogueArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `refresh` | `bool` | optional | Absence uses the source API default. |

### `newport.desktop.applications.DisplayArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |

### `newport.desktop.applications.LaunchArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |
| 3 | `target` | `newport.desktop.applications.Target` | message presence |  |

### `newport.desktop.applications.ShowArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |

### `newport.desktop.applications.StatesArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |
| 3 | `applications` | `newport.desktop.applications.Application` | repeated |  |

### `newport.desktop.applications.StopArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |
| 3 | `application` | `newport.desktop.applications.Application` | message presence |  |

### `newport.desktop.applications.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `status` | `newport.desktop.applications.StatusArgs` | oneof `value` |  |
| 2 | `catalogue` | `newport.desktop.applications.CatalogueArgs` | oneof `value` |  |
| 3 | `display` | `newport.desktop.applications.DisplayArgs` | oneof `value` |  |
| 4 | `launch` | `newport.desktop.applications.LaunchArgs` | oneof `value` |  |
| 5 | `show` | `newport.desktop.applications.ShowArgs` | oneof `value` |  |
| 6 | `states` | `newport.desktop.applications.StatesArgs` | oneof `value` |  |
| 7 | `stop` | `newport.desktop.applications.StopArgs` | oneof `value` |  |

### `newport.desktop.applications.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `status` | `newport.desktop.applications.Availability` | oneof `value` |  |
| 2 | `catalogue` | `newport.remote.catalogue.Catalogue` | oneof `value` |  |
| 3 | `display` | `newport.desktop.applications.Display` | oneof `value` |  |
| 4 | `launch` | `newport.desktop.applications.Launched` | oneof `value` |  |
| 5 | `show` | `newport.wire.Present` | oneof `value` |  |
| 6 | `states` | `newport.desktop.applications.Observations` | oneof `value` |  |
| 7 | `stop` | `newport.desktop.applications.Stopped` | oneof `value` |  |

### `newport.desktop.applications.State`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `STATE_UNSPECIFIED` |  |
| 1 | `STATE_PENDING` |  |
| 2 | `STATE_OPENING` |  |
| 3 | `STATE_BACKGROUND` |  |
| 4 | `STATE_RUNNING` |  |
| 5 | `STATE_STOPPING` |  |
| 6 | `STATE_ENDED` |  |

### `newport.desktop.applications.Requirement`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `REQUIREMENT_UNSPECIFIED` |  |
| 1 | `REQUIREMENT_DBUS` |  |
| 2 | `REQUIREMENT_XWAYLAND` |  |
| 3 | `REQUIREMENT_FONTS` |  |
| 4 | `REQUIREMENT_GLES` |  |
| 5 | `REQUIREMENT_PORTAL` |  |
| 6 | `REQUIREMENT_AUDIO` |  |

### `newport.desktop.applications.Inactive`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `INACTIVE_UNSPECIFIED` |  |
| 1 | `INACTIVE_ROOT` |  |
| 2 | `INACTIVE_STOPPED` |  |
| 3 | `INACTIVE_FAILED` |  |

### `newport.desktop.applications.Elevation`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `ELEVATION_UNSPECIFIED` |  |
| 1 | `ELEVATION_ROOT` |  |
| 2 | `ELEVATION_SUDO` |  |
| 3 | `ELEVATION_DOAS` |  |
| 4 | `ELEVATION_UNAVAILABLE` |  |

### `newport.desktop.applications.Updater`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `UPDATER_UNSPECIFIED` |  |
| 1 | `UPDATER_RPM_OSTREE` |  |
| 2 | `UPDATER_TRANSACTIONAL_UPDATE` |  |

## Schema `newport/desktop/common.proto`

Source: [newport/desktop/common.proto](proto/newport/desktop/common.proto). Package: `newport.desktop.common`.

Imports: `newport/desktop/account.proto`, `newport/wire.proto`.

### `newport.desktop.common.Hello`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | optional |  |
| 2 | `contract_id` | `string` | optional |  |

### `newport.desktop.common.Ready`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | optional |  |
| 2 | `backend_instance` | `string` | optional |  |
| 3 | `contract_id` | `string` | optional |  |

### `newport.desktop.common.FailureDetail`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `license_refusal` | `newport.desktop.account.RefusalReason` | oneof `value` |  |

### `newport.desktop.common.Failure`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `error` | `newport.wire.Error` | message presence |  |
| 2 | `detail` | `newport.desktop.common.FailureDetail` | message presence |  |

### `newport.desktop.common.AgentApproval`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `awaiting` | `bool` | optional |  |

### `newport.desktop.common.ProgressState`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 2 | `agent_approval` | `newport.desktop.common.AgentApproval` | oneof `value` |  |

### `newport.desktop.common.Progress`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | optional |  |
| 2 | `agent_approval` | `newport.desktop.common.AgentApproval` | oneof `state` |  |

## Schema `newport/desktop/configuration.proto`

Source: [newport/desktop/configuration.proto](proto/newport/desktop/configuration.proto). Package: `newport.desktop.configuration`.

Imports: `newport/runtime.proto`, `newport/wire.proto`.

### `newport.desktop.configuration.Proxy`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `command` | `string` | oneof `value` |  |
| 2 | `jump` | `string` | oneof `value` |  |

### `newport.desktop.configuration.Server`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `name` | `string` | optional |  |
| 3 | `ssh_user` | `string` | optional |  |
| 4 | `ssh_host` | `string` | optional |  |
| 5 | `ssh_port` | `uint32` | optional |  |
| 6 | `identity_file` | `string` | optional |  |
| 7 | `agent_source` | `string` | optional |  |
| 8 | `agent_key_fingerprint` | `string` | optional |  |
| 9 | `auth_method` | `newport.desktop.configuration.AuthMethod` | optional |  |
| 10 | `clipboard_enabled` | `bool` | optional |  |
| 11 | `browser_enabled` | `bool` | optional |  |
| 12 | `agent_install_approved` | `bool` | optional |  |
| 13 | `setup_complete` | `bool` | optional |  |
| 14 | `proxy` | `newport.desktop.configuration.Proxy` | message presence |  |

### `newport.desktop.configuration.Tunnel`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `name` | `string` | optional |  |
| 3 | `server_id` | `string` | optional |  |
| 4 | `local_port` | `uint32` | optional |  |
| 5 | `local_port_end` | `uint32` | optional |  |
| 6 | `remote_host` | `string` | optional |  |
| 7 | `remote_port` | `uint32` | optional |  |
| 8 | `remote_port_end` | `uint32` | optional |  |
| 9 | `auto_connect` | `bool` | optional |  |
| 10 | `auto_reconnect` | `bool` | optional |  |
| 11 | `direction` | `newport.desktop.configuration.Direction` | optional |  |
| 12 | `project` | `string` | optional |  |

### `newport.desktop.configuration.Config`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `servers` | `newport.desktop.configuration.Server` | repeated |  |
| 2 | `tunnels` | `newport.desktop.configuration.Tunnel` | repeated |  |

### `newport.desktop.configuration.ConnectionSettings`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server_id` | `string` | optional |  |
| 2 | `ssh_host` | `string` | optional |  |
| 3 | `ssh_user` | `string` | optional |  |
| 4 | `ssh_port` | `uint32` | optional |  |
| 5 | `auth_method` | `newport.desktop.configuration.AuthMethod` | optional |  |
| 6 | `identity_file` | `string` | optional |  |
| 7 | `agent_source` | `string` | optional |  |
| 8 | `agent_key_fingerprint` | `string` | optional |  |
| 9 | `proxy` | `newport.desktop.configuration.Proxy` | message presence |  |

### `newport.desktop.configuration.ConnectionScope`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `settings` | `newport.desktop.configuration.ConnectionSettings` | message presence |  |
| 2 | `connection_revision` | `uint64` | optional |  |
| 3 | `backend_instance` | `string` | optional |  |

### `newport.desktop.configuration.Transport`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `status` | `newport.desktop.configuration.TransportStatus` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `waiting` | `uint64` | optional |  |

### `newport.desktop.configuration.RuntimeInstance`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `instance_id` | `string` | optional |  |
| 2 | `build_sha256` | `string` | optional |  |
| 3 | `generation` | `uint64` | optional |  |
| 4 | `retiring` | `bool` | optional |  |
| 5 | `draining` | `bool` | optional |  |
| 6 | `connected` | `bool` | optional |  |
| 7 | `registrations` | `uint64` | optional |  |
| 8 | `worker_leases` | `uint64` | optional |  |

### `newport.desktop.configuration.RuntimeSnapshot`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `status` | `newport.desktop.configuration.RuntimeStatus` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `hello` | `newport.runtime.Hello` | message presence |  |
| 4 | `error` | `string` | optional |  |
| 5 | `retryable` | `bool` | optional |  |
| 6 | `failures` | `uint32` | optional |  |
| 7 | `instances` | `newport.desktop.configuration.RuntimeInstance` | repeated |  |

### `newport.desktop.configuration.PermissionsSnapshot`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `desired` | `newport.runtime.Permissions` | message presence |  |
| 2 | `effective` | `newport.runtime.Permissions` | message presence |  |
| 3 | `error` | `string` | optional |  |
| 4 | `clipboard_epoch` | `uint64` | optional |  |
| 5 | `browser_epoch` | `uint64` | optional |  |
| 6 | `session_epoch` | `uint64` | optional |  |

### `newport.desktop.configuration.UpdateOutcome`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `at` | `uint64` | optional |  |
| 2 | `from` | `string` | optional |  |
| 3 | `to` | `string` | optional |  |
| 4 | `error` | `string` | optional |  |

### `newport.desktop.configuration.InstallationStatus`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `installed` | `bool` | optional |  |
| 2 | `update_available` | `bool` | optional |  |
| 3 | `build_relation` | `newport.desktop.configuration.BuildRelation` | optional |  |
| 4 | `installed_hash` | `string` | optional |  |
| 5 | `bundled_hash` | `string` | optional |  |
| 6 | `installed_version` | `string` | optional |  |
| 7 | `bundled_version` | `string` | optional |  |
| 8 | `installed_variant` | `newport.runtime.Product` | optional |  |
| 9 | `bundled_variant` | `newport.runtime.Product` | optional |  |
| 10 | `installed_running` | `bool` | optional |  |
| 11 | `bundled_staged` | `bool` | optional |  |
| 12 | `checked_at` | `uint64` | optional |  |
| 13 | `error` | `string` | optional |  |
| 14 | `automatic_update` | `newport.desktop.configuration.UpdateOutcome` | message presence |  |

### `newport.desktop.configuration.InstallProgress`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `stage` | `newport.desktop.configuration.InstallStage` | optional |  |
| 2 | `sent` | `uint64` | optional |  |
| 3 | `total` | `uint64` | optional |  |
| 4 | `error` | `string` | optional |  |
| 5 | `failed_stage` | `newport.desktop.configuration.InstallStage` | optional |  |
| 6 | `bytes_per_second` | `double` | optional |  |

### `newport.desktop.configuration.ConnectionState`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `status` | `newport.desktop.configuration.ConnectionStatus` | optional |  |
| 2 | `error_message` | `string` | optional |  |
| 3 | `reconnect_attempt` | `uint32` | optional |  |

### `newport.desktop.configuration.DestinationHealth`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `local_port` | `uint32` | optional |  |
| 2 | `remote_port` | `uint32` | optional |  |
| 3 | `status` | `newport.desktop.configuration.DestinationStatus` | optional |  |
| 4 | `message` | `string` | optional |  |
| 5 | `checked_at` | `uint64` | optional |  |

### `newport.desktop.configuration.DestinationHealthList`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `values` | `newport.desktop.configuration.DestinationHealth` | repeated |  |

### `newport.desktop.configuration.TransportRow`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `value` | `newport.desktop.configuration.Transport` | message presence |  |

### `newport.desktop.configuration.RuntimeRow`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `value` | `newport.desktop.configuration.RuntimeSnapshot` | message presence |  |

### `newport.desktop.configuration.PermissionsRow`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `value` | `newport.desktop.configuration.PermissionsSnapshot` | message presence |  |

### `newport.desktop.configuration.InstallRow`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `value` | `newport.desktop.configuration.InstallProgress` | message presence |  |

### `newport.desktop.configuration.InstallationRow`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `value` | `newport.desktop.configuration.InstallationStatus` | message presence |  |

### `newport.desktop.configuration.ConnectionRow`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `value` | `newport.desktop.configuration.ConnectionState` | message presence |  |

### `newport.desktop.configuration.HealthRow`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `value` | `newport.desktop.configuration.DestinationHealthList` | message presence |  |

### `newport.desktop.configuration.MessageRow`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `value` | `string` | optional |  |

### `newport.desktop.configuration.RevisionRow`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `value` | `uint64` | optional |  |

### `newport.desktop.configuration.Runtime`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `agent_transport` | `newport.desktop.configuration.TransportRow` | repeated |  |
| 2 | `agent_runtime` | `newport.desktop.configuration.RuntimeRow` | repeated |  |
| 3 | `agent_permissions` | `newport.desktop.configuration.PermissionsRow` | repeated |  |
| 4 | `agent_status` | `newport.desktop.configuration.InstallationRow` | repeated |  |
| 5 | `agent_installation` | `newport.desktop.configuration.InstallRow` | repeated |  |
| 6 | `tunnels` | `newport.desktop.configuration.ConnectionRow` | repeated |  |
| 7 | `tunnel_health` | `newport.desktop.configuration.HealthRow` | repeated |  |
| 8 | `clipboard` | `newport.desktop.configuration.ConnectionRow` | repeated |  |
| 9 | `clipboard_messages` | `newport.desktop.configuration.MessageRow` | repeated |  |
| 10 | `connection_revisions` | `newport.desktop.configuration.RevisionRow` | repeated |  |
| 11 | `awaiting_agent` | `string` | repeated |  |

### `newport.desktop.configuration.Snapshot`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `instance_id` | `string` | optional |  |
| 2 | `revision` | `uint64` | optional |  |
| 3 | `config` | `newport.desktop.configuration.Config` | message presence |  |
| 4 | `runtime` | `newport.desktop.configuration.Runtime` | message presence |  |
| 5 | `load_error` | `string` | optional |  |

### `newport.desktop.configuration.Tunnels`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `tunnels` | `newport.desktop.configuration.Tunnel` | repeated |  |

### `newport.desktop.configuration.SaveServerArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `newport.desktop.configuration.Server` | message presence |  |
| 2 | `password` | `newport.wire.Secret` | message presence |  |

### `newport.desktop.configuration.DeleteServerArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |

### `newport.desktop.configuration.ForwardDiscoveredPortArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `address` | `string` | optional |  |
| 3 | `port` | `uint32` | optional |  |

### `newport.desktop.configuration.ForwardTunnelBatchArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `create` | `newport.desktop.configuration.Tunnel` | repeated |  |
| 3 | `connect` | `newport.desktop.configuration.Tunnel` | repeated |  |
| 4 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |

### `newport.desktop.configuration.SaveTunnelArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `tunnel` | `newport.desktop.configuration.Tunnel` | message presence |  |

### `newport.desktop.configuration.UpdateTunnelArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `tunnel` | `newport.desktop.configuration.Tunnel` | message presence |  |
| 2 | `expected` | `newport.desktop.configuration.Tunnel` | message presence |  |

### `newport.desktop.configuration.DeleteTunnelArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `tunnel` | `string` | optional |  |

### `newport.desktop.configuration.DeleteTunnelsArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `expected` | `newport.desktop.configuration.Tunnel` | repeated |  |

### `newport.desktop.configuration.SetTunnelConnectedArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `tunnel` | `string` | optional |  |
| 2 | `connected` | `bool` | optional |  |

### `newport.desktop.configuration.SetClipboardEnabledArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `enabled` | `bool` | optional |  |
| 3 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |

### `newport.desktop.configuration.SetIntegrationEnabledArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `feature` | `newport.desktop.configuration.Feature` | optional |  |
| 3 | `enabled` | `bool` | optional |  |
| 4 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |

### `newport.desktop.configuration.OpenTunnelArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `tunnel` | `string` | optional |  |
| 2 | `remote_port` | `uint32` | optional |  |
| 3 | `expected` | `newport.desktop.configuration.Tunnel` | message presence |  |
| 4 | `expected_connection` | `newport.desktop.configuration.ConnectionScope` | message presence |  |

### `newport.desktop.configuration.OpenTerminalLinkArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `terminal` | `string` | optional |  |
| 2 | `url` | `string` | optional |  |

### `newport.desktop.configuration.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `save_server` | `newport.desktop.configuration.SaveServerArgs` | oneof `value` |  |
| 2 | `delete_server` | `newport.desktop.configuration.DeleteServerArgs` | oneof `value` |  |
| 3 | `forward_discovered_port` | `newport.desktop.configuration.ForwardDiscoveredPortArgs` | oneof `value` |  |
| 4 | `forward_tunnel_batch` | `newport.desktop.configuration.ForwardTunnelBatchArgs` | oneof `value` |  |
| 5 | `save_tunnel` | `newport.desktop.configuration.SaveTunnelArgs` | oneof `value` |  |
| 6 | `update_tunnel` | `newport.desktop.configuration.UpdateTunnelArgs` | oneof `value` |  |
| 7 | `delete_tunnel` | `newport.desktop.configuration.DeleteTunnelArgs` | oneof `value` |  |
| 8 | `delete_tunnels` | `newport.desktop.configuration.DeleteTunnelsArgs` | oneof `value` |  |
| 9 | `set_tunnel_connected` | `newport.desktop.configuration.SetTunnelConnectedArgs` | oneof `value` |  |
| 10 | `set_clipboard_enabled` | `newport.desktop.configuration.SetClipboardEnabledArgs` | oneof `value` |  |
| 11 | `set_integration_enabled` | `newport.desktop.configuration.SetIntegrationEnabledArgs` | oneof `value` |  |
| 12 | `open_tunnel` | `newport.desktop.configuration.OpenTunnelArgs` | oneof `value` |  |
| 13 | `open_terminal_link` | `newport.desktop.configuration.OpenTerminalLinkArgs` | oneof `value` |  |
| 14 | `snapshot` | `newport.wire.Present` | oneof `value` |  |

### `newport.desktop.configuration.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `save_server` | `newport.wire.Present` | oneof `value` |  |
| 2 | `delete_server` | `newport.wire.Present` | oneof `value` |  |
| 3 | `forward_discovered_port` | `newport.wire.Present` | oneof `value` |  |
| 4 | `forward_tunnel_batch` | `newport.desktop.configuration.Tunnels` | oneof `value` |  |
| 5 | `save_tunnel` | `newport.wire.Present` | oneof `value` |  |
| 6 | `update_tunnel` | `newport.wire.Present` | oneof `value` |  |
| 7 | `delete_tunnel` | `newport.wire.Present` | oneof `value` |  |
| 8 | `delete_tunnels` | `newport.wire.Present` | oneof `value` |  |
| 9 | `set_tunnel_connected` | `newport.wire.Present` | oneof `value` |  |
| 10 | `set_clipboard_enabled` | `newport.wire.Present` | oneof `value` |  |
| 11 | `set_integration_enabled` | `newport.wire.Present` | oneof `value` |  |
| 12 | `open_tunnel` | `newport.wire.Present` | oneof `value` |  |
| 13 | `open_terminal_link` | `newport.wire.Present` | oneof `value` |  |
| 14 | `snapshot` | `newport.desktop.configuration.Snapshot` | oneof `value` |  |

### `newport.desktop.configuration.AuthMethod`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `AUTH_METHOD_UNSPECIFIED` |  |
| 1 | `AUTH_METHOD_PUBLIC_KEY` |  |
| 2 | `AUTH_METHOD_PASSWORD` |  |

### `newport.desktop.configuration.Direction`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `DIRECTION_UNSPECIFIED` |  |
| 1 | `DIRECTION_LOCAL` |  |
| 2 | `DIRECTION_REMOTE` |  |

### `newport.desktop.configuration.Feature`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `FEATURE_UNSPECIFIED` |  |
| 1 | `FEATURE_CLIPBOARD` |  |
| 2 | `FEATURE_BROWSER` |  |

### `newport.desktop.configuration.TransportStatus`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `TRANSPORT_STATUS_UNSPECIFIED` |  |
| 1 | `TRANSPORT_STATUS_DISCONNECTED` |  |
| 2 | `TRANSPORT_STATUS_CONNECTING` |  |
| 3 | `TRANSPORT_STATUS_CONNECTED` |  |
| 4 | `TRANSPORT_STATUS_RECOVERING` |  |
| 5 | `TRANSPORT_STATUS_FAILED` |  |

### `newport.desktop.configuration.RuntimeStatus`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `RUNTIME_STATUS_UNSPECIFIED` |  |
| 1 | `RUNTIME_STATUS_UNKNOWN` |  |
| 2 | `RUNTIME_STATUS_STARTING` |  |
| 3 | `RUNTIME_STATUS_READY` |  |
| 4 | `RUNTIME_STATUS_FAILED` |  |

### `newport.desktop.configuration.BuildRelation`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `BUILD_RELATION_UNSPECIFIED` |  |
| 1 | `BUILD_RELATION_UNKNOWN` |  |
| 2 | `BUILD_RELATION_MISSING` |  |
| 3 | `BUILD_RELATION_BUNDLED` |  |
| 4 | `BUILD_RELATION_DIFFERENT` |  |
| 5 | `BUILD_RELATION_OLDER` |  |
| 6 | `BUILD_RELATION_NEWER` |  |
| 7 | `BUILD_RELATION_SAME_RELEASE` |  |

### `newport.desktop.configuration.InstallStage`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `INSTALL_STAGE_UNSPECIFIED` |  |
| 1 | `INSTALL_STAGE_PREPARING` |  |
| 2 | `INSTALL_STAGE_UPLOADING` |  |
| 3 | `INSTALL_STAGE_INSTALLING` |  |
| 4 | `INSTALL_STAGE_VERIFYING` |  |
| 5 | `INSTALL_STAGE_ACTIVATING` |  |
| 6 | `INSTALL_STAGE_READY` |  |
| 7 | `INSTALL_STAGE_ERROR` |  |

### `newport.desktop.configuration.ConnectionStatus`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CONNECTION_STATUS_UNSPECIFIED` |  |
| 1 | `CONNECTION_STATUS_DISCONNECTED` |  |
| 2 | `CONNECTION_STATUS_CONNECTING` |  |
| 3 | `CONNECTION_STATUS_CONNECTED` |  |
| 4 | `CONNECTION_STATUS_RECONNECTING` |  |
| 5 | `CONNECTION_STATUS_ERROR` |  |

### `newport.desktop.configuration.DestinationStatus`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `DESTINATION_STATUS_UNSPECIFIED` |  |
| 1 | `DESTINATION_STATUS_CHECKING` |  |
| 2 | `DESTINATION_STATUS_REACHABLE` |  |
| 3 | `DESTINATION_STATUS_UNAVAILABLE` |  |
| 4 | `DESTINATION_STATUS_BLOCKED` |  |
| 5 | `DESTINATION_STATUS_UNKNOWN` |  |

## Schema `newport/desktop/connection.proto`

Source: [newport/desktop/connection.proto](proto/newport/desktop/connection.proto). Package: `newport.desktop.connection`.

Imports: `newport/desktop/configuration.proto`, `newport/wire.proto`, `newport/discovery.proto`.

### `newport.desktop.connection.Discovery`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `current` | `newport.discovery.Snapshot` | message presence |  |
| 2 | `retained` | `newport.discovery.Snapshot` | repeated |  |
| 3 | `retention` | `newport.discovery.Report` | message presence |  |

### `newport.desktop.connection.SetupInspection`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `os` | `string` | optional |  |
| 2 | `architecture` | `string` | optional |  |
| 3 | `cores` | `uint64` | optional |  |
| 4 | `memory_kib` | `uint64` | optional |  |
| 5 | `free_kib` | `uint64` | optional |  |
| 6 | `installed` | `bool` | optional |  |
| 7 | `total` | `uint64` | optional |  |

### `newport.desktop.connection.OptionalNotice`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `notice` | `string` | optional |  |

### `newport.desktop.connection.AgentKey`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `source` | `string` | optional |  |
| 2 | `comment` | `string` | optional |  |
| 3 | `fingerprint` | `string` | optional |  |
| 4 | `algorithm` | `string` | optional |  |

### `newport.desktop.connection.AgentKeyList`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `keys` | `newport.desktop.connection.AgentKey` | repeated |  |
| 2 | `warnings` | `string` | repeated |  |

### `newport.desktop.connection.ConfigHost`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `alias` | `string` | optional |  |
| 3 | `host` | `string` | optional |  |
| 4 | `user` | `string` | optional |  |
| 5 | `port` | `uint32` | optional |  |
| 6 | `identity_file` | `string` | optional |  |
| 7 | `proxy` | `newport.desktop.configuration.Proxy` | message presence |  |
| 8 | `through` | `string` | repeated |  |
| 9 | `importable` | `bool` | optional |  |
| 10 | `suggested` | `bool` | optional |  |
| 11 | `note` | `string` | optional |  |

### `newport.desktop.connection.SshResolveArgs`

A host to look up with `ssh -G`.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `host` | `string` | optional |  |

### `newport.desktop.connection.SshResolved`

What the SSH config sets for a host, as OpenSSH resolves it. A field is set
 only when the config changes it from OpenSSH's default.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `hostname` | `string` | optional |  |
| 2 | `user` | `string` | optional |  |
| 3 | `port` | `uint32` | optional |  |
| 4 | `identity_file` | `string` | optional |  |
| 5 | `proxy_command` | `string` | optional | As written, with %h, %p, %r and %n unexpanded. |
| 6 | `proxy_jump` | `string` | optional |  |

### `newport.desktop.connection.ConfigHosts`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `path` | `string` | optional |  |
| 2 | `hosts` | `newport.desktop.connection.ConfigHost` | repeated |  |

### `newport.desktop.connection.PortsObservation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 7 | `discovery` | `newport.desktop.connection.Discovery` | message presence |  |
| 2 | `error` | `string` | optional |  |
| 3 | `fetching` | `bool` | optional |  |
| 4 | `stale` | `bool` | optional |  |
| 5 | `revision` | `uint64` | optional |  |
| 6 | `updated_at` | `uint64` | optional |  |

Reserved: 1, `data`.

### `newport.desktop.connection.UninstallOutput`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `text` | `string` | optional |  |

### `newport.desktop.connection.SetupInspectArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `expected_server` | `newport.desktop.configuration.Server` | message presence |  |

### `newport.desktop.connection.SetupInstallArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `expected_server` | `newport.desktop.configuration.Server` | message presence |  |

### `newport.desktop.connection.SetupFinishArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `expected_server` | `newport.desktop.configuration.Server` | message presence |  |
| 2 | `browser` | `bool` | optional |  |
| 3 | `clipboard` | `bool` | optional |  |

### `newport.desktop.connection.SetupSkipArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `expected_server` | `newport.desktop.configuration.Server` | message presence |  |

### `newport.desktop.connection.TestConnectionArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |

### `newport.desktop.connection.ConnectionsQueryArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `refresh` | `bool` | optional | Absence uses the source API default. |

### `newport.desktop.connection.ReinstallAgentArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |

### `newport.desktop.connection.UninstallAgentArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `expected_server` | `newport.desktop.configuration.Server` | message presence |  |

### `newport.desktop.connection.InspectAgentArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `expected_server` | `newport.desktop.configuration.Server` | message presence |  |

### `newport.desktop.connection.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `setup_inspect` | `newport.desktop.connection.SetupInspectArgs` | oneof `value` |  |
| 2 | `setup_install` | `newport.desktop.connection.SetupInstallArgs` | oneof `value` |  |
| 3 | `setup_finish` | `newport.desktop.connection.SetupFinishArgs` | oneof `value` |  |
| 4 | `setup_skip` | `newport.desktop.connection.SetupSkipArgs` | oneof `value` |  |
| 5 | `ssh_agent_keys` | `newport.wire.Present` | oneof `value` |  |
| 6 | `ssh_config_hosts` | `newport.wire.Present` | oneof `value` |  |
| 7 | `test_connection` | `newport.desktop.connection.TestConnectionArgs` | oneof `value` |  |
| 8 | `connections_query` | `newport.desktop.connection.ConnectionsQueryArgs` | oneof `value` |  |
| 9 | `reinstall_agent` | `newport.desktop.connection.ReinstallAgentArgs` | oneof `value` |  |
| 10 | `uninstall_agent` | `newport.desktop.connection.UninstallAgentArgs` | oneof `value` |  |
| 11 | `inspect_agent` | `newport.desktop.connection.InspectAgentArgs` | oneof `value` |  |
| 12 | `ssh_resolve` | `newport.desktop.connection.SshResolveArgs` | oneof `value` |  |

### `newport.desktop.connection.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `setup_inspect` | `newport.desktop.connection.SetupInspection` | oneof `value` |  |
| 2 | `setup_install` | `newport.wire.Present` | oneof `value` |  |
| 3 | `setup_finish` | `newport.desktop.connection.OptionalNotice` | oneof `value` |  |
| 4 | `setup_skip` | `newport.wire.Present` | oneof `value` |  |
| 5 | `ssh_agent_keys` | `newport.desktop.connection.AgentKeyList` | oneof `value` |  |
| 6 | `ssh_config_hosts` | `newport.desktop.connection.ConfigHosts` | oneof `value` |  |
| 7 | `test_connection` | `newport.wire.Present` | oneof `value` |  |
| 8 | `connections_query` | `newport.desktop.connection.PortsObservation` | oneof `value` |  |
| 9 | `reinstall_agent` | `newport.wire.Present` | oneof `value` |  |
| 10 | `uninstall_agent` | `newport.desktop.connection.UninstallOutput` | oneof `value` |  |
| 11 | `inspect_agent` | `newport.desktop.configuration.InstallationStatus` | oneof `value` |  |
| 12 | `ssh_resolve` | `newport.desktop.connection.SshResolved` | oneof `value` |  |

## Schema `newport/desktop/containers.proto`

Source: [newport/desktop/containers.proto](proto/newport/desktop/containers.proto). Package: `newport.desktop.containers`.

Imports: `newport/monitor.proto`.

### `newport.desktop.containers.SnapshotArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `refresh` | `bool` | optional | Absence uses the source API default. |

### `newport.desktop.containers.InspectArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `container` | `string` | optional |  |
| 3 | `refresh` | `bool` | optional | Absence uses the source API default. |

### `newport.desktop.containers.LogsArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `request` | `newport.monitor.ContainersLogsRequest` | message presence |  |

### `newport.desktop.containers.ActionArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `container` | `string` | optional |  |
| 3 | `verb` | `newport.monitor.ContainersContainerVerb` | optional |  |

### `newport.desktop.containers.ProjectActionArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `project` | `string` | optional |  |
| 3 | `verb` | `newport.monitor.ContainersProjectVerb` | optional |  |
| 4 | `expected_ids` | `string` | repeated |  |

### `newport.desktop.containers.RemoveArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `containers` | `string` | repeated |  |

### `newport.desktop.containers.DiskArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `part` | `newport.desktop.containers.DiskPart` | optional | Absence uses the source API default. |
| 3 | `refresh` | `bool` | optional | Absence uses the source API default. |

### `newport.desktop.containers.PruneArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `what` | `newport.monitor.ContainersPruneTarget` | optional |  |

### `newport.desktop.containers.ComposeProjectArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `project` | `string` | optional |  |
| 3 | `refresh` | `bool` | optional | Absence uses the source API default. |

### `newport.desktop.containers.ComposeRunArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `project` | `string` | optional |  |
| 3 | `action` | `newport.monitor.ComposeAction` | message presence |  |

### `newport.desktop.containers.ComposeJobArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `job` | `string` | optional |  |
| 3 | `offset` | `uint64` | optional | Absence uses the source API default. |

### `newport.desktop.containers.ComposeCancelArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `job` | `string` | optional |  |

### `newport.desktop.containers.ComposeJobsArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |

### `newport.desktop.containers.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `newport.desktop.containers.SnapshotArgs` | oneof `value` |  |
| 2 | `inspect` | `newport.desktop.containers.InspectArgs` | oneof `value` |  |
| 3 | `logs` | `newport.desktop.containers.LogsArgs` | oneof `value` |  |
| 4 | `action` | `newport.desktop.containers.ActionArgs` | oneof `value` |  |
| 5 | `project_action` | `newport.desktop.containers.ProjectActionArgs` | oneof `value` |  |
| 6 | `remove` | `newport.desktop.containers.RemoveArgs` | oneof `value` |  |
| 7 | `disk` | `newport.desktop.containers.DiskArgs` | oneof `value` |  |
| 8 | `prune` | `newport.desktop.containers.PruneArgs` | oneof `value` |  |
| 9 | `compose_project` | `newport.desktop.containers.ComposeProjectArgs` | oneof `value` |  |
| 10 | `compose_run` | `newport.desktop.containers.ComposeRunArgs` | oneof `value` |  |
| 11 | `compose_job` | `newport.desktop.containers.ComposeJobArgs` | oneof `value` |  |
| 12 | `compose_cancel` | `newport.desktop.containers.ComposeCancelArgs` | oneof `value` |  |
| 13 | `compose_jobs` | `newport.desktop.containers.ComposeJobsArgs` | oneof `value` |  |

### `newport.desktop.containers.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `newport.monitor.ContainersContainersSnapshot` | oneof `value` |  |
| 2 | `inspect` | `newport.monitor.ContainersContainerInspect` | oneof `value` |  |
| 3 | `logs` | `newport.monitor.ContainersContainerLogPage` | oneof `value` |  |
| 4 | `action` | `newport.monitor.ContainersDone` | oneof `value` |  |
| 5 | `project_action` | `newport.monitor.ContainersDone` | oneof `value` |  |
| 6 | `remove` | `newport.monitor.ContainersRemoved` | oneof `value` |  |
| 7 | `disk` | `newport.monitor.ContainersDockerDisk` | oneof `value` |  |
| 8 | `prune` | `newport.monitor.ContainersPruned` | oneof `value` |  |
| 9 | `compose_project` | `newport.monitor.ComposeProject` | oneof `value` |  |
| 10 | `compose_run` | `newport.monitor.ComposeStarted` | oneof `value` |  |
| 11 | `compose_job` | `newport.monitor.ComposeJob` | oneof `value` |  |
| 12 | `compose_cancel` | `newport.monitor.ComposeDone` | oneof `value` |  |
| 13 | `compose_jobs` | `newport.monitor.ComposeJobs` | oneof `value` |  |

### `newport.desktop.containers.DiskPart`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `DISK_PART_UNSPECIFIED` |  |
| 1 | `DISK_PART_FULL` |  |
| 2 | `DISK_PART_QUICK` |  |
| 3 | `DISK_PART_VOLUMES` |  |

## Schema `newport/desktop/events.proto`

Source: [newport/desktop/events.proto](proto/newport/desktop/events.proto). Package: `newport.desktop.events`.

Imports: `newport/git.proto`.

### `newport.desktop.events.GitCommand`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `repo_id` | `string` | optional |  |
| 3 | `operation_id` | `string` | optional |  |
| 4 | `entry` | `newport.git.CommandLog` | message presence |  |

### `newport.desktop.events.Event`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `git_command` | `newport.desktop.events.GitCommand` | oneof `value` |  |

### `newport.desktop.events.Batch`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `cursor` | `uint64` | optional |  |
| 2 | `entries` | `newport.desktop.events.Event` | repeated |  |

### `newport.desktop.events.PollArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `after` | `uint64` | optional |  |

### `newport.desktop.events.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `poll` | `newport.desktop.events.PollArgs` | oneof `value` |  |

### `newport.desktop.events.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `poll` | `newport.desktop.events.Batch` | oneof `value` |  |

## Schema `newport/desktop/files.proto`

Source: [newport/desktop/files.proto](proto/newport/desktop/files.proto). Package: `newport.desktop.files`.

Imports: `newport/desktop/configuration.proto`, `newport/wire.proto`.

### `newport.desktop.files.Entry`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `path` | `string` | optional |  |
| 3 | `kind` | `newport.desktop.files.EntryKind` | optional |  |
| 4 | `size` | `uint64` | optional |  |
| 5 | `modified` | `uint32` | optional |  |
| 6 | `permissions` | `string` | optional |  |

### `newport.desktop.files.Listing`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `path` | `string` | optional |  |
| 2 | `entries` | `newport.desktop.files.Entry` | repeated |  |

### `newport.desktop.files.Content`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `text` | `bytes` | oneof `value` |  |
| 2 | `binary` | `bytes` | oneof `value` |  |

### `newport.desktop.files.Preview`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `kind` | `newport.desktop.files.PreviewKind` | optional |  |
| 2 | `mime` | `string` | optional |  |
| 3 | `content` | `newport.desktop.files.Content` | message presence |  |
| 4 | `truncated` | `bool` | optional |  |

### `newport.desktop.files.Page`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `offset` | `uint64` | optional |  |
| 2 | `next` | `uint64` | optional |  |
| 3 | `size` | `uint64` | optional |  |
| 4 | `reason` | `newport.desktop.files.PageReason` | optional |  |

### `newport.desktop.files.Document`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `path` | `string` | optional |  |
| 2 | `content` | `bytes` | optional |  |
| 3 | `revision` | `string` | optional |  |
| 4 | `page` | `newport.desktop.files.Page` | message presence |  |
| 5 | `cached` | `bool` | optional |  |

### `newport.desktop.files.Index`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `paths` | `string` | repeated |  |
| 2 | `skipped` | `uint64` | optional |  |

### `newport.desktop.files.WatchReply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `mode` | `newport.desktop.files.WatchMode` | optional |  |
| 2 | `cursor` | `string` | optional |  |
| 3 | `paths` | `string` | repeated |  |
| 4 | `reset` | `bool` | optional |  |

### `newport.desktop.files.TransferProgress`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `completed` | `uint64` | optional |  |
| 3 | `total` | `uint64` | optional |  |

### `newport.desktop.files.ProgressLookup`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `progress` | `newport.desktop.files.TransferProgress` | message presence |  |

### `newport.desktop.files.NativePath`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `unix` | `bytes` | oneof `value` |  |
| 2 | `windows` | `bytes` | oneof `value` |  |

### `newport.desktop.files.TransferPath`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `remote` | `string` | oneof `value` |  |
| 2 | `local` | `newport.desktop.files.NativePath` | oneof `value` |  |

### `newport.desktop.files.TransferResult`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `path` | `newport.desktop.files.TransferPath` | message presence |  |

### `newport.desktop.files.ListArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `operation` | `string` | optional |  |
| 3 | `path` | `string` | optional |  |
| 4 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |
| 5 | `refresh` | `bool` | optional | Absence uses the source API default. |
| 6 | `project_root` | `string` | optional |  |

### `newport.desktop.files.WatchArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `operation` | `string` | optional |  |
| 3 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |
| 4 | `root` | `string` | optional |  |
| 5 | `watch` | `string` | optional |  |
| 6 | `directories` | `string` | repeated |  |
| 7 | `cursor` | `string` | optional |  |

### `newport.desktop.files.PreviewArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `operation` | `string` | optional |  |
| 3 | `path` | `string` | optional |  |
| 4 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |

### `newport.desktop.files.ReadTextArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `operation` | `string` | optional |  |
| 3 | `root` | `string` | optional |  |
| 4 | `path` | `string` | optional |  |
| 5 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |
| 6 | `revision` | `string` | optional |  |
| 7 | `offset` | `uint64` | optional |  |
| 8 | `allow_cached` | `bool` | optional | Absence uses the source API default. |

### `newport.desktop.files.ProjectIndexArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `operation` | `string` | optional |  |
| 3 | `root` | `string` | optional |  |
| 4 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |

### `newport.desktop.files.WriteTextArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `operation` | `string` | optional |  |
| 3 | `root` | `string` | optional |  |
| 4 | `path` | `string` | optional |  |
| 5 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |
| 6 | `content` | `bytes` | optional |  |
| 7 | `revision` | `string` | optional |  |

### `newport.desktop.files.CancelArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `operation` | `string` | optional |  |

### `newport.desktop.files.ProgressArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `operation` | `string` | optional |  |

### `newport.desktop.files.UploadArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `operation` | `string` | optional |  |
| 3 | `path` | `string` | optional |  |
| 4 | `local_path` | `newport.desktop.files.NativePath` | message presence |  |
| 5 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |

### `newport.desktop.files.DownloadArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `operation` | `string` | optional |  |
| 3 | `path` | `string` | optional |  |
| 4 | `local_path` | `newport.desktop.files.NativePath` | message presence |  |
| 5 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |

### `newport.desktop.files.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `list` | `newport.desktop.files.ListArgs` | oneof `value` |  |
| 2 | `watch` | `newport.desktop.files.WatchArgs` | oneof `value` |  |
| 3 | `preview` | `newport.desktop.files.PreviewArgs` | oneof `value` |  |
| 4 | `read_text` | `newport.desktop.files.ReadTextArgs` | oneof `value` |  |
| 5 | `project_index` | `newport.desktop.files.ProjectIndexArgs` | oneof `value` |  |
| 6 | `write_text` | `newport.desktop.files.WriteTextArgs` | oneof `value` |  |
| 7 | `cancel` | `newport.desktop.files.CancelArgs` | oneof `value` |  |
| 8 | `progress` | `newport.desktop.files.ProgressArgs` | oneof `value` |  |
| 9 | `upload` | `newport.desktop.files.UploadArgs` | oneof `value` |  |
| 10 | `download` | `newport.desktop.files.DownloadArgs` | oneof `value` |  |

### `newport.desktop.files.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `list` | `newport.desktop.files.Listing` | oneof `value` |  |
| 2 | `watch` | `newport.desktop.files.WatchReply` | oneof `value` |  |
| 3 | `preview` | `newport.desktop.files.Preview` | oneof `value` |  |
| 4 | `read_text` | `newport.desktop.files.Document` | oneof `value` |  |
| 5 | `project_index` | `newport.desktop.files.Index` | oneof `value` |  |
| 6 | `write_text` | `newport.desktop.files.Document` | oneof `value` |  |
| 7 | `cancel` | `newport.wire.Present` | oneof `value` |  |
| 8 | `progress` | `newport.desktop.files.ProgressLookup` | oneof `value` |  |
| 9 | `upload` | `newport.desktop.files.TransferResult` | oneof `value` |  |
| 10 | `download` | `newport.desktop.files.TransferResult` | oneof `value` |  |

### `newport.desktop.files.EntryKind`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `ENTRY_KIND_UNSPECIFIED` |  |
| 1 | `ENTRY_KIND_DIRECTORY` |  |
| 2 | `ENTRY_KIND_SYMLINK` |  |
| 3 | `ENTRY_KIND_FILE` |  |
| 4 | `ENTRY_KIND_OTHER` |  |

### `newport.desktop.files.PreviewKind`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `PREVIEW_KIND_UNSPECIFIED` |  |
| 1 | `PREVIEW_KIND_TEXT` |  |
| 2 | `PREVIEW_KIND_IMAGE` |  |

### `newport.desktop.files.PageReason`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `PAGE_REASON_UNSPECIFIED` |  |
| 1 | `PAGE_REASON_SIZE` |  |
| 2 | `PAGE_REASON_LONG_LINES` |  |

### `newport.desktop.files.WatchMode`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `WATCH_MODE_UNSPECIFIED` |  |
| 1 | `WATCH_MODE_NATIVE` |  |
| 2 | `WATCH_MODE_POLL` |  |

## Schema `newport/desktop/git.proto`

Source: [newport/desktop/git.proto](proto/newport/desktop/git.proto). Package: `newport.desktop.git`.

Imports: `newport/git.proto`, `newport/wire.proto`.

### `newport.desktop.git.CollectedDiff`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `string` | optional |  |
| 2 | `diff` | `newport.git.Diff` | message presence |  |

### `newport.desktop.git.GitResult`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `reply` | `newport.git.Reply` | oneof `value` |  |
| 2 | `diff` | `newport.desktop.git.CollectedDiff` | oneof `value` |  |

### `newport.desktop.git.Refreshed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |

### `newport.desktop.git.Receipt`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `operation_id` | `string` | optional |  |
| 2 | `server` | `string` | optional |  |
| 3 | `action` | `string` | optional |  |
| 4 | `state` | `newport.desktop.git.ReceiptState` | optional |  |

### `newport.desktop.git.Receipts`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `receipts` | `newport.desktop.git.Receipt` | repeated |  |

### `newport.desktop.git.RequestArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `request` | `newport.git.Request` | message presence |  |
| 3 | `retry_connection` | `bool` | optional | Absence uses the source API default. |

### `newport.desktop.git.RefreshRepositoryArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `repo_id` | `string` | optional |  |

### `newport.desktop.git.PendingOperationsArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |

### `newport.desktop.git.AcknowledgeOperationArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `operation_id` | `string` | optional |  |

### `newport.desktop.git.ReviewOperationArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `operation_id` | `string` | optional |  |

### `newport.desktop.git.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `request` | `newport.desktop.git.RequestArgs` | oneof `value` |  |
| 2 | `refresh_repository` | `newport.desktop.git.RefreshRepositoryArgs` | oneof `value` |  |
| 3 | `pending_operations` | `newport.desktop.git.PendingOperationsArgs` | oneof `value` |  |
| 4 | `acknowledge_operation` | `newport.desktop.git.AcknowledgeOperationArgs` | oneof `value` |  |
| 5 | `review_operation` | `newport.desktop.git.ReviewOperationArgs` | oneof `value` |  |

### `newport.desktop.git.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `request` | `newport.desktop.git.GitResult` | oneof `value` |  |
| 2 | `refresh_repository` | `newport.desktop.git.Refreshed` | oneof `value` |  |
| 3 | `pending_operations` | `newport.desktop.git.Receipts` | oneof `value` |  |
| 4 | `acknowledge_operation` | `newport.wire.Present` | oneof `value` |  |
| 5 | `review_operation` | `newport.wire.Present` | oneof `value` |  |

### `newport.desktop.git.ReceiptState`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `RECEIPT_STATE_UNSPECIFIED` |  |
| 1 | `RECEIPT_STATE_PENDING` |  |
| 2 | `RECEIPT_STATE_SUCCEEDED` |  |
| 3 | `RECEIPT_STATE_FAILED` |  |
| 4 | `RECEIPT_STATE_REJECTED` |  |
| 5 | `RECEIPT_STATE_NEEDS_RESOLUTION` |  |
| 6 | `RECEIPT_STATE_OUTCOME_UNKNOWN` |  |
| 7 | `RECEIPT_STATE_REVIEWED_UNKNOWN` |  |

## Schema `newport/desktop/host.proto`

Source: [newport/desktop/host.proto](proto/newport/desktop/host.proto). Package: `newport.desktop.host`.

Imports: `newport/wire.proto`.

### `newport.desktop.host.Startup`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `enabled` | `bool` | optional |  |
| 2 | `available` | `bool` | optional |  |

### `newport.desktop.host.BuildStamp`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `commit` | `string` | optional |  |
| 2 | `local` | `bool` | optional |  |
| 3 | `built_at` | `int64` | optional |  |

### `newport.desktop.host.UpdateStatus`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `enabled` | `bool` | optional |  |
| 2 | `current_version` | `string` | optional |  |
| 3 | `build` | `newport.desktop.host.BuildStamp` | message presence |  |
| 4 | `phase` | `newport.desktop.host.UpdatePhase` | optional |  |
| 5 | `version` | `string` | optional |  |
| 6 | `downloaded` | `uint64` | optional |  |
| 7 | `total` | `uint64` | optional |  |
| 8 | `error` | `string` | optional |  |

### `newport.desktop.host.OpenRemoteAppArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `action` | `string` | optional |  |

### `newport.desktop.host.SetLaunchAtLoginArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `enabled` | `bool` | optional |  |

### `newport.desktop.host.InstallUpdateArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `confirmed_version` | `string` | optional |  |

### `newport.desktop.host.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `activate` | `newport.wire.Present` | oneof `value` |  |
| 2 | `open_remote_app` | `newport.desktop.host.OpenRemoteAppArgs` | oneof `value` |  |
| 3 | `startup_settings` | `newport.wire.Present` | oneof `value` |  |
| 4 | `set_launch_at_login` | `newport.desktop.host.SetLaunchAtLoginArgs` | oneof `value` |  |
| 5 | `update_status` | `newport.wire.Present` | oneof `value` |  |
| 6 | `check_for_updates` | `newport.wire.Present` | oneof `value` |  |
| 7 | `install_update` | `newport.desktop.host.InstallUpdateArgs` | oneof `value` |  |

### `newport.desktop.host.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `activate` | `newport.wire.Present` | oneof `value` |  |
| 2 | `open_remote_app` | `newport.wire.Present` | oneof `value` |  |
| 3 | `startup_settings` | `newport.desktop.host.Startup` | oneof `value` |  |
| 4 | `set_launch_at_login` | `newport.desktop.host.Startup` | oneof `value` |  |
| 5 | `update_status` | `newport.desktop.host.UpdateStatus` | oneof `value` |  |
| 6 | `check_for_updates` | `newport.wire.Present` | oneof `value` |  |
| 7 | `install_update` | `newport.wire.Present` | oneof `value` |  |

### `newport.desktop.host.UpdatePhase`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `UPDATE_PHASE_UNSPECIFIED` |  |
| 1 | `UPDATE_PHASE_IDLE` |  |
| 2 | `UPDATE_PHASE_CHECKING` |  |
| 3 | `UPDATE_PHASE_DOWNLOADING` |  |
| 4 | `UPDATE_PHASE_READY` |  |
| 5 | `UPDATE_PHASE_INSTALLING` |  |
| 6 | `UPDATE_PHASE_CURRENT` |  |
| 7 | `UPDATE_PHASE_ERROR` |  |

## Schema `newport/desktop/metrics.proto`

Source: [newport/desktop/metrics.proto](proto/newport/desktop/metrics.proto). Package: `newport.desktop.metrics`.

Imports: `newport/monitor.proto`, `newport/wire.proto`.

### `newport.desktop.metrics.Reading`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `sample` | `newport.monitor.MetricsSnapshot` | message presence |  |
| 2 | `sampled_at` | `int64` | optional |  |
| 3 | `collection_error` | `string` | optional |  |
| 4 | `history_error` | `string` | optional |  |

### `newport.desktop.metrics.NumericMeasures`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `cpu` | `double` | optional |  |
| 2 | `memory_used` | `double` | optional |  |
| 3 | `memory_total` | `double` | optional |  |
| 4 | `swap_used` | `double` | optional |  |
| 5 | `swap_total` | `double` | optional |  |
| 6 | `load_one` | `double` | optional |  |
| 7 | `load_five` | `double` | optional |  |
| 8 | `load_fifteen` | `double` | optional |  |

### `newport.desktop.metrics.NetworkHistory`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `received` | `uint64` | optional |  |
| 3 | `sent` | `uint64` | optional |  |
| 4 | `rx` | `double` | optional |  |
| 5 | `tx` | `double` | optional |  |
| 6 | `rx_peak` | `double` | optional |  |
| 7 | `tx_peak` | `double` | optional |  |

### `newport.desktop.metrics.ChartBucket`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `resolution_ms` | `uint64` | optional |  |
| 2 | `sample_count` | `uint64` | optional |  |
| 3 | `gap` | `bool` | optional |  |
| 4 | `means` | `newport.desktop.metrics.NumericMeasures` | message presence |  |
| 5 | `peaks` | `newport.desktop.metrics.NumericMeasures` | message presence |  |
| 6 | `network` | `newport.desktop.metrics.NetworkHistory` | repeated |  |

### `newport.desktop.metrics.HistoricalDisk`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `mount` | `string` | optional |  |
| 2 | `device` | `string` | optional |  |
| 3 | `total` | `uint64` | optional |  |
| 4 | `used` | `uint64` | optional |  |
| 5 | `available` | `int64` | optional |  |

### `newport.desktop.metrics.HistoricalDisks`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `items` | `newport.desktop.metrics.HistoricalDisk` | repeated |  |

### `newport.desktop.metrics.HistoricalNetwork`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `items` | `newport.desktop.metrics.NetworkHistory` | repeated |  |

### `newport.desktop.metrics.HistoricalRaw`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `measures` | `newport.desktop.metrics.NumericMeasures` | message presence |  |
| 2 | `uptime` | `double` | optional |  |
| 3 | `network` | `newport.desktop.metrics.HistoricalNetwork` | message presence | Required wrapper distinguishes a missing network observation from an empty list. |
| 4 | `disks` | `newport.desktop.metrics.HistoricalDisks` | message presence |  |

### `newport.desktop.metrics.MinuteRollup`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `summary` | `newport.desktop.metrics.ChartBucket` | message presence |  |
| 2 | `uptime` | `double` | optional |  |
| 3 | `disks` | `newport.desktop.metrics.HistoricalDisks` | message presence |  |

### `newport.desktop.metrics.SampleValue`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `raw` | `newport.desktop.metrics.HistoricalRaw` | oneof `value` |  |
| 2 | `minute` | `newport.desktop.metrics.MinuteRollup` | oneof `value` |  |
| 3 | `chart` | `newport.desktop.metrics.ChartBucket` | oneof `value` |  |

### `newport.desktop.metrics.Sample`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `at` | `int64` | optional |  |
| 2 | `value` | `newport.desktop.metrics.SampleValue` | message presence |  |

### `newport.desktop.metrics.History`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `samples` | `newport.desktop.metrics.Sample` | repeated |  |

### `newport.desktop.metrics.CacheInfo`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `bytes` | `uint64` | optional |  |
| 2 | `samples` | `uint64` | optional |  |

### `newport.desktop.metrics.CollectArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `refresh` | `bool` | optional | Absence uses the source API default. |
| 3 | `section` | `newport.desktop.metrics.Section` | optional |  |

### `newport.desktop.metrics.HistoryArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `minutes` | `uint32` | optional |  |

### `newport.desktop.metrics.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `collect` | `newport.desktop.metrics.CollectArgs` | oneof `value` |  |
| 2 | `history` | `newport.desktop.metrics.HistoryArgs` | oneof `value` |  |
| 3 | `cache` | `newport.wire.Present` | oneof `value` |  |
| 4 | `clear_cache` | `newport.wire.Present` | oneof `value` |  |

### `newport.desktop.metrics.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `collect` | `newport.desktop.metrics.Reading` | oneof `value` |  |
| 2 | `history` | `newport.desktop.metrics.History` | oneof `value` |  |
| 3 | `cache` | `newport.desktop.metrics.CacheInfo` | oneof `value` |  |
| 4 | `clear_cache` | `newport.desktop.metrics.CacheInfo` | oneof `value` |  |

### `newport.desktop.metrics.Section`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `SECTION_UNSPECIFIED` |  |
| 1 | `SECTION_OVERVIEW` |  |

## Schema `newport/desktop/preferences.proto`

Source: [newport/desktop/preferences.proto](proto/newport/desktop/preferences.proto). Package: `newport.desktop.preferences`.

Imports: `newport/git.proto`, `newport/wire.proto`.

### `newport.desktop.preferences.WindowPatch`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `sidebar_width` | `float` | optional |  |
| 2 | `sidebar_compact` | `bool` | optional |  |
| 3 | `appearance` | `newport.desktop.preferences.Appearance` | optional |  |
| 4 | `terminal_option_as_meta` | `bool` | optional |  |

### `newport.desktop.preferences.Project`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `server` | `string` | optional |  |
| 3 | `name` | `string` | optional |  |
| 4 | `path` | `newport.git.Path` | message presence |  |

### `newport.desktop.preferences.Projects`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `projects` | `newport.desktop.preferences.Project` | repeated |  |

### `newport.desktop.preferences.Settings`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `audio` | `bool` | optional |  |
| 2 | `codec` | `newport.desktop.preferences.Codec` | optional |  |
| 3 | `fps_goal` | `uint32` | optional |  |
| 4 | `hud` | `bool` | optional |  |
| 5 | `separate_apps` | `bool` | optional |  |

### `newport.desktop.preferences.Installed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `servers` | `string` | repeated |  |

### `newport.desktop.preferences.SetWindowArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `patch` | `newport.desktop.preferences.WindowPatch` | message presence |  |

### `newport.desktop.preferences.ProjectsListArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |

### `newport.desktop.preferences.ProjectsSaveArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `project` | `newport.desktop.preferences.Project` | message presence |  |

### `newport.desktop.preferences.ProjectsRemoveArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `project` | `string` | optional |  |

### `newport.desktop.preferences.SettingsSaveArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `settings` | `newport.desktop.preferences.Settings` | message presence |  |

### `newport.desktop.preferences.InstalledSaveArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `installed` | `bool` | optional |  |

### `newport.desktop.preferences.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `set_window` | `newport.desktop.preferences.SetWindowArgs` | oneof `value` |  |
| 2 | `projects_list` | `newport.desktop.preferences.ProjectsListArgs` | oneof `value` |  |
| 3 | `projects_save` | `newport.desktop.preferences.ProjectsSaveArgs` | oneof `value` |  |
| 4 | `projects_remove` | `newport.desktop.preferences.ProjectsRemoveArgs` | oneof `value` |  |
| 5 | `settings` | `newport.wire.Present` | oneof `value` |  |
| 6 | `settings_save` | `newport.desktop.preferences.SettingsSaveArgs` | oneof `value` |  |
| 7 | `installed` | `newport.wire.Present` | oneof `value` |  |
| 8 | `installed_save` | `newport.desktop.preferences.InstalledSaveArgs` | oneof `value` |  |

### `newport.desktop.preferences.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `set_window` | `newport.wire.Present` | oneof `value` |  |
| 2 | `projects_list` | `newport.desktop.preferences.Projects` | oneof `value` |  |
| 3 | `projects_save` | `newport.desktop.preferences.Project` | oneof `value` |  |
| 4 | `projects_remove` | `newport.wire.Present` | oneof `value` |  |
| 5 | `settings` | `newport.desktop.preferences.Settings` | oneof `value` |  |
| 6 | `settings_save` | `newport.desktop.preferences.Settings` | oneof `value` |  |
| 7 | `installed` | `newport.desktop.preferences.Installed` | oneof `value` |  |
| 8 | `installed_save` | `newport.desktop.preferences.Installed` | oneof `value` |  |

### `newport.desktop.preferences.Appearance`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `APPEARANCE_UNSPECIFIED` |  |
| 1 | `APPEARANCE_SYSTEM` |  |
| 2 | `APPEARANCE_LIGHT` |  |
| 3 | `APPEARANCE_DARK` |  |

### `newport.desktop.preferences.Codec`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CODEC_UNSPECIFIED` |  |
| 1 | `CODEC_AUTO` |  |
| 2 | `CODEC_LOSSLESS` |  |
| 3 | `CODEC_FAST` |  |

## Schema `newport/desktop/services.proto`

Source: [newport/desktop/services.proto](proto/newport/desktop/services.proto). Package: `newport.desktop.services`.

Imports: `newport/monitor.proto`.

### `newport.desktop.services.SnapshotArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `refresh` | `bool` | optional | Absence uses the source API default. |

### `newport.desktop.services.LogsArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `request` | `newport.monitor.ServicesLogsRequest` | message presence |  |

### `newport.desktop.services.UnitFileArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `unit` | `string` | optional |  |
| 3 | `scope` | `newport.monitor.ServicesScope` | optional |  |
| 4 | `refresh` | `bool` | optional | Absence uses the source API default. |

### `newport.desktop.services.ActionArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `unit` | `string` | optional |  |
| 3 | `scope` | `newport.monitor.ServicesScope` | optional |  |
| 4 | `verb` | `newport.monitor.ServicesVerb` | optional |  |

### `newport.desktop.services.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `newport.desktop.services.SnapshotArgs` | oneof `value` |  |
| 2 | `logs` | `newport.desktop.services.LogsArgs` | oneof `value` |  |
| 3 | `unit_file` | `newport.desktop.services.UnitFileArgs` | oneof `value` |  |
| 4 | `action` | `newport.desktop.services.ActionArgs` | oneof `value` |  |

### `newport.desktop.services.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `newport.monitor.ServicesSnapshot` | oneof `value` |  |
| 2 | `logs` | `newport.monitor.ServicesLogs` | oneof `value` |  |
| 3 | `unit_file` | `newport.monitor.ServicesUnitFile` | oneof `value` |  |
| 4 | `action` | `newport.monitor.ServicesActionReply` | oneof `value` |  |

## Schema `newport/desktop/stream.proto`

Source: [newport/desktop/stream.proto](proto/newport/desktop/stream.proto). Package: `newport.desktop.stream`.

Imports: `newport/desktop/common.proto`, `newport/desktop/configuration.proto`, `newport/wire.proto`.

### `newport.desktop.stream.TerminalOpenArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `session` | `string` | optional |  |
| 3 | `columns` | `uint32` | optional |  |
| 4 | `rows` | `uint32` | optional |  |
| 5 | `persist` | `bool` | optional | Absence uses the source API default. |
| 6 | `attach` | `bool` | optional | Absence uses the source API default. |
| 7 | `colors` | `bytes` | optional |  |

### `newport.desktop.stream.CommandOpenArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `command` | `string` | optional |  |

### `newport.desktop.stream.RemoteAppsOpenArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `arguments` | `string` | repeated |  |

### `newport.desktop.stream.LspOpenArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `expected` | `newport.desktop.configuration.ConnectionScope` | message presence |  |
| 3 | `root` | `string` | optional |  |
| 4 | `server_name` | `string` | optional |  |
| 5 | `envrc` | `bool` | optional |  |

### `newport.desktop.stream.TerminalOpened`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | optional |  |
| 2 | `session` | `string` | optional |  |

### `newport.desktop.stream.CommandOpened`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | optional |  |
| 2 | `server` | `string` | optional |  |

### `newport.desktop.stream.RemoteAppsOpened`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | optional |  |
| 2 | `server` | `string` | optional |  |
| 3 | `service_version` | `uint32` | optional |  |

### `newport.desktop.stream.LspOpened`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | optional |  |
| 2 | `server` | `string` | optional |  |

### `newport.desktop.stream.Opened`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `terminal` | `newport.desktop.stream.TerminalOpened` | oneof `value` |  |
| 2 | `command` | `newport.desktop.stream.CommandOpened` | oneof `value` |  |
| 3 | `remote_apps` | `newport.desktop.stream.RemoteAppsOpened` | oneof `value` |  |
| 4 | `lsp` | `newport.desktop.stream.LspOpened` | oneof `value` |  |

### `newport.desktop.stream.CommandCompletion`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `status` | `uint32` | optional |  |
| 2 | `signal` | `string` | optional |  |
| 3 | `ending` | `newport.desktop.stream.Ending` | optional |  |
| 4 | `stderr` | `bytes` | optional |  |
| 5 | `error` | `newport.wire.Error` | message presence |  |

### `newport.desktop.stream.LspCompletion`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `error` | `newport.wire.Error` | message presence |  |

### `newport.desktop.stream.Completion`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `command` | `newport.desktop.stream.CommandCompletion` | oneof `value` |  |
| 2 | `lsp` | `newport.desktop.stream.LspCompletion` | oneof `value` |  |

### `newport.desktop.stream.OpaqueClient`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `data` | `bytes` | oneof `value` |  |
| 2 | `input_complete` | `newport.wire.Present` | oneof `value` |  |

### `newport.desktop.stream.OpaqueServer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `data` | `bytes` | oneof `value` |  |
| 2 | `completion` | `newport.desktop.stream.Completion` | oneof `value` |  |

### `newport.desktop.stream.Input`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `epoch` | `uint64` | optional |  |
| 2 | `data` | `bytes` | optional |  |

### `newport.desktop.stream.Resize`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `columns` | `uint32` | optional |  |
| 2 | `rows` | `uint32` | optional |  |

### `newport.desktop.stream.Ack`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `bytes` | `uint64` | optional |  |

### `newport.desktop.stream.TerminalClient`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `input` | `newport.desktop.stream.Input` | oneof `value` |  |
| 2 | `resize` | `newport.desktop.stream.Resize` | oneof `value` |  |
| 3 | `ack` | `newport.desktop.stream.Ack` | oneof `value` |  |
| 4 | `end` | `newport.wire.Present` | oneof `value` |  |
| 5 | `activate` | `newport.wire.Present` | oneof `value` |  |

### `newport.desktop.stream.Output`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `data` | `bytes` | optional |  |

### `newport.desktop.stream.Taken`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `bytes` | `uint64` | optional |  |

### `newport.desktop.stream.Restore`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `state` | `bytes` | optional |  |
| 2 | `prompted` | `bool` | optional |  |

### `newport.desktop.stream.Reconnecting`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `attempt` | `uint32` | optional |  |
| 2 | `message` | `string` | optional |  |

### `newport.desktop.stream.Exit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `code` | `uint32` | optional |  |

### `newport.desktop.stream.Active`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `active` | `bool` | optional |  |

### `newport.desktop.stream.NotHeld`

A new tab's shell that Needle couldn't hold: an ordinary shell runs
 instead, and ends when the connection does. `reason` is Needle's.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `reason` | `string` | optional |  |

### `newport.desktop.stream.TerminalEvent`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ready` | `newport.wire.Present` | oneof `value` |  |
| 2 | `persistent` | `newport.wire.Present` | oneof `value` |  |
| 3 | `reconnecting` | `newport.desktop.stream.Reconnecting` | oneof `value` |  |
| 4 | `exit` | `newport.desktop.stream.Exit` | oneof `value` |  |
| 5 | `error` | `newport.desktop.common.Failure` | oneof `value` |  |
| 6 | `ended` | `newport.wire.Present` | oneof `value` |  |
| 7 | `active` | `newport.desktop.stream.Active` | oneof `value` |  |
| 8 | `display_lost` | `newport.wire.Present` | oneof `value` |  |
| 9 | `not_held` | `newport.desktop.stream.NotHeld` | oneof `value` |  |
| 10 | `size` | `newport.desktop.stream.Resize` | oneof `value` | The held shell's size, which every tab with it open draws it at. |

### `newport.desktop.stream.TerminalServer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `output` | `newport.desktop.stream.Output` | oneof `value` |  |
| 2 | `taken` | `newport.desktop.stream.Taken` | oneof `value` |  |
| 3 | `restore` | `newport.desktop.stream.Restore` | oneof `value` |  |
| 4 | `event` | `newport.desktop.stream.TerminalEvent` | oneof `value` |  |

### `newport.desktop.stream.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `terminal` | `newport.desktop.stream.TerminalOpenArgs` | oneof `value` |  |
| 2 | `command` | `newport.desktop.stream.CommandOpenArgs` | oneof `value` |  |
| 3 | `remote_apps` | `newport.desktop.stream.RemoteAppsOpenArgs` | oneof `value` |  |
| 4 | `lsp` | `newport.desktop.stream.LspOpenArgs` | oneof `value` |  |

### `newport.desktop.stream.Ending`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `ENDING_UNSPECIFIED` |  |
| 1 | `ENDING_EXITED` |  |
| 2 | `ENDING_STOPPED` |  |
| 3 | `ENDING_REFUSED` |  |
| 4 | `ENDING_LOST` |  |

## Schema `newport/desktop/stub.proto`

Source: [newport/desktop/stub.proto](proto/newport/desktop/stub.proto). Package: `newport.desktop.stub`.

### `newport.desktop.stub.Empty`

Empty message.

### `newport.desktop.stub.Point`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `x` | `sint32` | optional |  |
| 2 | `y` | `sint32` | optional |  |

### `newport.desktop.stub.Size`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `width` | `uint32` | optional |  |
| 2 | `height` | `uint32` | optional |  |

### `newport.desktop.stub.Position`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `x` | `double` | optional |  |
| 2 | `y` | `double` | optional |  |

### `newport.desktop.stub.Area`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `x` | `double` | optional |  |
| 2 | `y` | `double` | optional |  |
| 3 | `width` | `double` | optional |  |
| 4 | `height` | `double` | optional |  |

### `newport.desktop.stub.Actions`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `close` | `bool` | optional |  |
| 2 | `minimize` | `bool` | optional |  |
| 3 | `maximize` | `bool` | optional |  |
| 4 | `resize` | `bool` | optional |  |
| 5 | `fullscreen` | `bool` | optional |  |

### `newport.desktop.stub.WindowState`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `kind` | `newport.desktop.stub.WindowKind` | optional |  |
| 4 | `parent` | `uint64` | optional |  |
| 5 | `modal` | `bool` | optional |  |
| 6 | `position` | `newport.desktop.stub.Point` | message presence |  |
| 7 | `logical` | `newport.desktop.stub.Size` | message presence |  |
| 8 | `pixels` | `newport.desktop.stub.Size` | message presence |  |
| 9 | `apply_size` | `bool` | optional |  |
| 10 | `decoration` | `newport.desktop.stub.Decoration` | optional |  |
| 11 | `actions` | `newport.desktop.stub.Actions` | message presence |  |
| 12 | `min` | `newport.desktop.stub.Size` | message presence |  |
| 13 | `max` | `newport.desktop.stub.Size` | message presence |  |
| 14 | `title` | `string` | optional |  |
| 15 | `activate` | `newport.desktop.stub.Activate` | optional |  |

### `newport.desktop.stub.HostHello`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | optional |  |
| 2 | `reason` | `newport.desktop.stub.LaunchReason` | optional |  |
| 3 | `bundle_id` | `string` | optional |  |

### `newport.desktop.stub.HostPointer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `x` | `double` | optional |  |
| 4 | `y` | `double` | optional |  |
| 5 | `modifiers` | `uint32` | optional |  |

### `newport.desktop.stub.HostButton`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `x` | `double` | optional |  |
| 4 | `y` | `double` | optional |  |
| 5 | `button` | `uint32` | optional |  |
| 6 | `down` | `bool` | optional |  |
| 7 | `modifiers` | `uint32` | optional |  |

### `newport.desktop.stub.HostScroll`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `x` | `double` | optional |  |
| 4 | `y` | `double` | optional |  |
| 5 | `dx` | `double` | optional |  |
| 6 | `dy` | `double` | optional |  |
| 7 | `precise` | `bool` | optional |  |
| 8 | `phase` | `uint64` | optional |  |
| 9 | `momentum` | `uint64` | optional |  |
| 10 | `modifiers` | `uint32` | optional |  |

### `newport.desktop.stub.HostKey`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `code` | `uint32` | optional |  |
| 4 | `characters` | `string` | optional |  |
| 5 | `unmodified` | `string` | optional |  |
| 6 | `modifiers` | `uint32` | optional |  |
| 7 | `repeat` | `bool` | optional |  |

### `newport.desktop.stub.HostKeyUp`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `code` | `uint32` | optional |  |
| 4 | `modifiers` | `uint32` | optional |  |

### `newport.desktop.stub.HostModifiers`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `code` | `uint32` | optional |  |
| 4 | `flags` | `uint64` | optional |  |

### `newport.desktop.stub.HostExit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |

### `newport.desktop.stub.HostText`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `text` | `string` | optional |  |

### `newport.desktop.stub.HostPreedit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `text` | `string` | optional |  |
| 4 | `cursor` | `uint32` | optional |  |

### `newport.desktop.stub.HostFocus`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `key` | `bool` | optional |  |

### `newport.desktop.stub.HostResized`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `logical` | `newport.desktop.stub.Size` | message presence |  |

### `newport.desktop.stub.HostCloseAsked`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |

### `newport.desktop.stub.HostMinimized`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `minimized` | `bool` | optional |  |

### `newport.desktop.stub.HostFullscreen`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `enabled` | `bool` | optional |  |

### `newport.desktop.stub.HostPresented`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `index` | `uint32` | optional |  |

### `newport.desktop.stub.HostAppActive`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `active` | `bool` | optional |  |

### `newport.desktop.stub.HostDisplay`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `scale` | `double` | optional |  |
| 3 | `work_area` | `newport.desktop.stub.Area` | message presence |  |
| 4 | `content` | `newport.desktop.stub.Position` | message presence |  |

### `newport.desktop.stub.HostVisible`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `visible` | `bool` | optional |  |

### `newport.desktop.stub.HostFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | optional |  |
| 2 | `hello` | `newport.desktop.stub.HostHello` | oneof `payload` |  |
| 3 | `pointer` | `newport.desktop.stub.HostPointer` | oneof `payload` |  |
| 4 | `button` | `newport.desktop.stub.HostButton` | oneof `payload` |  |
| 5 | `scroll` | `newport.desktop.stub.HostScroll` | oneof `payload` |  |
| 6 | `key` | `newport.desktop.stub.HostKey` | oneof `payload` |  |
| 7 | `key_up` | `newport.desktop.stub.HostKeyUp` | oneof `payload` |  |
| 8 | `modifiers` | `newport.desktop.stub.HostModifiers` | oneof `payload` |  |
| 9 | `exit` | `newport.desktop.stub.HostExit` | oneof `payload` |  |
| 10 | `text` | `newport.desktop.stub.HostText` | oneof `payload` |  |
| 11 | `preedit` | `newport.desktop.stub.HostPreedit` | oneof `payload` |  |
| 12 | `focus` | `newport.desktop.stub.HostFocus` | oneof `payload` |  |
| 13 | `resized` | `newport.desktop.stub.HostResized` | oneof `payload` |  |
| 14 | `close_asked` | `newport.desktop.stub.HostCloseAsked` | oneof `payload` |  |
| 15 | `minimized` | `newport.desktop.stub.HostMinimized` | oneof `payload` |  |
| 16 | `fullscreen` | `newport.desktop.stub.HostFullscreen` | oneof `payload` |  |
| 17 | `presented` | `newport.desktop.stub.HostPresented` | oneof `payload` |  |
| 18 | `app_active` | `newport.desktop.stub.HostAppActive` | oneof `payload` |  |
| 19 | `quit_asked` | `newport.desktop.stub.Empty` | oneof `payload` |  |
| 20 | `reopen` | `newport.desktop.stub.Empty` | oneof `payload` |  |
| 21 | `display` | `newport.desktop.stub.HostDisplay` | oneof `payload` |  |
| 22 | `visible` | `newport.desktop.stub.HostVisible` | oneof `payload` |  |

### `newport.desktop.stub.StubWelcome`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `server` | `string` | optional |  |

### `newport.desktop.stub.StubWindow`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `state` | `newport.desktop.stub.WindowState` | message presence |  |
| 2 | `surfaces` | `uint32` | optional |  |

### `newport.desktop.stub.StubSurface`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `index` | `uint32` | optional |  |

### `newport.desktop.stub.StubPresentFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `generation` | `uint64` | optional |  |
| 3 | `index` | `uint32` | optional |  |

### `newport.desktop.stub.StubTitle`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `title` | `string` | optional |  |

### `newport.desktop.stub.StubOperation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `operation` | `newport.desktop.stub.OperationValue` | message presence |  |

### `newport.desktop.stub.StubDestroy`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |

### `newport.desktop.stub.StubQuitReply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `allow` | `bool` | optional |  |

### `newport.desktop.stub.StubAlert`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `message` | `string` | optional |  |
| 2 | `detail` | `string` | optional |  |

### `newport.desktop.stub.StubTextContext`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `editing` | `bool` | optional |  |
| 3 | `caret` | `newport.desktop.stub.Point` | message presence |  |

### `newport.desktop.stub.StubCursor`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `uint64` | optional |  |
| 2 | `cursor` | `uint32` | optional |  |

### `newport.desktop.stub.StubFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | optional |  |
| 2 | `welcome` | `newport.desktop.stub.StubWelcome` | oneof `payload` |  |
| 3 | `window` | `newport.desktop.stub.StubWindow` | oneof `payload` |  |
| 4 | `surface` | `newport.desktop.stub.StubSurface` | oneof `payload` |  |
| 5 | `frame` | `newport.desktop.stub.StubPresentFrame` | oneof `payload` |  |
| 6 | `title` | `newport.desktop.stub.StubTitle` | oneof `payload` |  |
| 7 | `operation` | `newport.desktop.stub.StubOperation` | oneof `payload` |  |
| 8 | `destroy` | `newport.desktop.stub.StubDestroy` | oneof `payload` |  |
| 9 | `quit_reply` | `newport.desktop.stub.StubQuitReply` | oneof `payload` |  |
| 10 | `alert` | `newport.desktop.stub.StubAlert` | oneof `payload` |  |
| 11 | `quit` | `newport.desktop.stub.Empty` | oneof `payload` |  |
| 12 | `text_context` | `newport.desktop.stub.StubTextContext` | oneof `payload` |  |
| 13 | `cursor` | `newport.desktop.stub.StubCursor` | oneof `payload` |  |

### `newport.desktop.stub.OperationZoom`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `maximized` | `bool` | optional |  |

### `newport.desktop.stub.OperationFullscreen`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `enabled` | `bool` | optional |  |

### `newport.desktop.stub.OperationValue`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `move` | `newport.desktop.stub.Empty` | oneof `payload` |  |
| 2 | `minimize` | `newport.desktop.stub.Empty` | oneof `payload` |  |
| 3 | `zoom` | `newport.desktop.stub.OperationZoom` | oneof `payload` |  |
| 4 | `fullscreen` | `newport.desktop.stub.OperationFullscreen` | oneof `payload` |  |
| 5 | `activate` | `newport.desktop.stub.Empty` | oneof `payload` |  |

### `newport.desktop.stub.LaunchReason`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `LAUNCH_REASON_UNSPECIFIED` |  |
| 1 | `LAUNCH_REASON_USER` |  |
| 2 | `LAUNCH_REASON_HOST` |  |

### `newport.desktop.stub.WindowKind`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `WINDOW_KIND_UNSPECIFIED` |  |
| 1 | `WINDOW_KIND_NORMAL` |  |
| 2 | `WINDOW_KIND_DIALOG` |  |
| 3 | `WINDOW_KIND_MENU` |  |
| 4 | `WINDOW_KIND_TOOLTIP` |  |

### `newport.desktop.stub.Decoration`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `DECORATION_UNSPECIFIED` |  |
| 1 | `DECORATION_SERVER` |  |
| 2 | `DECORATION_CLIENT` |  |
| 3 | `DECORATION_NONE` |  |

### `newport.desktop.stub.Activate`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `ACTIVATE_UNSPECIFIED` |  |
| 1 | `ACTIVATE_NO` |  |
| 2 | `ACTIVATE_YES` |  |
| 3 | `ACTIVATE_ATTENTION` |  |

## Schema `newport/desktop/terminal.proto`

Source: [newport/desktop/terminal.proto](proto/newport/desktop/terminal.proto). Package: `newport.desktop.terminal`.

Imports: `newport/terminal.proto`, `newport/wire.proto`.

### `newport.desktop.terminal.HeldShell`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `terminal` | `string` | optional |  |
| 2 | `name` | `string` | optional |  |
| 3 | `title` | `string` | optional |  |
| 4 | `creator` | `newport.terminal.Computer` | message presence |  |
| 5 | `attached` | `newport.terminal.Attached` | repeated |  |
| 6 | `started` | `uint64` | optional |  |
| 7 | `exited` | `bool` | optional |  |
| 8 | `agent` | `string` | optional |  |

### `newport.desktop.terminal.ShellList`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `shells` | `newport.desktop.terminal.HeldShell` | repeated |  |

### `newport.desktop.terminal.HeldShells`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `desktop` | `string` | optional |  |
| 2 | `holds` | `bool` | optional |  |
| 3 | `shells` | `newport.desktop.terminal.ShellList` | message presence |  |

### `newport.desktop.terminal.Renamed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `renamed` | `bool` | optional |  |

### `newport.desktop.terminal.Ended`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ended` | `bool` | optional |  |

### `newport.desktop.terminal.CloseArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `string` | optional |  |

### `newport.desktop.terminal.ListArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |

### `newport.desktop.terminal.RenameArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `terminal` | `string` | optional |  |
| 3 | `name` | `string` | optional |  |

### `newport.desktop.terminal.EndArgs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `server` | `string` | optional |  |
| 2 | `terminal` | `string` | optional |  |

### `newport.desktop.terminal.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `close` | `newport.desktop.terminal.CloseArgs` | oneof `value` |  |
| 2 | `list` | `newport.desktop.terminal.ListArgs` | oneof `value` |  |
| 3 | `rename` | `newport.desktop.terminal.RenameArgs` | oneof `value` |  |
| 4 | `end` | `newport.desktop.terminal.EndArgs` | oneof `value` |  |

### `newport.desktop.terminal.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `close` | `newport.wire.Present` | oneof `value` |  |
| 2 | `list` | `newport.desktop.terminal.HeldShells` | oneof `value` |  |
| 3 | `rename` | `newport.desktop.terminal.Renamed` | oneof `value` |  |
| 4 | `end` | `newport.desktop.terminal.Ended` | oneof `value` |  |

## Schema `newport/discovery.proto`

Source: [newport/discovery.proto](proto/newport/discovery.proto). Package: `newport.discovery`.

Imports: `newport/wire.proto`.

### `newport.discovery.Request`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `refresh` | `bool` | implicit |  |

### `newport.discovery.HelperRequest`

A finite scan or project enrichment. The caller retains its control pipe.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `contract_id` | `string` | optional |  |
| 2 | `generation` | `uint64` | implicit |  |
| 3 | `projects` | `newport.discovery.Snapshot` | message presence | Only the unprivileged projects command accepts filesystem enrichment. |

### `newport.discovery.HelperReply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `contract_id` | `string` | optional |  |
| 2 | `generation` | `uint64` | implicit |  |
| 3 | `snapshot` | `newport.discovery.Snapshot` | oneof `result` |  |
| 4 | `error` | `newport.wire.Error` | oneof `result` |  |

### `newport.discovery.Snapshot`

All identifiers and observations are scoped to one scan on one server.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `generation` | `uint64` | implicit |  |
| 2 | `started_at_ms` | `uint64` | optional |  |
| 3 | `elapsed_ms` | `uint64` | optional |  |
| 4 | `scope` | `newport.discovery.Scope` | message presence |  |
| 5 | `sockets` | `newport.discovery.Socket` | repeated |  |
| 6 | `processes` | `newport.discovery.Process` | repeated |  |
| 7 | `services` | `newport.discovery.Service` | repeated |  |
| 8 | `publications` | `newport.discovery.Publication` | repeated |  |
| 9 | `coverage` | `newport.discovery.Coverage` | message presence |  |
| 10 | `container_engine` | `string` | optional | Stable identity, including empty inventories. Absence prevents complete coverage. |
| 11 | `diagnostics` | `newport.discovery.Diagnostics` | message presence |  |

### `newport.discovery.Diagnostics`

Selected inventory sources remain identifiable when an inventory is empty.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ipv4_source` | `newport.discovery.SocketSource` | optional |  |
| 2 | `ipv6_source` | `newport.discovery.SocketSource` | optional |  |

### `newport.discovery.Namespace`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `device` | `uint64` | optional |  |
| 2 | `inode` | `uint64` | implicit |  |

### `newport.discovery.Scope`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `network` | `newport.discovery.Namespace` | message presence |  |
| 2 | `processes` | `newport.discovery.Namespace` | message presence |  |
| 3 | `boot_id` | `string` | optional |  |

### `newport.discovery.Endpoint`

Address is exactly four or sixteen network-order bytes. Interface is remote.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `address` | `bytes` | implicit |  |
| 2 | `port` | `uint32` | implicit |  |
| 3 | `interface` | `uint32` | optional |  |

### `newport.discovery.Cgroup`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `path` | `string` | optional |  |

### `newport.discovery.Association`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `service` | `uint32` | implicit |  |
| 2 | `socket_cgroup` | `newport.wire.Present` | oneof `relation` |  |
| 3 | `holder` | `uint32` | oneof `relation` |  |

### `newport.discovery.Socket`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint32` | implicit |  |
| 2 | `endpoint` | `newport.discovery.Endpoint` | message presence |  |
| 3 | `source` | `newport.discovery.SocketSource` | implicit |  |
| 4 | `inode` | `uint64` | optional |  |
| 5 | `cookie` | `uint64` | optional |  |
| 6 | `uid` | `uint32` | optional |  |
| 7 | `account` | `string` | optional |  |
| 8 | `ipv6_only` | `bool` | optional |  |
| 9 | `cgroup` | `newport.discovery.Cgroup` | message presence |  |
| 10 | `holders` | `uint32` | repeated |  |
| 11 | `holder_state` | `newport.discovery.HolderState` | implicit |  |
| 12 | `holders_truncated` | `bool` | implicit |  |
| 13 | `services` | `newport.discovery.Association` | repeated |  |

### `newport.discovery.ProcessIdentity`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `pid` | `uint32` | implicit |  |
| 2 | `start_ticks` | `uint64` | optional |  |

### `newport.discovery.ObservedPath`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `value` | `string` | implicit |  |
| 2 | `deleted` | `bool` | implicit |  |
| 3 | `lossy` | `bool` | implicit |  |

### `newport.discovery.Project`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `root` | `string` | implicit |  |
| 2 | `marker` | `string` | implicit |  |
| 3 | `worktree` | `string` | optional |  |
| 4 | `branch` | `string` | optional |  |
| 5 | `main` | `bool` | optional |  |

### `newport.discovery.Details`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `executable` | `newport.discovery.ObservedPath` | message presence |  |
| 2 | `directory` | `newport.discovery.ObservedPath` | message presence |  |
| 3 | `arguments` | `string` | repeated |  |
| 4 | `project` | `newport.discovery.Project` | message presence |  |

### `newport.discovery.Process`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint32` | implicit |  |
| 2 | `identity` | `newport.discovery.ProcessIdentity` | message presence |  |
| 3 | `name` | `string` | optional |  |
| 4 | `executable_name` | `string` | optional |  |
| 5 | `uid` | `uint32` | optional |  |
| 6 | `account` | `string` | optional |  |
| 7 | `details` | `newport.discovery.Details` | message presence |  |
| 8 | `metadata` | `newport.discovery.Report` | message presence |  |

### `newport.discovery.Manager`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `system` | `newport.wire.Present` | oneof `scope` |  |
| 2 | `user_uid` | `uint32` | oneof `scope` |  |

### `newport.discovery.Service`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint32` | implicit |  |
| 2 | `manager` | `newport.discovery.Manager` | message presence |  |
| 3 | `unit` | `string` | implicit |  |
| 4 | `description` | `string` | optional |  |
| 5 | `control_group` | `string` | implicit |  |
| 6 | `main_pid` | `uint32` | optional |  |
| 7 | `triggers` | `string` | repeated |  |

### `newport.discovery.Compose`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `project` | `string` | optional |  |
| 2 | `service` | `string` | optional |  |
| 3 | `directory` | `string` | optional |  |
| 4 | `directory_project` | `newport.discovery.Project` | message presence |  |

### `newport.discovery.Publication`

Publication is an engine observation, not proof of a host listening socket.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `engine` | `string` | optional | Unknown identity requires partial container coverage and cannot prove absence. |
| 2 | `container` | `string` | implicit |  |
| 3 | `name` | `string` | implicit |  |
| 4 | `image` | `string` | optional |  |
| 5 | `state` | `newport.discovery.ContainerState` | implicit |  |
| 6 | `host` | `newport.discovery.Endpoint` | message presence |  |
| 7 | `target_port` | `uint32` | implicit |  |
| 8 | `target_address` | `bytes` | optional |  |
| 9 | `compose` | `newport.discovery.Compose` | message presence |  |

### `newport.discovery.Issue`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `kind` | `newport.discovery.IssueKind` | implicit |  |
| 2 | `count` | `uint64` | implicit |  |
| 3 | `message` | `string` | implicit |  |

### `newport.discovery.Report`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `outcome` | `newport.discovery.Outcome` | implicit |  |
| 2 | `issues` | `newport.discovery.Issue` | repeated |  |

### `newport.discovery.Coverage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ipv4` | `newport.discovery.Report` | message presence |  |
| 2 | `ipv6` | `newport.discovery.Report` | message presence |  |
| 3 | `processes` | `newport.discovery.Report` | message presence |  |
| 4 | `cgroups` | `newport.discovery.Report` | message presence |  |
| 5 | `services` | `newport.discovery.Report` | message presence |  |
| 6 | `containers` | `newport.discovery.Report` | message presence |  |
| 7 | `elevation` | `newport.discovery.Report` | message presence |  |
| 8 | `details` | `newport.discovery.Report` | message presence |  |

### `newport.discovery.SocketSource`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `SOCKET_SOURCE_UNSPECIFIED` |  |
| 1 | `SOCKET_SOURCE_DIAGNOSTIC` |  |
| 2 | `SOCKET_SOURCE_PROC` |  |

### `newport.discovery.HolderState`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `HOLDER_STATE_UNSPECIFIED` |  |
| 1 | `HOLDER_STATE_UNRESOLVED` |  |
| 2 | `HOLDER_STATE_OBSERVED` |  |
| 3 | `HOLDER_STATE_CHANGED` |  |

### `newport.discovery.ContainerState`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CONTAINER_STATE_UNSPECIFIED` |  |
| 1 | `CONTAINER_STATE_RUNNING` |  |
| 2 | `CONTAINER_STATE_STOPPED` |  |
| 3 | `CONTAINER_STATE_UNKNOWN` |  |

### `newport.discovery.Outcome`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `OUTCOME_UNSPECIFIED` |  |
| 1 | `OUTCOME_NOT_ATTEMPTED` |  |
| 2 | `OUTCOME_COMPLETE` |  |
| 3 | `OUTCOME_PARTIAL` |  |
| 4 | `OUTCOME_UNAVAILABLE` |  |
| 5 | `OUTCOME_NOT_APPLICABLE` |  |

### `newport.discovery.IssueKind`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `ISSUE_KIND_UNSPECIFIED` |  |
| 1 | `ISSUE_KIND_PERMISSION_DENIED` |  |
| 2 | `ISSUE_KIND_UNSUPPORTED` |  |
| 3 | `ISSUE_KIND_TIMED_OUT` |  |
| 4 | `ISSUE_KIND_INTERRUPTED` |  |
| 5 | `ISSUE_KIND_BUDGET_EXCEEDED` |  |
| 6 | `ISSUE_KIND_IO` |  |
| 7 | `ISSUE_KIND_INVALID_RESPONSE` |  |
| 8 | `ISSUE_KIND_UNAVAILABLE` |  |

## Schema `newport/git.proto`

Source: [newport/git.proto](proto/newport/git.proto). Package: `newport.git`.

Imports: `newport/worker.proto`.

### `newport.git.Path`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `data` | `bytes` | optional |  |
| 2 | `display` | `string` | optional |  |

### `newport.git.StatusFilter`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `text` | `string` | optional |  |
| 2 | `group` | `string` | optional |  |

### `newport.git.Author`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `email` | `string` | optional |  |

### `newport.git.TagAnnotation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `message` | `string` | optional |  |
| 2 | `author` | `newport.git.Author` | optional |  |

### `newport.git.HunkSelection`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ids` | `string` | repeated |  |
| 2 | `lines` | `newport.git.StringList` | optional |  |
| 3 | `context_lines` | `uint32` | optional |  |

### `newport.git.CommandLog`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `command` | `string` | optional |  |
| 2 | `duration_ms` | `uint64` | optional |  |
| 3 | `exit_code` | `sint32` | optional |  |
| 4 | `output` | `string` | optional |  |
| 5 | `interrupted` | `bool` | optional |  |

### `newport.git.ObjectId`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `algorithm` | `string` | optional |  |
| 2 | `hex` | `string` | optional |  |

### `newport.git.TagObjectId`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `format` | `string` | optional |  |
| 2 | `hex` | `string` | optional |  |

### `newport.git.Head`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `oid` | `newport.git.ObjectId` | optional |  |
| 2 | `name` | `newport.git.Path` | optional |  |
| 3 | `detached` | `bool` | optional |  |
| 4 | `unborn` | `bool` | optional |  |

### `newport.git.IntegrationState`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `kind` | `string` | optional |  |
| 2 | `managed` | `bool` | optional |  |
| 3 | `can_continue` | `bool` | optional |  |
| 4 | `can_abort` | `bool` | optional |  |
| 5 | `can_skip` | `bool` | optional |  |

### `newport.git.RepositoryCapabilities`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `read_only` | `bool` | optional |  |
| 2 | `working_tree` | `bool` | optional |  |

### `newport.git.Repository`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `common_repo_id` | `string` | optional |  |
| 3 | `root` | `newport.git.Path` | optional |  |
| 4 | `bare` | `bool` | optional |  |
| 5 | `object_format` | `string` | optional |  |
| 6 | `head` | `newport.git.Head` | optional |  |
| 7 | `operation_state` | `string` | optional |  |
| 8 | `integration` | `newport.git.IntegrationState` | optional |  |
| 9 | `capabilities` | `newport.git.RepositoryCapabilities` | optional |  |

### `newport.git.GroupCounts`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `staged` | `uint64` | optional |  |
| 2 | `unstaged` | `uint64` | optional |  |
| 3 | `untracked` | `uint64` | optional |  |
| 4 | `conflicted` | `uint64` | optional |  |

### `newport.git.Upstream`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `remote` | `newport.git.Path` | optional |  |
| 2 | `reference` | `newport.git.Path` | optional |  |

### `newport.git.StatusSummary`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `head` | `newport.git.Head` | optional |  |
| 2 | `operation_state` | `string` | optional |  |
| 3 | `integration` | `newport.git.IntegrationState` | optional |  |
| 4 | `ahead` | `uint64` | optional |  |
| 5 | `behind` | `uint64` | optional |  |
| 6 | `basis` | `string` | optional |  |
| 7 | `upstream_ref` | `newport.git.Path` | optional |  |
| 8 | `upstream` | `newport.git.Upstream` | optional |  |
| 9 | `truncated` | `bool` | optional |  |
| 10 | `total_entries` | `uint64` | optional |  |
| 11 | `group_counts` | `newport.git.GroupCounts` | optional |  |
| 12 | `matched_entries` | `uint64` | optional |  |

### `newport.git.ConflictEntry`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `mode` | `uint32` | optional |  |
| 2 | `oid` | `newport.git.ObjectId` | optional |  |
| 3 | `path` | `newport.git.Path` | optional |  |

### `newport.git.Conflict`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `base` | `newport.git.ConflictEntry` | optional |  |
| 2 | `ours` | `newport.git.ConflictEntry` | optional |  |
| 3 | `theirs` | `newport.git.ConflictEntry` | optional |  |

### `newport.git.StatusEntry`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `entry_id` | `string` | optional |  |
| 2 | `path` | `newport.git.Path` | optional |  |
| 3 | `old_path` | `newport.git.Path` | optional |  |
| 4 | `flags` | `uint32` | optional |  |
| 5 | `staged` | `bool` | optional |  |
| 6 | `unstaged` | `bool` | optional |  |
| 7 | `untracked` | `bool` | optional |  |
| 8 | `conflicted` | `bool` | optional |  |
| 9 | `conflict` | `newport.git.Conflict` | optional |  |

### `newport.git.ConfigurationEntry`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `value` | `newport.git.Path` | optional |  |
| 2 | `level` | `string` | optional |  |
| 3 | `include_depth` | `uint64` | optional |  |

### `newport.git.TrackingConfiguration`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `remote` | `newport.git.ConfigurationEntry` | repeated |  |
| 2 | `merge` | `newport.git.ConfigurationEntry` | repeated |  |

### `newport.git.Tracking`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `token` | `string` | optional |  |
| 2 | `editable` | `bool` | optional |  |
| 3 | `configuration` | `newport.git.TrackingConfiguration` | optional |  |

### `newport.git.Branch`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `newport.git.Path` | optional |  |
| 2 | `reference` | `newport.git.Path` | optional |  |
| 3 | `oid` | `newport.git.ObjectId` | optional |  |
| 4 | `remote` | `bool` | optional |  |
| 5 | `current` | `bool` | optional |  |
| 6 | `upstream` | `newport.git.Path` | optional |  |
| 7 | `tracking` | `newport.git.Tracking` | optional |  |

### `newport.git.TotalEntries`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `total_entries` | `uint64` | optional |  |

### `newport.git.Worktree`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `newport.git.Path` | optional |  |
| 2 | `kind` | `string` | optional |  |
| 3 | `path` | `newport.git.Path` | optional |  |
| 4 | `git_dir` | `newport.git.Path` | optional |  |
| 5 | `current` | `bool` | optional |  |
| 6 | `state` | `string` | optional |  |
| 7 | `head` | `newport.git.Head` | optional |  |
| 8 | `locked` | `bool` | optional |  |
| 9 | `lock_reason` | `newport.git.Path` | optional |  |
| 10 | `prunable` | `bool` | optional |  |

### `newport.git.WorktreeMetadata`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `list_token` | `string` | optional |  |
| 2 | `total_entries` | `uint64` | optional |  |
| 3 | `matching_entries` | `uint64` | optional |  |
| 4 | `current` | `newport.git.Worktree` | optional |  |
| 5 | `main` | `newport.git.Worktree` | optional |  |

### `newport.git.Remote`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `url` | `string` | optional |  |
| 3 | `push_url` | `string` | optional |  |
| 4 | `token` | `string` | optional |  |

### `newport.git.RemoteAuthentication`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ssh` | `string` | optional |  |
| 2 | `https` | `string` | optional |  |

### `newport.git.RepositoryConfiguration`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `fetch_prune` | `bool` | optional |  |
| 2 | `user_name` | `string` | optional |  |
| 3 | `user_email` | `string` | optional |  |
| 4 | `token` | `string` | optional |  |

### `newport.git.Remotes`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `entries` | `newport.git.Remote` | repeated |  |
| 2 | `authentication` | `newport.git.RemoteAuthentication` | optional |  |
| 3 | `configuration` | `newport.git.RepositoryConfiguration` | optional |  |

### `newport.git.RemoteName`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |

### `newport.git.RemoteReference`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `reference` | `newport.git.Path` | optional |  |
| 2 | `kind` | `string` | optional |  |
| 3 | `oid` | `newport.git.ObjectId` | optional |  |
| 4 | `symbolic_target` | `newport.git.Path` | optional |  |

### `newport.git.RemoteReferenceMetadata`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `remote` | `string` | optional |  |
| 2 | `remote_token` | `string` | optional |  |
| 3 | `for_push` | `bool` | optional |  |
| 4 | `basis` | `string` | optional |  |
| 5 | `truncated` | `bool` | optional |  |
| 6 | `total_entries` | `uint64` | optional |  |

### `newport.git.Blob`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `oid` | `newport.git.ObjectId` | optional |  |
| 2 | `size` | `uint64` | optional |  |
| 3 | `bytes_b64` | `bytes` | optional |  |
| 4 | `truncated` | `bool` | optional |  |

### `newport.git.BlobFragment`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `offset` | `uint64` | optional |  |
| 2 | `bytes_b64` | `bytes` | optional |  |

### `newport.git.BlobMetadata`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `oid` | `newport.git.ObjectId` | optional |  |
| 2 | `size` | `uint64` | optional |  |

### `newport.git.Stash`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `index` | `uint64` | optional |  |
| 2 | `oid` | `string` | optional |  |
| 3 | `previous_oid` | `string` | optional |  |
| 4 | `message` | `string` | optional |  |
| 5 | `message_truncated` | `bool` | optional |  |
| 6 | `time` | `sint64` | optional |  |

### `newport.git.StashMetadata`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `list_token` | `string` | optional |  |
| 2 | `total_entries` | `uint64` | optional |  |

### `newport.git.Commit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `oid` | `newport.git.ObjectId` | optional |  |
| 2 | `parents` | `newport.git.ObjectId` | repeated |  |
| 3 | `message` | `newport.git.Path` | optional |  |
| 4 | `message_truncated` | `bool` | optional |  |
| 5 | `author` | `newport.git.Author` | optional |  |
| 6 | `time` | `sint64` | optional |  |
| 7 | `offset_minutes` | `sint32` | optional |  |

### `newport.git.HistoryMetadata`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `resolved_revision` | `newport.git.ObjectId` | optional |  |
| 2 | `truncated` | `bool` | optional |  |

### `newport.git.Tagger`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `email` | `string` | optional |  |
| 3 | `time` | `sint64` | optional |  |
| 4 | `offset_minutes` | `sint32` | optional |  |

### `newport.git.Tag`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `newport.git.Path` | optional |  |
| 2 | `reference` | `newport.git.Path` | optional |  |
| 3 | `symbolic_target` | `newport.git.Path` | optional |  |
| 4 | `oid` | `newport.git.TagObjectId` | optional |  |
| 5 | `annotated` | `bool` | optional |  |
| 6 | `details_omitted` | `bool` | optional |  |
| 7 | `object_type` | `string` | optional |  |
| 8 | `target_oid` | `newport.git.TagObjectId` | optional |  |
| 9 | `peeled_oid` | `newport.git.TagObjectId` | optional |  |
| 10 | `peeled_type` | `string` | optional |  |
| 11 | `message` | `newport.git.Path` | optional |  |
| 12 | `message_truncated` | `bool` | optional |  |
| 13 | `tagger` | `newport.git.Tagger` | optional |  |

### `newport.git.Delta`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `old_path` | `newport.git.Path` | optional |  |
| 2 | `new_path` | `newport.git.Path` | optional |  |
| 3 | `old_oid` | `newport.git.ObjectId` | optional |  |
| 4 | `new_oid` | `newport.git.ObjectId` | optional |  |
| 5 | `old_mode` | `uint32` | optional |  |
| 6 | `new_mode` | `uint32` | optional |  |
| 7 | `status` | `string` | optional |  |

### `newport.git.DiffMetadata`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `read_only` | `bool` | optional |  |
| 2 | `commit_oid` | `newport.git.ObjectId` | optional |  |
| 3 | `parent_oid` | `newport.git.ObjectId` | optional |  |
| 4 | `parent_index` | `uint64` | optional |  |
| 5 | `parents` | `newport.git.ObjectIdList` | optional |  |
| 6 | `source_snapshot` | `string` | optional |  |
| 7 | `entry_id` | `string` | optional |  |
| 8 | `side` | `newport.git.Side` | optional |  |
| 9 | `context_lines` | `uint32` | optional |  |
| 10 | `selected_path` | `newport.git.Path` | optional |  |
| 11 | `truncated` | `bool` | optional |  |
| 12 | `total_files` | `uint64` | optional |  |
| 13 | `total_units` | `uint64` | optional |  |
| 14 | `has_omissions` | `bool` | optional |  |

### `newport.git.DiffLine`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `origin` | `string` | optional |  |
| 3 | `old_line` | `uint64` | optional |  |
| 4 | `new_line` | `uint64` | optional |  |
| 5 | `content` | `newport.git.Path` | optional |  |

### `newport.git.DiffHunk`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `old_start` | `uint64` | optional |  |
| 3 | `old_lines` | `uint64` | optional |  |
| 4 | `new_start` | `uint64` | optional |  |
| 5 | `new_lines` | `uint64` | optional |  |
| 6 | `lines` | `newport.git.DiffLine` | repeated |  |

### `newport.git.DiffFile`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `old_path` | `newport.git.Path` | optional |  |
| 2 | `new_path` | `newport.git.Path` | optional |  |
| 3 | `old_oid` | `newport.git.ObjectId` | optional |  |
| 4 | `new_oid` | `newport.git.ObjectId` | optional |  |
| 5 | `old_mode` | `uint32` | optional |  |
| 6 | `new_mode` | `uint32` | optional |  |
| 7 | `status` | `string` | optional |  |
| 8 | `binary` | `bool` | optional |  |
| 9 | `additions` | `uint64` | optional |  |
| 10 | `deletions` | `uint64` | optional |  |
| 11 | `hunks` | `newport.git.DiffHunk` | repeated |  |

### `newport.git.Diff`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `files` | `newport.git.DiffFile` | repeated |  |
| 2 | `read_only` | `bool` | optional |  |
| 3 | `commit_oid` | `newport.git.ObjectId` | optional |  |
| 4 | `parent_oid` | `newport.git.ObjectId` | optional |  |
| 5 | `parent_index` | `uint64` | optional |  |
| 6 | `parents` | `newport.git.ObjectIdList` | optional |  |
| 7 | `source_snapshot` | `string` | optional |  |
| 8 | `entry_id` | `string` | optional |  |
| 9 | `side` | `newport.git.Side` | optional |  |
| 10 | `context_lines` | `uint32` | optional |  |
| 11 | `selected_path` | `newport.git.Path` | optional |  |
| 12 | `truncated` | `bool` | optional |  |
| 13 | `total_files` | `uint64` | optional |  |
| 14 | `total_units` | `uint64` | optional |  |
| 15 | `has_omissions` | `bool` | optional |  |

### `newport.git.LineFragment`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `line_index` | `uint64` | optional |  |
| 2 | `byte_offset` | `uint64` | optional |  |
| 3 | `line_complete` | `bool` | optional |  |
| 4 | `origin` | `string` | optional |  |
| 5 | `old_line` | `uint64` | optional |  |
| 6 | `new_line` | `uint64` | optional |  |
| 7 | `content_bytes_b64` | `bytes` | optional |  |
| 8 | `id` | `string` | optional |  |

### `newport.git.HunkFragment`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `index` | `uint64` | optional |  |
| 3 | `old_start` | `uint64` | optional |  |
| 4 | `old_lines` | `uint64` | optional |  |
| 5 | `new_start` | `uint64` | optional |  |
| 6 | `new_lines` | `uint64` | optional |  |
| 7 | `total_lines` | `uint64` | optional |  |
| 8 | `lines` | `newport.git.LineFragment` | repeated |  |

### `newport.git.FileFragment`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `old_path` | `newport.git.Path` | optional |  |
| 2 | `new_path` | `newport.git.Path` | optional |  |
| 3 | `old_oid` | `newport.git.ObjectId` | optional |  |
| 4 | `new_oid` | `newport.git.ObjectId` | optional |  |
| 5 | `old_mode` | `uint32` | optional |  |
| 6 | `new_mode` | `uint32` | optional |  |
| 7 | `status` | `string` | optional |  |
| 8 | `binary` | `bool` | optional |  |
| 9 | `additions` | `uint64` | optional |  |
| 10 | `deletions` | `uint64` | optional |  |
| 11 | `file_index` | `uint64` | optional |  |
| 12 | `omission_reason` | `string` | optional |  |
| 13 | `hunks` | `newport.git.HunkFragment` | repeated |  |

### `newport.git.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `operation_id` | `string` | optional |  |
| 2 | `repository` | `string` | optional |  |
| 3 | `payload_hash` | `string` | optional |  |
| 4 | `state` | `newport.git.OperationState` | optional |  |
| 5 | `seq` | `uint32` | optional |  |
| 6 | `result` | `newport.git.OperationResult` | optional |  |
| 7 | `error` | `newport.git.Failure` | optional |  |

### `newport.git.Closed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `closed` | `bool` | optional |  |

### `newport.git.StreamComplete`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `last_seq` | `uint32` | optional |  |
| 2 | `bytes` | `uint64` | optional |  |

### `newport.git.Refresh`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `refresh_required` | `bool` | optional |  |

### `newport.git.Committed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `merge_completed` | `bool` | optional |  |
| 2 | `commit_oid` | `string` | optional |  |
| 3 | `parent_oid` | `string` | optional |  |
| 4 | `refresh_required` | `bool` | optional |  |

### `newport.git.Amended`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `amended` | `bool` | optional |  |
| 2 | `replaced_oid` | `string` | optional |  |
| 3 | `commit_oid` | `string` | optional |  |
| 4 | `refresh_required` | `bool` | optional |  |

### `newport.git.TrackingChanged`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `tracking` | `newport.git.Tracking` | optional |  |
| 2 | `upstream` | `string` | optional |  |
| 3 | `refresh_required` | `bool` | optional |  |

### `newport.git.CheckedOut`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `detached` | `bool` | optional |  |
| 2 | `oid` | `string` | optional |  |
| 3 | `refresh_required` | `bool` | optional |  |

### `newport.git.RemoteRenamed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `renamed_references` | `uint64` | optional |  |
| 2 | `token` | `string` | optional |  |
| 3 | `refresh_required` | `bool` | optional |  |

### `newport.git.FastForwarded`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `fast_forwarded` | `bool` | optional |  |
| 2 | `oid` | `string` | optional |  |
| 3 | `refresh_required` | `bool` | optional |  |

### `newport.git.LeaseMatched`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `lease_matched` | `bool` | optional |  |
| 2 | `refresh_required` | `bool` | optional |  |

### `newport.git.RemoteDeleted`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `deleted` | `bool` | optional |  |
| 2 | `lease_matched` | `bool` | optional |  |
| 3 | `oid` | `string` | optional |  |
| 4 | `destination_tag` | `string` | optional |  |
| 5 | `refresh_required` | `bool` | optional |  |

### `newport.git.TagCreated`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `oid` | `string` | optional |  |
| 2 | `target_oid` | `string` | optional |  |
| 3 | `refresh_required` | `bool` | optional |  |

### `newport.git.Reset`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `previous_oid` | `string` | optional |  |
| 2 | `oid` | `string` | optional |  |
| 3 | `refresh_required` | `bool` | optional |  |

### `newport.git.Discarded`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `index_changed` | `bool` | optional |  |
| 2 | `refresh_required` | `bool` | optional |  |

### `newport.git.StashSaved`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `oid` | `string` | optional |  |
| 2 | `saved` | `bool` | optional |  |
| 3 | `refresh_required` | `bool` | optional |  |

### `newport.git.StashApplied`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `oid` | `string` | optional |  |
| 2 | `applied` | `bool` | optional |  |
| 3 | `dropped` | `bool` | optional |  |
| 4 | `needs_resolution` | `bool` | optional |  |
| 5 | `refresh_required` | `bool` | optional |  |

### `newport.git.StashDropped`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `oid` | `string` | optional |  |
| 2 | `dropped` | `bool` | optional |  |
| 3 | `refresh_required` | `bool` | optional |  |

### `newport.git.NeedsResolution`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `needs_resolution` | `bool` | optional |  |
| 2 | `refresh_required` | `bool` | optional |  |

### `newport.git.IntegrationCompleted`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `needs_resolution` | `bool` | optional |  |
| 2 | `aborted` | `bool` | optional |  |
| 3 | `merge_completed` | `bool` | optional |  |
| 4 | `integration_completed` | `string` | optional |  |
| 5 | `commit_oid` | `string` | optional |  |
| 6 | `oid` | `string` | optional |  |
| 7 | `refresh_required` | `bool` | optional |  |

### `newport.git.WorktreeAdded`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `path` | `newport.git.Path` | optional |  |
| 3 | `open_required` | `bool` | optional |  |
| 4 | `branch_created` | `bool` | optional |  |
| 5 | `refresh_required` | `bool` | optional |  |

### `newport.git.WorktreeRepaired`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `changed` | `bool` | optional |  |
| 2 | `refresh_required` | `bool` | optional |  |

### `newport.git.WorktreeRegistrationChanged`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `registration_only` | `bool` | optional |  |
| 2 | `refresh_required` | `bool` | optional |  |

### `newport.git.Initialized`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `initialized` | `bool` | optional |  |
| 2 | `path` | `newport.git.Path` | optional |  |
| 3 | `initial_branch` | `string` | optional |  |
| 4 | `bare` | `bool` | optional |  |
| 5 | `open_required` | `bool` | optional |  |

### `newport.git.Cloned`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `cloned` | `bool` | optional |  |
| 2 | `path` | `newport.git.Path` | optional |  |
| 3 | `bare` | `bool` | optional |  |
| 4 | `oid` | `string` | optional |  |
| 5 | `open_required` | `bool` | optional |  |

### `newport.git.Configure`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `client_id` | `string` | optional |  |
| 3 | `client_version` | `string` | optional |  |
| 4 | `command_logs` | `bool` | optional |  |

### `newport.git.Limits`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `max_frame_bytes` | `uint64` | optional |  |
| 2 | `max_chunk_bytes` | `uint64` | optional |  |
| 3 | `max_inflight` | `uint32` | optional |  |
| 4 | `max_page_items` | `uint32` | optional |  |
| 5 | `stream_window` | `uint32` | optional |  |
| 6 | `command_logs` | `bool` | optional |  |

### `newport.git.Capabilities`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `methods` | `string` | repeated |  |
| 2 | `object_formats` | `string` | repeated |  |
| 3 | `features` | `string` | repeated |  |
| 4 | `actions` | `string` | repeated |  |

### `newport.git.Ready`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `limits` | `newport.git.Limits` | optional |  |
| 3 | `capabilities` | `newport.git.Capabilities` | optional |  |

### `newport.git.Call`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `request` | `newport.git.Request` | optional |  |

### `newport.git.Begin`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `stream_id` | `string` | optional |  |
| 3 | `snapshot` | `string` | optional |  |

### `newport.git.Chunk`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `stream_id` | `string` | optional |  |
| 3 | `seq` | `uint32` | optional |  |
| 4 | `data` | `bytes` | optional |  |

### `newport.git.Ack`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `stream_id` | `string` | optional |  |
| 2 | `seq` | `uint32` | optional |  |

### `newport.git.Log`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `entry` | `newport.git.CommandLog` | optional |  |

### `newport.git.Failure`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `code` | `string` | optional |  |
| 2 | `message` | `string` | optional |  |
| 3 | `retry` | `string` | optional |  |

### `newport.git.RequestClone`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `operation_id` | `string` | optional |  |
| 2 | `url` | `string` | optional |  |
| 3 | `path` | `newport.git.Path` | optional |  |
| 4 | `branch` | `string` | optional |  |
| 5 | `bare` | `bool` | optional |  |

### `newport.git.RequestInit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `operation_id` | `string` | optional |  |
| 2 | `path` | `newport.git.Path` | optional |  |
| 3 | `initial_branch` | `string` | optional |  |

### `newport.git.RequestTag`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `oid` | `string` | optional |  |

### `newport.git.RequestTags`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `page_size` | `uint64` | optional |  |
| 3 | `cursor` | `string` | optional |  |
| 4 | `message_bytes` | `uint64` | optional |  |

### `newport.git.RequestStashes`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `page_size` | `uint64` | optional |  |
| 3 | `cursor` | `string` | optional |  |

### `newport.git.RequestBlobPage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `oid` | `string` | optional |  |
| 3 | `max_bytes` | `uint64` | optional |  |
| 4 | `cursor` | `string` | optional |  |

### `newport.git.RequestBlob`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `oid` | `string` | optional |  |

### `newport.git.RequestRemoteRefs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `remote` | `string` | optional |  |
| 3 | `expected_token` | `string` | optional |  |
| 4 | `for_push` | `bool` | optional |  |
| 5 | `filter` | `string` | optional |  |
| 6 | `page_size` | `uint64` | optional |  |
| 7 | `cursor` | `string` | optional |  |

### `newport.git.RequestRemoteNames`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `filter` | `string` | optional |  |
| 3 | `page_size` | `uint64` | optional |  |
| 4 | `cursor` | `string` | optional |  |

### `newport.git.RequestRemote`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `name` | `string` | optional |  |

### `newport.git.RequestRemotes`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |

### `newport.git.RequestStart`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `operation_id` | `string` | optional |  |
| 2 | `repo_id` | `string` | optional |  |
| 3 | `expected_snapshot` | `string` | optional |  |
| 4 | `action` | `newport.git.Action` | optional |  |

### `newport.git.RequestGet`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `operation_id` | `string` | optional |  |

### `newport.git.RequestReview`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `operation_id` | `string` | optional |  |

### `newport.git.RequestOpen`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `path` | `newport.git.Path` | optional |  |

### `newport.git.RequestClose`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |

### `newport.git.RequestStatusSummary`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `path` | `newport.git.Path` | optional |  |

### `newport.git.RequestStatus`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `filter` | `newport.git.StatusFilter` | optional |  |
| 3 | `page_size` | `uint64` | optional |  |
| 4 | `cursor` | `string` | optional |  |

### `newport.git.RequestWorktrees`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `page_size` | `uint64` | optional |  |
| 3 | `cursor` | `string` | optional |  |
| 4 | `filter` | `string` | optional |  |
| 5 | `branch` | `string` | optional |  |
| 6 | `name` | `string` | optional |  |
| 7 | `at_snapshot` | `string` | optional |  |

### `newport.git.RequestBranches`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `page_size` | `uint64` | optional |  |
| 3 | `cursor` | `string` | optional |  |
| 4 | `filter` | `string` | optional |  |
| 5 | `branch_kind` | `string` | optional |  |

### `newport.git.RequestHistory`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `page_size` | `uint64` | optional |  |
| 3 | `cursor` | `string` | optional |  |
| 4 | `revision` | `string` | optional |  |
| 5 | `message_bytes` | `uint64` | optional |  |

### `newport.git.RequestCommit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `commit_oid` | `string` | optional |  |

### `newport.git.RequestCommitFiles`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `commit_oid` | `string` | optional |  |
| 3 | `parent_index` | `uint64` | optional |  |
| 4 | `page_size` | `uint64` | optional |  |
| 5 | `cursor` | `string` | optional |  |

### `newport.git.RequestCommitDiff`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `path` | `newport.git.Path` | optional |  |
| 2 | `repo_id` | `string` | optional |  |
| 3 | `commit_oid` | `string` | optional |  |
| 4 | `parent_index` | `uint64` | optional |  |
| 5 | `context_lines` | `uint32` | optional |  |

### `newport.git.RequestCommitDiffPage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `commit_oid` | `string` | optional |  |
| 3 | `path` | `newport.git.Path` | optional |  |
| 4 | `parent_index` | `uint64` | optional |  |
| 5 | `context_lines` | `uint32` | optional |  |
| 6 | `page_size` | `uint64` | optional |  |
| 7 | `max_bytes` | `uint64` | optional |  |
| 8 | `cursor` | `string` | optional |  |
| 9 | `line_encoding` | `newport.git.DiffLineEncoding` | optional |  |

### `newport.git.RequestDiffPage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `snapshot` | `string` | optional |  |
| 3 | `entry_id` | `string` | optional |  |
| 4 | `side` | `newport.git.Side` | optional |  |
| 5 | `context_lines` | `uint32` | optional |  |
| 6 | `page_size` | `uint64` | optional |  |
| 7 | `max_bytes` | `uint64` | optional |  |
| 8 | `cursor` | `string` | optional |  |
| 9 | `line_encoding` | `newport.git.DiffLineEncoding` | optional |  |

### `newport.git.RequestDiff`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `repo_id` | `string` | optional |  |
| 2 | `snapshot` | `string` | optional |  |
| 3 | `entry_id` | `string` | optional |  |
| 4 | `side` | `newport.git.Side` | optional |  |
| 5 | `context_lines` | `uint32` | optional |  |

### `newport.git.ActionConflictResolve`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `entry_ids` | `string` | repeated |  |
| 2 | `side` | `newport.git.ConflictSide` | optional |  |
| 3 | `expected_oid` | `string` | optional |  |

### `newport.git.ActionDiscard`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `entry_ids` | `string` | repeated |  |
| 2 | `source` | `newport.git.DiscardSource` | optional |  |
| 3 | `hunks` | `newport.git.HunkSelection` | optional |  |

### `newport.git.ActionReset`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `target_oid` | `string` | optional |  |
| 2 | `expected_oid` | `string` | optional |  |
| 3 | `mode` | `newport.git.ResetMode` | optional |  |

### `newport.git.ActionRebase`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `upstream_oid` | `string` | optional |  |
| 2 | `onto_oid` | `string` | optional |  |
| 3 | `committer` | `newport.git.Author` | optional |  |

### `newport.git.ActionWorktreeRepair`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `path` | `newport.git.Path` | optional |  |

### `newport.git.ActionWorktreeRemove`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |

### `newport.git.ActionWorktreePrune`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |

### `newport.git.ActionWorktreeAdd`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `path` | `newport.git.Path` | optional |  |
| 3 | `branch` | `string` | optional |  |
| 4 | `expected_oid` | `string` | optional |  |
| 5 | `locked` | `bool` | optional |  |
| 6 | `new_branch` | `bool` | optional |  |

### `newport.git.ActionWorktreeLock`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `reason` | `string` | optional |  |

### `newport.git.ActionWorktreeUnlock`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |

### `newport.git.ActionIntegrationSkip`

Empty message.

### `newport.git.ActionBranchSetUpstream`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `expected_oid` | `string` | optional |  |
| 3 | `expected_token` | `string` | optional |  |
| 4 | `upstream` | `string` | optional |  |

### `newport.git.ActionCherryPick`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `target_oid` | `string` | optional |  |
| 2 | `mainline` | `uint32` | optional |  |
| 3 | `author` | `newport.git.Author` | optional |  |

### `newport.git.ActionRevert`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `target_oid` | `string` | optional |  |
| 2 | `mainline` | `uint32` | optional |  |
| 3 | `author` | `newport.git.Author` | optional |  |

### `newport.git.ActionIntegrationContinue`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `message` | `string` | optional |  |
| 2 | `author` | `newport.git.Author` | optional |  |

### `newport.git.ActionIntegrationAbort`

Empty message.

### `newport.git.ActionTagDeleteRemote`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `remote` | `string` | optional |  |
| 2 | `expected_token` | `string` | optional |  |
| 3 | `name` | `string` | optional |  |
| 4 | `expected_oid` | `string` | optional |  |

### `newport.git.ActionTagPush`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `remote` | `string` | optional |  |
| 2 | `expected_token` | `string` | optional |  |
| 3 | `name` | `string` | optional |  |
| 4 | `expected_oid` | `string` | optional |  |

### `newport.git.ActionTagCreate`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `target_oid` | `string` | optional |  |
| 3 | `annotation` | `newport.git.TagAnnotation` | optional |  |

### `newport.git.ActionTagDelete`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `expected_oid` | `string` | optional |  |

### `newport.git.ActionStashSave`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `message` | `string` | optional |  |
| 2 | `include_untracked` | `bool` | optional |  |
| 3 | `keep_index` | `bool` | optional |  |
| 4 | `author` | `newport.git.Author` | optional |  |

### `newport.git.ActionStashApply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `oid` | `string` | optional |  |
| 2 | `index` | `uint64` | optional |  |
| 3 | `expected_token` | `string` | optional |  |
| 4 | `reinstate_index` | `bool` | optional |  |

### `newport.git.ActionStashPop`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `oid` | `string` | optional |  |
| 2 | `index` | `uint64` | optional |  |
| 3 | `expected_token` | `string` | optional |  |
| 4 | `reinstate_index` | `bool` | optional |  |

### `newport.git.ActionStashDrop`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `oid` | `string` | optional |  |
| 2 | `index` | `uint64` | optional |  |
| 3 | `expected_token` | `string` | optional |  |

### `newport.git.ActionMergeAbort`

Empty message.

### `newport.git.ActionMerge`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `target_oid` | `string` | optional |  |

### `newport.git.ActionPullFastForward`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `remote` | `string` | optional |  |
| 2 | `expected_token` | `string` | optional |  |
| 3 | `remote_branch` | `string` | optional |  |

### `newport.git.ActionFastForward`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `target_oid` | `string` | optional |  |

### `newport.git.ActionRemoteAdd`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `url` | `string` | optional |  |

### `newport.git.ActionRemoteRename`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `new_name` | `string` | optional |  |
| 3 | `expected_token` | `string` | optional |  |

### `newport.git.ActionRemoteSetUrl`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `url` | `string` | optional |  |
| 3 | `expected_token` | `string` | optional |  |

### `newport.git.ActionRemoteRemove`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `expected_token` | `string` | optional |  |

### `newport.git.ActionConfigSet`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `expected_token` | `string` | optional |  |
| 2 | `fetch_prune` | `bool` | optional |  |
| 3 | `user_name` | `string` | optional |  |
| 4 | `user_email` | `string` | optional |  |

### `newport.git.ActionFetch`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `remote` | `string` | optional |  |
| 2 | `expected_token` | `string` | optional |  |
| 3 | `prune` | `bool` | optional |  |

### `newport.git.ActionBranchDeleteRemote`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `remote` | `string` | optional |  |
| 2 | `expected_token` | `string` | optional |  |
| 3 | `branch` | `string` | optional |  |
| 4 | `expected_oid` | `string` | optional |  |

### `newport.git.ActionPushWithLease`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `remote` | `string` | optional |  |
| 2 | `expected_token` | `string` | optional |  |
| 3 | `branch` | `string` | optional |  |
| 4 | `expected_oid` | `string` | optional |  |
| 5 | `destination_branch` | `string` | optional |  |
| 6 | `expected_remote_oid` | `string` | optional |  |

### `newport.git.ActionPush`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `remote` | `string` | optional |  |
| 2 | `expected_token` | `string` | optional |  |
| 3 | `branch` | `string` | optional |  |
| 4 | `expected_oid` | `string` | optional |  |
| 5 | `destination_branch` | `string` | optional |  |

### `newport.git.ActionCheckout`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `target` | `newport.git.CheckoutTarget` | optional |  |

### `newport.git.ActionBranchCreate`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `start_oid` | `string` | optional |  |

### `newport.git.ActionBranchRename`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `new_name` | `string` | optional |  |
| 3 | `expected_oid` | `string` | optional |  |

### `newport.git.ActionBranchDelete`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `expected_oid` | `string` | optional |  |
| 3 | `force` | `bool` | optional |  |

### `newport.git.ActionAmend`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `expected_oid` | `string` | optional |  |
| 2 | `message` | `string` | optional |  |
| 3 | `committer` | `newport.git.Author` | optional |  |
| 4 | `author` | `newport.git.Author` | optional |  |

### `newport.git.ActionCommit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `message` | `string` | optional |  |
| 2 | `author` | `newport.git.Author` | optional |  |

### `newport.git.ActionStage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `entry_ids` | `string` | repeated |  |
| 2 | `hunks` | `newport.git.HunkSelection` | optional |  |

### `newport.git.ActionUnstage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `entry_ids` | `string` | repeated |  |
| 2 | `hunks` | `newport.git.HunkSelection` | optional |  |

### `newport.git.CheckoutTargetBranch`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | optional |  |
| 2 | `expected_oid` | `string` | optional |  |

### `newport.git.CheckoutTargetDetached`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `oid` | `string` | optional |  |

### `newport.git.StatusEntryPage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `string` | optional |  |
| 2 | `entries` | `newport.git.StatusEntry` | repeated |  |
| 3 | `next_cursor` | `string` | optional |  |
| 4 | `metadata` | `newport.git.StatusSummary` | optional |  |

### `newport.git.BranchPage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `string` | optional |  |
| 2 | `entries` | `newport.git.Branch` | repeated |  |
| 3 | `next_cursor` | `string` | optional |  |
| 4 | `metadata` | `newport.git.TotalEntries` | optional |  |

### `newport.git.WorktreePage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `string` | optional |  |
| 2 | `entries` | `newport.git.Worktree` | repeated |  |
| 3 | `next_cursor` | `string` | optional |  |
| 4 | `metadata` | `newport.git.WorktreeMetadata` | optional |  |

### `newport.git.RemoteNamePage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `string` | optional |  |
| 2 | `entries` | `newport.git.RemoteName` | repeated |  |
| 3 | `next_cursor` | `string` | optional |  |
| 4 | `metadata` | `newport.git.TotalEntries` | optional |  |

### `newport.git.RemoteReferencePage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `string` | optional |  |
| 2 | `entries` | `newport.git.RemoteReference` | repeated |  |
| 3 | `next_cursor` | `string` | optional |  |
| 4 | `metadata` | `newport.git.RemoteReferenceMetadata` | optional |  |

### `newport.git.BlobFragmentPage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `string` | optional |  |
| 2 | `entries` | `newport.git.BlobFragment` | repeated |  |
| 3 | `next_cursor` | `string` | optional |  |
| 4 | `metadata` | `newport.git.BlobMetadata` | optional |  |

### `newport.git.StashPage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `string` | optional |  |
| 2 | `entries` | `newport.git.Stash` | repeated |  |
| 3 | `next_cursor` | `string` | optional |  |
| 4 | `metadata` | `newport.git.StashMetadata` | optional |  |

### `newport.git.TagPage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `string` | optional |  |
| 2 | `entries` | `newport.git.Tag` | repeated |  |
| 3 | `next_cursor` | `string` | optional |  |
| 4 | `metadata` | `newport.git.TotalEntries` | optional |  |

### `newport.git.CommitPage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `string` | optional |  |
| 2 | `entries` | `newport.git.Commit` | repeated |  |
| 3 | `next_cursor` | `string` | optional |  |
| 4 | `metadata` | `newport.git.HistoryMetadata` | optional |  |

### `newport.git.DeltaPage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `string` | optional |  |
| 2 | `entries` | `newport.git.Delta` | repeated |  |
| 3 | `next_cursor` | `string` | optional |  |
| 4 | `metadata` | `newport.git.DiffMetadata` | optional |  |

### `newport.git.FileFragmentPage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `string` | optional |  |
| 2 | `entries` | `newport.git.FileFragment` | repeated |  |
| 3 | `next_cursor` | `string` | optional |  |
| 4 | `metadata` | `newport.git.DiffMetadata` | optional |  |

### `newport.git.StringList`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `items` | `string` | repeated |  |

### `newport.git.ObjectIdList`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `items` | `newport.git.ObjectId` | repeated |  |

### `newport.git.Request`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `clone` | `newport.git.RequestClone` | oneof `value` |  |
| 2 | `init` | `newport.git.RequestInit` | oneof `value` |  |
| 3 | `tag` | `newport.git.RequestTag` | oneof `value` |  |
| 4 | `tags` | `newport.git.RequestTags` | oneof `value` |  |
| 5 | `stashes` | `newport.git.RequestStashes` | oneof `value` |  |
| 6 | `blob_page` | `newport.git.RequestBlobPage` | oneof `value` |  |
| 7 | `blob` | `newport.git.RequestBlob` | oneof `value` |  |
| 8 | `remote_refs` | `newport.git.RequestRemoteRefs` | oneof `value` |  |
| 9 | `remote_names` | `newport.git.RequestRemoteNames` | oneof `value` |  |
| 10 | `remote` | `newport.git.RequestRemote` | oneof `value` |  |
| 11 | `remotes` | `newport.git.RequestRemotes` | oneof `value` |  |
| 12 | `start` | `newport.git.RequestStart` | oneof `value` |  |
| 13 | `get` | `newport.git.RequestGet` | oneof `value` |  |
| 14 | `review` | `newport.git.RequestReview` | oneof `value` |  |
| 15 | `open` | `newport.git.RequestOpen` | oneof `value` |  |
| 16 | `close` | `newport.git.RequestClose` | oneof `value` |  |
| 17 | `status_summary` | `newport.git.RequestStatusSummary` | oneof `value` |  |
| 18 | `status` | `newport.git.RequestStatus` | oneof `value` |  |
| 19 | `worktrees` | `newport.git.RequestWorktrees` | oneof `value` |  |
| 20 | `branches` | `newport.git.RequestBranches` | oneof `value` |  |
| 21 | `history` | `newport.git.RequestHistory` | oneof `value` |  |
| 22 | `commit` | `newport.git.RequestCommit` | oneof `value` |  |
| 23 | `commit_files` | `newport.git.RequestCommitFiles` | oneof `value` |  |
| 24 | `commit_diff` | `newport.git.RequestCommitDiff` | oneof `value` |  |
| 25 | `commit_diff_page` | `newport.git.RequestCommitDiffPage` | oneof `value` |  |
| 26 | `diff_page` | `newport.git.RequestDiffPage` | oneof `value` |  |
| 27 | `diff` | `newport.git.RequestDiff` | oneof `value` |  |

### `newport.git.Action`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `conflict_resolve` | `newport.git.ActionConflictResolve` | oneof `value` |  |
| 2 | `discard` | `newport.git.ActionDiscard` | oneof `value` |  |
| 3 | `reset` | `newport.git.ActionReset` | oneof `value` |  |
| 4 | `rebase` | `newport.git.ActionRebase` | oneof `value` |  |
| 5 | `worktree_repair` | `newport.git.ActionWorktreeRepair` | oneof `value` |  |
| 6 | `worktree_remove` | `newport.git.ActionWorktreeRemove` | oneof `value` |  |
| 7 | `worktree_prune` | `newport.git.ActionWorktreePrune` | oneof `value` |  |
| 8 | `worktree_add` | `newport.git.ActionWorktreeAdd` | oneof `value` |  |
| 9 | `worktree_lock` | `newport.git.ActionWorktreeLock` | oneof `value` |  |
| 10 | `worktree_unlock` | `newport.git.ActionWorktreeUnlock` | oneof `value` |  |
| 11 | `integration_skip` | `newport.git.ActionIntegrationSkip` | oneof `value` |  |
| 12 | `branch_set_upstream` | `newport.git.ActionBranchSetUpstream` | oneof `value` |  |
| 13 | `cherry_pick` | `newport.git.ActionCherryPick` | oneof `value` |  |
| 14 | `revert` | `newport.git.ActionRevert` | oneof `value` |  |
| 15 | `integration_continue` | `newport.git.ActionIntegrationContinue` | oneof `value` |  |
| 16 | `integration_abort` | `newport.git.ActionIntegrationAbort` | oneof `value` |  |
| 17 | `tag_delete_remote` | `newport.git.ActionTagDeleteRemote` | oneof `value` |  |
| 18 | `tag_push` | `newport.git.ActionTagPush` | oneof `value` |  |
| 19 | `tag_create` | `newport.git.ActionTagCreate` | oneof `value` |  |
| 20 | `tag_delete` | `newport.git.ActionTagDelete` | oneof `value` |  |
| 21 | `stash_save` | `newport.git.ActionStashSave` | oneof `value` |  |
| 22 | `stash_apply` | `newport.git.ActionStashApply` | oneof `value` |  |
| 23 | `stash_pop` | `newport.git.ActionStashPop` | oneof `value` |  |
| 24 | `stash_drop` | `newport.git.ActionStashDrop` | oneof `value` |  |
| 25 | `merge_abort` | `newport.git.ActionMergeAbort` | oneof `value` |  |
| 26 | `merge` | `newport.git.ActionMerge` | oneof `value` |  |
| 27 | `pull_fast_forward` | `newport.git.ActionPullFastForward` | oneof `value` |  |
| 28 | `fast_forward` | `newport.git.ActionFastForward` | oneof `value` |  |
| 29 | `remote_add` | `newport.git.ActionRemoteAdd` | oneof `value` |  |
| 30 | `remote_rename` | `newport.git.ActionRemoteRename` | oneof `value` |  |
| 31 | `remote_set_url` | `newport.git.ActionRemoteSetUrl` | oneof `value` |  |
| 32 | `remote_remove` | `newport.git.ActionRemoteRemove` | oneof `value` |  |
| 33 | `config_set` | `newport.git.ActionConfigSet` | oneof `value` |  |
| 34 | `fetch` | `newport.git.ActionFetch` | oneof `value` |  |
| 35 | `branch_delete_remote` | `newport.git.ActionBranchDeleteRemote` | oneof `value` |  |
| 36 | `push_with_lease` | `newport.git.ActionPushWithLease` | oneof `value` |  |
| 37 | `push` | `newport.git.ActionPush` | oneof `value` |  |
| 38 | `checkout` | `newport.git.ActionCheckout` | oneof `value` |  |
| 39 | `branch_create` | `newport.git.ActionBranchCreate` | oneof `value` |  |
| 40 | `branch_rename` | `newport.git.ActionBranchRename` | oneof `value` |  |
| 41 | `branch_delete` | `newport.git.ActionBranchDelete` | oneof `value` |  |
| 42 | `amend` | `newport.git.ActionAmend` | oneof `value` |  |
| 43 | `commit` | `newport.git.ActionCommit` | oneof `value` |  |
| 44 | `stage` | `newport.git.ActionStage` | oneof `value` |  |
| 45 | `unstage` | `newport.git.ActionUnstage` | oneof `value` |  |

### `newport.git.CheckoutTarget`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `branch` | `newport.git.CheckoutTargetBranch` | oneof `value` |  |
| 2 | `detached` | `newport.git.CheckoutTargetDetached` | oneof `value` |  |

### `newport.git.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `open` | `newport.git.Repository` | oneof `value` |  |
| 2 | `close` | `newport.git.Closed` | oneof `value` |  |
| 3 | `status_summary` | `newport.git.StatusSummary` | oneof `value` |  |
| 4 | `status` | `newport.git.StatusEntryPage` | oneof `value` |  |
| 5 | `branches` | `newport.git.BranchPage` | oneof `value` |  |
| 6 | `worktrees` | `newport.git.WorktreePage` | oneof `value` |  |
| 7 | `remotes` | `newport.git.Remotes` | oneof `value` |  |
| 8 | `remote` | `newport.git.Remote` | oneof `value` |  |
| 9 | `remote_names` | `newport.git.RemoteNamePage` | oneof `value` |  |
| 10 | `remote_refs` | `newport.git.RemoteReferencePage` | oneof `value` |  |
| 11 | `blob` | `newport.git.Blob` | oneof `value` |  |
| 12 | `blob_page` | `newport.git.BlobFragmentPage` | oneof `value` |  |
| 13 | `stashes` | `newport.git.StashPage` | oneof `value` |  |
| 14 | `tags` | `newport.git.TagPage` | oneof `value` |  |
| 15 | `tag` | `newport.git.Tag` | oneof `value` |  |
| 16 | `history` | `newport.git.CommitPage` | oneof `value` |  |
| 17 | `commit` | `newport.git.Commit` | oneof `value` |  |
| 18 | `commit_files` | `newport.git.DeltaPage` | oneof `value` |  |
| 19 | `diff_page` | `newport.git.FileFragmentPage` | oneof `value` |  |
| 20 | `commit_diff_page` | `newport.git.FileFragmentPage` | oneof `value` |  |
| 21 | `operation` | `newport.git.Operation` | oneof `value` |  |
| 22 | `stream_complete` | `newport.git.StreamComplete` | oneof `value` |  |

### `newport.git.OperationResult`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `refresh` | `newport.git.Refresh` | oneof `value` |  |
| 2 | `committed` | `newport.git.Committed` | oneof `value` |  |
| 3 | `amended` | `newport.git.Amended` | oneof `value` |  |
| 4 | `tracking_changed` | `newport.git.TrackingChanged` | oneof `value` |  |
| 5 | `checked_out` | `newport.git.CheckedOut` | oneof `value` |  |
| 6 | `remote_renamed` | `newport.git.RemoteRenamed` | oneof `value` |  |
| 7 | `fast_forwarded` | `newport.git.FastForwarded` | oneof `value` |  |
| 8 | `lease_matched` | `newport.git.LeaseMatched` | oneof `value` |  |
| 9 | `remote_deleted` | `newport.git.RemoteDeleted` | oneof `value` |  |
| 10 | `tag_created` | `newport.git.TagCreated` | oneof `value` |  |
| 11 | `reset` | `newport.git.Reset` | oneof `value` |  |
| 12 | `discarded` | `newport.git.Discarded` | oneof `value` |  |
| 13 | `stash_saved` | `newport.git.StashSaved` | oneof `value` |  |
| 14 | `stash_applied` | `newport.git.StashApplied` | oneof `value` |  |
| 15 | `stash_dropped` | `newport.git.StashDropped` | oneof `value` |  |
| 16 | `needs_resolution` | `newport.git.NeedsResolution` | oneof `value` |  |
| 17 | `integration_completed` | `newport.git.IntegrationCompleted` | oneof `value` |  |
| 18 | `worktree_added` | `newport.git.WorktreeAdded` | oneof `value` |  |
| 19 | `worktree_repaired` | `newport.git.WorktreeRepaired` | oneof `value` |  |
| 20 | `worktree_registration_changed` | `newport.git.WorktreeRegistrationChanged` | oneof `value` |  |
| 21 | `initialized` | `newport.git.Initialized` | oneof `value` |  |
| 22 | `cloned` | `newport.git.Cloned` | oneof `value` |  |

### `newport.git.Response`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | optional |  |
| 2 | `reply` | `newport.git.Reply` | oneof `result` |  |
| 3 | `error` | `newport.git.Failure` | oneof `result` |  |

### `newport.git.ClientFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `configure` | `newport.git.Configure` | oneof `message` |  |
| 2 | `call` | `newport.git.Call` | oneof `message` |  |
| 3 | `ping` | `string` | oneof `message` |  |
| 4 | `ack` | `newport.git.Ack` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ClientControl` | oneof `message` |  |

### `newport.git.ServerFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 17 | `ready` | `newport.git.Ready` | oneof `message` |  |
| 18 | `response` | `newport.git.Response` | oneof `message` |  |
| 19 | `pong` | `string` | oneof `message` |  |
| 20 | `begin` | `newport.git.Begin` | oneof `message` |  |
| 21 | `chunk` | `newport.git.Chunk` | oneof `message` |  |
| 22 | `log` | `newport.git.Log` | oneof `message` |  |
| 23 | `failed` | `newport.git.Failure` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ServerControl` | oneof `message` |  |

### `newport.git.Side`

Runtime admission selects this service; configure binds its durable client identity.

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `SIDE_UNSPECIFIED` |  |
| 1 | `SIDE_HEAD_TO_INDEX` |  |
| 2 | `SIDE_INDEX_TO_WORKTREE` |  |
| 3 | `SIDE_HEAD_TO_WORKTREE` |  |

### `newport.git.ResetMode`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `RESET_MODE_UNSPECIFIED` |  |
| 1 | `RESET_MODE_SOFT` |  |
| 2 | `RESET_MODE_MIXED` |  |
| 3 | `RESET_MODE_HARD` |  |

### `newport.git.DiscardSource`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `DISCARD_SOURCE_UNSPECIFIED` |  |
| 1 | `DISCARD_SOURCE_INDEX` |  |
| 2 | `DISCARD_SOURCE_HEAD` |  |

### `newport.git.ConflictSide`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CONFLICT_SIDE_UNSPECIFIED` |  |
| 1 | `CONFLICT_SIDE_BASE` |  |
| 2 | `CONFLICT_SIDE_OURS` |  |
| 3 | `CONFLICT_SIDE_THEIRS` |  |

### `newport.git.DiffLineEncoding`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `DIFF_LINE_ENCODING_UNSPECIFIED` |  |
| 1 | `DIFF_LINE_ENCODING_TUPLE_V1` |  |

### `newport.git.OperationState`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `OPERATION_STATE_UNSPECIFIED` |  |
| 1 | `OPERATION_STATE_RUNNING` |  |
| 2 | `OPERATION_STATE_SUCCEEDED` |  |
| 3 | `OPERATION_STATE_FAILED` |  |
| 4 | `OPERATION_STATE_NEEDS_RESOLUTION` |  |
| 5 | `OPERATION_STATE_OUTCOME_UNKNOWN` |  |
| 6 | `OPERATION_STATE_REVIEWED_UNKNOWN` |  |

## Schema `newport/integration.proto`

Source: [newport/integration.proto](proto/newport/integration.proto). Package: `newport.integration`.

Imports: `newport/wire.proto`, `newport/worker.proto`, `newport/runtime.proto`, `newport/clipboard.proto`.

### `newport.integration.ClipboardOffer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `int64` | implicit |  |
| 2 | `formats` | `string` | repeated |  |

### `newport.integration.ClipboardRequest`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `revision` | `int64` | implicit |  |
| 3 | `format` | `string` | implicit |  |

### `newport.integration.ClipboardReply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `revision` | `int64` | implicit |  |
| 3 | `encoding` | `newport.integration.Encoding` | implicit |  |
| 4 | `done` | `bool` | implicit |  |
| 5 | `data` | `bytes` | implicit |  |

### `newport.integration.BrowserRequest`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `url` | `string` | implicit |  |

### `newport.integration.BrowserReply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `success` | `bool` | implicit |  |
| 3 | `message` | `string` | optional |  |

### `newport.integration.Snapshot`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `archive` | `bytes` | implicit |  |

### `newport.integration.Acknowledged`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `message` | `string` | implicit |  |

### `newport.integration.Call`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `permissions` | `newport.runtime.Permissions` | oneof `request` |  |
| 3 | `clipboard_offer` | `newport.integration.ClipboardOffer` | oneof `request` |  |
| 4 | `snapshot` | `newport.integration.Snapshot` | oneof `request` |  |
| 5 | `gui_notices` | `newport.wire.Present` | oneof `request` |  |
| 6 | `heartbeat` | `newport.wire.Present` | oneof `request` |  |
| 7 | `export` | `newport.clipboard.ExportControl` | oneof `request` |  |

### `newport.integration.Response`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `acknowledged` | `newport.integration.Acknowledged` | oneof `reply` |  |
| 3 | `permissions` | `newport.runtime.Applied` | oneof `reply` |  |
| 4 | `error` | `newport.wire.Error` | oneof `reply` |  |

### `newport.integration.ClientFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `call` | `newport.integration.Call` | oneof `message` |  |
| 2 | `clipboard` | `newport.integration.ClipboardReply` | oneof `message` |  |
| 3 | `browser` | `newport.integration.BrowserReply` | oneof `message` |  |
| 4 | `quit` | `newport.wire.Present` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ClientControl` | oneof `message` |  |

### `newport.integration.ServerFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `response` | `newport.integration.Response` | oneof `message` |  |
| 2 | `clipboard_request` | `newport.integration.ClipboardRequest` | oneof `message` |  |
| 3 | `browser` | `newport.integration.BrowserRequest` | oneof `message` |  |
| 4 | `gui_notice` | `newport.wire.Present` | oneof `message` |  |
| 5 | `export` | `newport.clipboard.ExportEvent` | oneof `message` |  |
| 6 | `error` | `newport.wire.Error` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ServerControl` | oneof `message` |  |

### `newport.integration.Encoding`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `ENCODING_RAW` |  |
| 1 | `ENCODING_FAILED` |  |
| 2 | `ENCODING_ZLIB` |  |

## Schema `newport/monitor.proto`

Source: [newport/monitor.proto](proto/newport/monitor.proto). Package: `newport.monitor`.

Imports: `newport/wire.proto`, `newport/worker.proto`, `newport/discovery.proto`.

### `newport.monitor.StringList`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `values` | `string` | repeated |  |

### `newport.monitor.Label`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `key` | `string` | implicit |  |
| 2 | `value` | `string` | implicit |  |

### `newport.monitor.MetricsSnapshot`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `hostname` | `string` | implicit |  |
| 2 | `os` | `string` | implicit |  |
| 3 | `kernel` | `string` | implicit |  |
| 4 | `cores` | `uint32` | implicit |  |
| 5 | `uptime` | `double` | implicit |  |
| 6 | `load` | `double` | repeated |  |
| 7 | `cpu` | `double` | implicit |  |
| 8 | `cpus` | `newport.monitor.MetricsCore` | repeated |  |
| 9 | `memory_total` | `uint64` | implicit |  |
| 10 | `memory_used` | `uint64` | implicit |  |
| 11 | `swap_total` | `uint64` | implicit |  |
| 12 | `swap_used` | `uint64` | implicit |  |
| 13 | `disks` | `newport.monitor.MetricsDisk` | repeated |  |
| 14 | `network` | `newport.monitor.MetricsNetwork` | repeated |  |
| 15 | `processes` | `newport.monitor.MetricsProcess` | repeated |  |
| 16 | `process_count` | `uint32` | implicit |  |

### `newport.monitor.MetricsCore`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint32` | implicit |  |
| 2 | `usage` | `double` | implicit |  |
| 3 | `history` | `double` | repeated |  |

### `newport.monitor.MetricsDisk`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `mount` | `string` | implicit |  |
| 2 | `device` | `string` | implicit |  |
| 3 | `total` | `uint64` | implicit |  |
| 4 | `used` | `uint64` | implicit |  |
| 5 | `available` | `sint64` | implicit |  |

### `newport.monitor.MetricsNetwork`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | implicit |  |
| 2 | `received` | `uint64` | implicit |  |
| 3 | `sent` | `uint64` | implicit |  |

### `newport.monitor.MetricsProcess`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `pid` | `uint32` | implicit |  |
| 2 | `user` | `string` | implicit |  |
| 3 | `cpu` | `double` | implicit |  |
| 4 | `memory` | `uint64` | implicit |  |
| 5 | `state` | `string` | implicit |  |
| 6 | `name` | `string` | implicit |  |
| 7 | `elapsed` | `uint64` | optional |  |
| 8 | `command` | `string` | optional |  |

### `newport.monitor.ServicesLogsRequest`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `scope` | `newport.monitor.ServicesScope` | implicit |  |
| 2 | `unit` | `string` | optional |  |
| 3 | `priority` | `newport.monitor.ServicesPriority` | optional |  |
| 4 | `cursor` | `string` | optional |  |
| 5 | `lines` | `uint32` | optional |  |

### `newport.monitor.ServicesService`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | implicit |  |
| 2 | `scope` | `newport.monitor.ServicesScope` | implicit |  |
| 3 | `description` | `string` | implicit |  |
| 4 | `load` | `string` | implicit |  |
| 5 | `active` | `string` | implicit |  |
| 6 | `sub` | `string` | implicit |  |
| 7 | `enabled` | `string` | optional |  |
| 8 | `main_pid` | `uint32` | optional |  |
| 9 | `since` | `sint64` | optional |  |
| 10 | `memory` | `uint64` | optional |  |
| 11 | `cpu` | `double` | optional |  |
| 12 | `restarts` | `uint64` | optional |  |
| 13 | `result` | `string` | optional |  |
| 14 | `exit_code` | `sint64` | optional |  |
| 15 | `exit_kind` | `newport.monitor.ServicesExitKind` | optional |  |
| 16 | `fragment` | `string` | optional |  |
| 17 | `drop_ins` | `string` | repeated |  |
| 18 | `triggered_by` | `string` | repeated |  |
| 19 | `user` | `string` | optional |  |

### `newport.monitor.ServicesTimer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | implicit |  |
| 2 | `scope` | `newport.monitor.ServicesScope` | implicit |  |
| 3 | `description` | `string` | implicit |  |
| 4 | `active` | `string` | implicit |  |
| 5 | `next` | `sint64` | optional |  |
| 6 | `last` | `sint64` | optional |  |
| 7 | `unit` | `string` | optional |  |

### `newport.monitor.ServicesUserManager`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `available` | `bool` | implicit |  |
| 2 | `reason` | `string` | optional |  |

### `newport.monitor.ServicesSnapshot`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `services` | `newport.monitor.ServicesService` | repeated |  |
| 2 | `timers` | `newport.monitor.ServicesTimer` | repeated |  |
| 3 | `user` | `newport.monitor.ServicesUserManager` | message presence |  |
| 4 | `privilege` | `newport.monitor.ServicesPrivilege` | implicit |  |

### `newport.monitor.ServicesEntry`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `at` | `sint64` | optional |  |
| 2 | `priority` | `uint32` | optional |  |
| 3 | `pid` | `uint32` | optional |  |
| 4 | `identifier` | `string` | optional |  |
| 5 | `message` | `string` | implicit |  |

### `newport.monitor.ServicesLogs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `entries` | `newport.monitor.ServicesEntry` | repeated |  |
| 2 | `cursor` | `string` | optional |  |

### `newport.monitor.ServicesUnitFile`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `text` | `string` | implicit |  |

### `newport.monitor.ServicesActionReply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ok` | `bool` | implicit |  |
| 2 | `needs_password` | `bool` | optional |  |
| 3 | `command` | `string` | optional |  |

### `newport.monitor.ContainersEngineInfo`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `string` | implicit |  |
| 2 | `api_version` | `string` | implicit |  |
| 3 | `os` | `string` | implicit |  |
| 4 | `rootless` | `bool` | implicit |  |
| 5 | `podman` | `bool` | implicit |  |

### `newport.monitor.ContainersContainerPort`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ip` | `string` | optional |  |
| 2 | `host_port` | `uint32` | optional |  |
| 3 | `container_port` | `uint32` | implicit |  |
| 4 | `protocol` | `newport.monitor.ContainersPortProtocol` | implicit |  |

### `newport.monitor.ContainersComposeLabels`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `project` | `string` | implicit |  |
| 2 | `service` | `string` | implicit |  |
| 3 | `oneoff` | `bool` | implicit |  |
| 4 | `number` | `uint64` | optional |  |
| 5 | `working_dir` | `string` | optional |  |
| 6 | `config_files` | `string` | optional |  |

### `newport.monitor.ContainersContainer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |
| 2 | `name` | `string` | implicit |  |
| 3 | `image` | `string` | implicit |  |
| 4 | `image_id` | `string` | implicit |  |
| 5 | `command` | `string` | implicit |  |
| 6 | `state` | `newport.monitor.ContainersContainerState` | implicit |  |
| 7 | `status` | `string` | implicit |  |
| 8 | `health` | `newport.monitor.ContainersContainerHealth` | optional |  |
| 9 | `exit_code` | `sint64` | optional |  |
| 10 | `oom_killed` | `bool` | implicit |  |
| 11 | `error` | `string` | optional |  |
| 12 | `restart_count` | `uint64` | implicit |  |
| 13 | `restart_policy` | `string` | optional |  |
| 14 | `created` | `sint64` | implicit |  |
| 15 | `started_at` | `sint64` | optional |  |
| 16 | `finished_at` | `sint64` | optional |  |
| 17 | `ports` | `newport.monitor.ContainersContainerPort` | repeated |  |
| 18 | `compose` | `newport.monitor.ContainersComposeLabels` | message presence |  |
| 19 | `cpu` | `double` | optional |  |
| 20 | `memory` | `uint64` | optional |  |
| 21 | `memory_limit` | `uint64` | optional |  |

### `newport.monitor.ContainersContainersSnapshot`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `engine` | `newport.monitor.ContainersEngineInfo` | message presence |  |
| 2 | `containers` | `newport.monitor.ContainersContainer` | repeated |  |
| 3 | `truncated` | `bool` | implicit |  |

### `newport.monitor.ContainersEnvVar`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | implicit |  |
| 2 | `value` | `string` | implicit |  |

### `newport.monitor.ContainersMount`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `kind` | `string` | implicit |  |
| 2 | `source` | `string` | optional |  |
| 3 | `destination` | `string` | implicit |  |
| 4 | `mode` | `string` | optional |  |
| 5 | `rw` | `bool` | implicit |  |
| 6 | `name` | `string` | optional |  |

### `newport.monitor.ContainersNetwork`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | implicit |  |
| 2 | `ip_address` | `string` | optional |  |
| 3 | `aliases` | `string` | repeated |  |

### `newport.monitor.ContainersHealthcheck`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `test` | `string` | repeated |  |
| 2 | `interval` | `uint64` | optional |  |

### `newport.monitor.ContainersHealthResult`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `at` | `sint64` | implicit |  |
| 2 | `exit_code` | `sint64` | implicit |  |
| 3 | `output` | `string` | implicit |  |

### `newport.monitor.ContainersContainerInspect`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |
| 2 | `name` | `string` | implicit |  |
| 3 | `image` | `string` | implicit |  |
| 4 | `created` | `sint64` | implicit |  |
| 5 | `entrypoint` | `newport.monitor.StringList` | message presence |  |
| 6 | `cmd` | `newport.monitor.StringList` | message presence |  |
| 7 | `working_dir` | `string` | optional |  |
| 8 | `user` | `string` | optional |  |
| 9 | `hostname` | `string` | optional |  |
| 10 | `env` | `newport.monitor.ContainersEnvVar` | repeated |  |
| 11 | `mounts` | `newport.monitor.ContainersMount` | repeated |  |
| 12 | `networks` | `newport.monitor.ContainersNetwork` | repeated |  |
| 13 | `restart_policy` | `string` | optional |  |
| 14 | `labels` | `newport.monitor.Label` | repeated |  |
| 15 | `healthcheck` | `newport.monitor.ContainersHealthcheck` | message presence |  |
| 16 | `last_health` | `newport.monitor.ContainersHealthResult` | repeated |  |

### `newport.monitor.ContainersContainerLogEntry`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `at` | `sint64` | optional |  |
| 2 | `stream` | `newport.monitor.ContainersLogStream` | implicit |  |
| 3 | `message` | `string` | implicit |  |
| 4 | `container` | `string` | optional |  |

### `newport.monitor.ContainersContainerLogPage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `entries` | `newport.monitor.ContainersContainerLogEntry` | repeated |  |
| 2 | `cursor` | `string` | optional |  |

### `newport.monitor.ContainersDockerImage`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |
| 2 | `tags` | `string` | repeated |  |
| 3 | `size` | `uint64` | implicit |  |
| 4 | `shared_size` | `uint64` | optional |  |
| 5 | `created` | `sint64` | implicit |  |
| 6 | `containers` | `uint64` | implicit |  |
| 7 | `dangling` | `bool` | implicit |  |

### `newport.monitor.ContainersDockerVolume`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | implicit |  |
| 2 | `driver` | `string` | implicit |  |
| 3 | `size` | `uint64` | optional |  |
| 4 | `ref_count` | `uint64` | optional |  |
| 5 | `anonymous` | `bool` | implicit |  |
| 6 | `compose` | `string` | optional |  |

### `newport.monitor.ContainersDiskTotals`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `count` | `uint64` | implicit |  |
| 2 | `size` | `uint64` | implicit |  |
| 3 | `reclaimable` | `uint64` | implicit |  |

### `newport.monitor.ContainersDockerTotals`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `images` | `newport.monitor.ContainersDiskTotals` | message presence |  |
| 2 | `containers` | `newport.monitor.ContainersDiskTotals` | message presence |  |
| 3 | `volumes` | `newport.monitor.ContainersDiskTotals` | message presence |  |
| 4 | `build_cache` | `newport.monitor.ContainersDiskTotals` | message presence |  |

### `newport.monitor.ContainersDockerDisk`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `images` | `newport.monitor.ContainersDockerImage` | repeated |  |
| 2 | `volumes` | `newport.monitor.ContainersDockerVolume` | repeated |  |
| 3 | `totals` | `newport.monitor.ContainersDockerTotals` | message presence |  |
| 4 | `volumes_measured` | `bool` | implicit |  |

### `newport.monitor.ContainersPruned`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `removed` | `uint64` | implicit |  |
| 2 | `reclaimed` | `uint64` | implicit |  |

### `newport.monitor.ContainersDone`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ok` | `bool` | implicit |  |

### `newport.monitor.ContainersRemoved`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `removed` | `uint64` | implicit |  |

### `newport.monitor.ComposeCli`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `kind` | `newport.monitor.ComposeCliKind` | implicit |  |
| 2 | `version` | `string` | implicit |  |

### `newport.monitor.ComposeFile`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `path` | `string` | implicit |  |
| 2 | `text` | `string` | optional |  |
| 3 | `error` | `string` | optional |  |

### `newport.monitor.ComposeService`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | implicit |  |
| 2 | `image` | `string` | optional |  |
| 3 | `build` | `bool` | implicit |  |
| 4 | `profiles` | `string` | repeated |  |

### `newport.monitor.ComposeProject`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `cli` | `newport.monitor.ComposeCli` | message presence |  |
| 2 | `cli_error` | `string` | optional |  |
| 3 | `working_dir` | `string` | optional |  |
| 4 | `config_files` | `string` | repeated |  |
| 5 | `files` | `newport.monitor.ComposeFile` | repeated |  |
| 6 | `services` | `newport.monitor.ComposeServiceList` | message presence |  |
| 7 | `config_error` | `string` | optional |  |

### `newport.monitor.ComposeStarted`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `job` | `string` | implicit |  |

### `newport.monitor.ComposeJob`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `job` | `string` | implicit |  |
| 2 | `project` | `string` | implicit |  |
| 3 | `command` | `string` | implicit |  |
| 4 | `state` | `newport.monitor.ComposeState` | implicit |  |
| 5 | `exit_code` | `sint32` | optional |  |
| 6 | `started_at` | `sint64` | implicit |  |
| 7 | `finished_at` | `sint64` | optional |  |
| 8 | `output` | `string` | implicit |  |
| 9 | `offset` | `uint64` | implicit |  |

### `newport.monitor.ComposeJobs`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `jobs` | `newport.monitor.ComposeJob` | repeated |  |

### `newport.monitor.ComposeDone`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ok` | `bool` | implicit |  |

### `newport.monitor.WatchRequest`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `watch` | `string` | implicit |  |
| 2 | `root` | `string` | implicit |  |
| 3 | `directories` | `string` | repeated |  |
| 4 | `cursor` | `string` | optional |  |

### `newport.monitor.WatchReply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `cursor` | `string` | optional |  |
| 2 | `paths` | `string` | repeated |  |
| 3 | `reset` | `bool` | implicit |  |

### `newport.monitor.DisplayState`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `running` | `bool` | implicit |  |
| 2 | `viewer` | `newport.monitor.DisplayViewer` | message presence |  |

### `newport.monitor.DisplayViewer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `string` | implicit |  |
| 2 | `computer` | `string` | optional |  |

### `newport.monitor.ServicesUnit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `scope` | `newport.monitor.ServicesScope` | implicit |  |
| 2 | `unit` | `string` | implicit |  |

### `newport.monitor.ServicesAction`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `scope` | `newport.monitor.ServicesScope` | implicit |  |
| 2 | `unit` | `string` | implicit |  |
| 3 | `verb` | `newport.monitor.ServicesVerb` | implicit |  |

### `newport.monitor.ServicesRequest`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `newport.wire.Present` | oneof `operation` |  |
| 2 | `logs` | `newport.monitor.ServicesLogsRequest` | oneof `operation` |  |
| 3 | `unit` | `newport.monitor.ServicesUnit` | oneof `operation` |  |
| 4 | `action` | `newport.monitor.ServicesAction` | oneof `operation` |  |

### `newport.monitor.ServicesReply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `newport.monitor.ServicesSnapshot` | oneof `result` |  |
| 2 | `logs` | `newport.monitor.ServicesLogs` | oneof `result` |  |
| 3 | `unit` | `newport.monitor.ServicesUnitFile` | oneof `result` |  |
| 4 | `action` | `newport.monitor.ServicesActionReply` | oneof `result` |  |

### `newport.monitor.ContainersTarget`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `container` | `string` | implicit |  |

### `newport.monitor.ContainersLogsRequest`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `container` | `string` | oneof `target` |  |
| 2 | `project` | `string` | oneof `target` |  |
| 3 | `cursor` | `string` | optional |  |
| 4 | `lines` | `uint32` | optional |  |

### `newport.monitor.ContainersAction`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `container` | `string` | implicit |  |
| 2 | `verb` | `newport.monitor.ContainersContainerVerb` | implicit |  |

### `newport.monitor.ContainersProjectAction`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `project` | `string` | implicit |  |
| 2 | `verb` | `newport.monitor.ContainersProjectVerb` | implicit |  |
| 3 | `expected_ids` | `string` | repeated |  |

### `newport.monitor.ContainersRemove`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `containers` | `string` | repeated |  |

### `newport.monitor.ContainersDiskRequest`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `part` | `newport.monitor.ContainersDiskPart` | optional |  |

### `newport.monitor.ContainersPrune`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `what` | `newport.monitor.ContainersPruneTarget` | implicit |  |

### `newport.monitor.ContainersRequest`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `newport.wire.Present` | oneof `operation` |  |
| 2 | `inspect` | `newport.monitor.ContainersTarget` | oneof `operation` |  |
| 3 | `logs` | `newport.monitor.ContainersLogsRequest` | oneof `operation` |  |
| 4 | `action` | `newport.monitor.ContainersAction` | oneof `operation` |  |
| 5 | `project_action` | `newport.monitor.ContainersProjectAction` | oneof `operation` |  |
| 6 | `remove` | `newport.monitor.ContainersRemove` | oneof `operation` |  |
| 7 | `disk` | `newport.monitor.ContainersDiskRequest` | oneof `operation` |  |
| 8 | `prune` | `newport.monitor.ContainersPrune` | oneof `operation` |  |

### `newport.monitor.ContainersReply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `snapshot` | `newport.monitor.ContainersContainersSnapshot` | oneof `result` |  |
| 2 | `inspect` | `newport.monitor.ContainersContainerInspect` | oneof `result` |  |
| 3 | `logs` | `newport.monitor.ContainersContainerLogPage` | oneof `result` |  |
| 4 | `action` | `newport.monitor.ContainersDone` | oneof `result` |  |
| 5 | `project_action` | `newport.monitor.ContainersDone` | oneof `result` |  |
| 6 | `removed` | `newport.monitor.ContainersRemoved` | oneof `result` |  |
| 7 | `disk` | `newport.monitor.ContainersDockerDisk` | oneof `result` |  |
| 8 | `pruned` | `newport.monitor.ContainersPruned` | oneof `result` |  |

### `newport.monitor.ComposeUp`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `services` | `string` | repeated |  |
| 2 | `build` | `bool` | implicit |  |
| 3 | `pull` | `bool` | implicit |  |
| 4 | `force_recreate` | `bool` | implicit |  |

### `newport.monitor.ComposeDown`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `volumes` | `bool` | implicit |  |
| 2 | `remove_orphans` | `bool` | implicit |  |

### `newport.monitor.ComposePull`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `services` | `string` | repeated |  |

### `newport.monitor.ComposeBuild`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `services` | `string` | repeated |  |
| 2 | `no_cache` | `bool` | implicit |  |

### `newport.monitor.ComposeScale`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `service` | `string` | implicit |  |
| 2 | `replicas` | `uint32` | implicit |  |

### `newport.monitor.ComposeAction`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `up` | `newport.monitor.ComposeUp` | oneof `operation` |  |
| 2 | `down` | `newport.monitor.ComposeDown` | oneof `operation` |  |
| 3 | `pull` | `newport.monitor.ComposePull` | oneof `operation` |  |
| 4 | `build` | `newport.monitor.ComposeBuild` | oneof `operation` |  |
| 5 | `scale` | `newport.monitor.ComposeScale` | oneof `operation` |  |

### `newport.monitor.ComposeProjectRequest`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `project` | `string` | implicit |  |

### `newport.monitor.ComposeRun`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `project` | `string` | implicit |  |
| 2 | `action` | `newport.monitor.ComposeAction` | message presence |  |

### `newport.monitor.ComposeJobRequest`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `job` | `string` | implicit |  |
| 2 | `offset` | `uint64` | implicit |  |

### `newport.monitor.ComposeCancel`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `job` | `string` | implicit |  |

### `newport.monitor.ComposeRequest`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `project` | `newport.monitor.ComposeProjectRequest` | oneof `operation` |  |
| 2 | `run` | `newport.monitor.ComposeRun` | oneof `operation` |  |
| 3 | `job` | `newport.monitor.ComposeJobRequest` | oneof `operation` |  |
| 4 | `cancel` | `newport.monitor.ComposeCancel` | oneof `operation` |  |
| 5 | `jobs` | `newport.wire.Present` | oneof `operation` |  |

### `newport.monitor.ComposeReply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `project` | `newport.monitor.ComposeProject` | oneof `result` |  |
| 2 | `started` | `newport.monitor.ComposeStarted` | oneof `result` |  |
| 3 | `job` | `newport.monitor.ComposeJob` | oneof `result` |  |
| 4 | `cancelled` | `newport.monitor.ComposeDone` | oneof `result` |  |
| 5 | `jobs` | `newport.monitor.ComposeJobs` | oneof `result` |  |

### `newport.monitor.Call`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `metrics` | `newport.wire.Present` | oneof `request` |  |
| 3 | `stable_display` | `newport.wire.Present` | oneof `request` |  |
| 4 | `services` | `newport.monitor.ServicesRequest` | oneof `request` |  |
| 5 | `containers` | `newport.monitor.ContainersRequest` | oneof `request` |  |
| 6 | `compose` | `newport.monitor.ComposeRequest` | oneof `request` |  |
| 9 | `discovery` | `newport.discovery.Request` | oneof `request` |  |
| 8 | `file_watch` | `newport.monitor.WatchRequest` | oneof `request` |  |

Reserved: 7, `ports`.

### `newport.monitor.Response`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `metrics` | `newport.monitor.MetricsSnapshot` | oneof `reply` |  |
| 3 | `stable_display` | `newport.monitor.DisplayState` | oneof `reply` |  |
| 4 | `services` | `newport.monitor.ServicesReply` | oneof `reply` |  |
| 5 | `containers` | `newport.monitor.ContainersReply` | oneof `reply` |  |
| 6 | `compose` | `newport.monitor.ComposeReply` | oneof `reply` |  |
| 10 | `discovery` | `newport.discovery.Snapshot` | oneof `reply` |  |
| 8 | `file_watch` | `newport.monitor.WatchReply` | oneof `reply` |  |
| 9 | `error` | `newport.wire.Error` | oneof `reply` |  |

Reserved: 7, `ports`.

### `newport.monitor.ClientFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `call` | `newport.monitor.Call` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ClientControl` | oneof `message` |  |

### `newport.monitor.ServerFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `response` | `newport.monitor.Response` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ServerControl` | oneof `message` |  |

### `newport.monitor.ComposeServiceList`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `values` | `newport.monitor.ComposeService` | repeated |  |

### `newport.monitor.ServicesScope`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `SERVICES_SCOPE_UNSPECIFIED` |  |
| 1 | `SERVICES_SCOPE_SYSTEM` |  |
| 2 | `SERVICES_SCOPE_USER` |  |

### `newport.monitor.ServicesPrivilege`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `SERVICES_PRIVILEGE_UNSPECIFIED` |  |
| 1 | `SERVICES_PRIVILEGE_ROOT` |  |
| 2 | `SERVICES_PRIVILEGE_SUDO` |  |
| 3 | `SERVICES_PRIVILEGE_PASSWORD` |  |
| 4 | `SERVICES_PRIVILEGE_NONE` |  |

### `newport.monitor.ServicesVerb`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `SERVICES_VERB_UNSPECIFIED` |  |
| 1 | `SERVICES_VERB_START` |  |
| 2 | `SERVICES_VERB_STOP` |  |
| 3 | `SERVICES_VERB_RESTART` |  |
| 4 | `SERVICES_VERB_RELOAD` |  |
| 5 | `SERVICES_VERB_ENABLE` |  |
| 6 | `SERVICES_VERB_DISABLE` |  |

### `newport.monitor.ServicesPriority`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `SERVICES_PRIORITY_UNSPECIFIED` |  |
| 1 | `SERVICES_PRIORITY_ERR` |  |
| 2 | `SERVICES_PRIORITY_WARNING` |  |

### `newport.monitor.ServicesExitKind`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `SERVICES_EXIT_KIND_UNSPECIFIED` |  |
| 1 | `SERVICES_EXIT_KIND_EXITED` |  |
| 2 | `SERVICES_EXIT_KIND_KILLED` |  |
| 3 | `SERVICES_EXIT_KIND_DUMPED` |  |

### `newport.monitor.ContainersContainerVerb`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CONTAINERS_CONTAINER_VERB_UNSPECIFIED` |  |
| 1 | `CONTAINERS_CONTAINER_VERB_START` |  |
| 2 | `CONTAINERS_CONTAINER_VERB_STOP` |  |
| 3 | `CONTAINERS_CONTAINER_VERB_RESTART` |  |
| 4 | `CONTAINERS_CONTAINER_VERB_PAUSE` |  |
| 5 | `CONTAINERS_CONTAINER_VERB_UNPAUSE` |  |

### `newport.monitor.ContainersProjectVerb`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CONTAINERS_PROJECT_VERB_UNSPECIFIED` |  |
| 1 | `CONTAINERS_PROJECT_VERB_START` |  |
| 2 | `CONTAINERS_PROJECT_VERB_STOP` |  |
| 3 | `CONTAINERS_PROJECT_VERB_RESTART` |  |

### `newport.monitor.ContainersPruneTarget`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CONTAINERS_PRUNE_TARGET_UNSPECIFIED` |  |
| 1 | `CONTAINERS_PRUNE_TARGET_CONTAINERS` |  |
| 2 | `CONTAINERS_PRUNE_TARGET_DANGLING_IMAGES` |  |
| 3 | `CONTAINERS_PRUNE_TARGET_UNUSED_IMAGES` |  |
| 4 | `CONTAINERS_PRUNE_TARGET_BUILD_CACHE` |  |
| 5 | `CONTAINERS_PRUNE_TARGET_VOLUMES` |  |

### `newport.monitor.ContainersContainerState`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CONTAINERS_CONTAINER_STATE_UNSPECIFIED` |  |
| 1 | `CONTAINERS_CONTAINER_STATE_CREATED` |  |
| 2 | `CONTAINERS_CONTAINER_STATE_RUNNING` |  |
| 3 | `CONTAINERS_CONTAINER_STATE_PAUSED` |  |
| 4 | `CONTAINERS_CONTAINER_STATE_RESTARTING` |  |
| 5 | `CONTAINERS_CONTAINER_STATE_REMOVING` |  |
| 6 | `CONTAINERS_CONTAINER_STATE_EXITED` |  |
| 7 | `CONTAINERS_CONTAINER_STATE_DEAD` |  |

### `newport.monitor.ContainersContainerHealth`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CONTAINERS_CONTAINER_HEALTH_UNSPECIFIED` |  |
| 1 | `CONTAINERS_CONTAINER_HEALTH_HEALTHY` |  |
| 2 | `CONTAINERS_CONTAINER_HEALTH_UNHEALTHY` |  |
| 3 | `CONTAINERS_CONTAINER_HEALTH_STARTING` |  |

### `newport.monitor.ContainersPortProtocol`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CONTAINERS_PORT_PROTOCOL_UNSPECIFIED` |  |
| 1 | `CONTAINERS_PORT_PROTOCOL_TCP` |  |
| 2 | `CONTAINERS_PORT_PROTOCOL_UDP` |  |
| 3 | `CONTAINERS_PORT_PROTOCOL_SCTP` |  |

### `newport.monitor.ContainersLogStream`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CONTAINERS_LOG_STREAM_UNSPECIFIED` |  |
| 1 | `CONTAINERS_LOG_STREAM_STDOUT` |  |
| 2 | `CONTAINERS_LOG_STREAM_STDERR` |  |

### `newport.monitor.ContainersDiskPart`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CONTAINERS_DISK_PART_UNSPECIFIED` |  |
| 1 | `CONTAINERS_DISK_PART_QUICK` |  |
| 2 | `CONTAINERS_DISK_PART_VOLUMES` |  |

### `newport.monitor.ComposeCliKind`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `COMPOSE_CLI_KIND_UNSPECIFIED` |  |
| 1 | `COMPOSE_CLI_KIND_PLUGIN` |  |
| 2 | `COMPOSE_CLI_KIND_STANDALONE` |  |
| 3 | `COMPOSE_CLI_KIND_PODMAN` |  |

### `newport.monitor.ComposeState`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `COMPOSE_STATE_UNSPECIFIED` |  |
| 1 | `COMPOSE_STATE_RUNNING` |  |
| 2 | `COMPOSE_STATE_SUCCEEDED` |  |
| 3 | `COMPOSE_STATE_FAILED` |  |
| 4 | `COMPOSE_STATE_CANCELLED` |  |

## Schema `newport/remote/apps.proto`

Source: [newport/remote/apps.proto](proto/newport/remote/apps.proto). Package: `newport.remote.apps`.

Remote Apps, protocol 1: Newport and the account's display, over the
 relayed stream. Each frame is one `ClientFrame` or `ServerFrame`.

 Session IDs are unpredictable, so they are `fixed64`. Frames, input and
 acknowledgments, the messages sent most, hold field numbers 1 to 15.

Imports: `newport/wire.proto`, `newport/worker.proto`, `newport/remote/display.proto`.

### `newport.remote.apps.ClientFrame`

Everything a client sends.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `input` | `newport.remote.apps.ClientFrame.Input` | oneof `message` |  |
| 2 | `ack` | `newport.remote.apps.ClientFrame.Ack` | oneof `message` |  |
| 3 | `heartbeat` | `newport.remote.apps.ClientFrame.Heartbeat` | oneof `message` |  |
| 4 | `request` | `newport.remote.apps.ClientFrame.Request` | oneof `message` |  |
| 5 | `text_input` | `newport.remote.apps.ClientFrame.TextInput` | oneof `message` |  |
| 6 | `drag` | `newport.remote.apps.ClientFrame.Drag` | oneof `message` |  |
| 7 | `reverse_drag` | `newport.remote.apps.ClientFrame.ReverseDrag` | oneof `message` |  |
| 8 | `reverse_drag_exit` | `newport.remote.apps.ClientFrame.ReverseDragExit` | oneof `message` |  |
| 9 | `file_upload` | `newport.remote.apps.ClientFrame.FileUpload` | oneof `message` |  |
| 10 | `reverse_files` | `newport.remote.apps.ClientFrame.ReverseFiles` | oneof `message` |  |
| 11 | `launch` | `newport.remote.apps.ClientFrame.Launch` | oneof `message` |  |
| 12 | `stop_application` | `newport.remote.apps.ClientFrame.StopApplication` | oneof `message` |  |
| 13 | `application_presented` | `newport.remote.apps.Lifecycle.Presented` | oneof `message` |  |
| 14 | `license` | `newport.remote.apps.ClientFrame.License` | oneof `message` |  |
| 15 | `quit` | `newport.remote.apps.ClientFrame.Quit` | oneof `message` |  |
| 16 | `hello` | `newport.remote.apps.ClientFrame.Hello` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ClientControl` | oneof `message` |  |

### `newport.remote.apps.ClientFrame.Input`

Input for one surface of a window.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 3 | `surface` | `uint64` | implicit |  |
| 4 | `input` | `newport.remote.apps.Input` | message presence |  |

### `newport.remote.apps.ClientFrame.Ack`

A frame was applied; it returns the frame's credit.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 3 | `surface` | `uint64` | implicit |  |
| 4 | `sequence` | `uint64` | implicit |  |

### `newport.remote.apps.ClientFrame.Heartbeat`

The client is still there.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |

### `newport.remote.apps.ClientFrame.Request`

An operation on a window, answered by an `OperationResult`.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `request` | `uint64` | implicit |  |
| 3 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 4 | `operation` | `newport.remote.apps.Operation` | message presence |  |

### `newport.remote.apps.ClientFrame.TextInput`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `message` | `newport.remote.apps.TextInput.Client` | message presence |  |

### `newport.remote.apps.ClientFrame.Drag`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `message` | `newport.remote.apps.Drag.Client` | message presence |  |

### `newport.remote.apps.ClientFrame.ReverseDrag`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `message` | `newport.remote.apps.ReverseDrag.Client` | message presence |  |

### `newport.remote.apps.ClientFrame.ReverseDragExit`

The pointer left the viewport during a press, before the display
 offered a drag.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `gesture` | `uint64` | implicit |  |
| 3 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 4 | `surface` | `uint64` | implicit |  |

### `newport.remote.apps.ClientFrame.FileUpload`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `message` | `newport.remote.apps.Upload.Client` | message presence |  |

### `newport.remote.apps.ClientFrame.ReverseFiles`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `message` | `newport.remote.apps.ReverseFiles.Client` | message presence |  |

### `newport.remote.apps.ClientFrame.Launch`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `request` | `uint64` | implicit |  |
| 3 | `target` | `newport.remote.apps.LaunchTarget` | message presence |  |

### `newport.remote.apps.ClientFrame.StopApplication`

Stops the owned group of one launch, never the display.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `request` | `uint64` | implicit |  |

### `newport.remote.apps.ClientFrame.License`

A renewed license for the running session.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `token` | `newport.wire.Secret` | message presence |  |

### `newport.remote.apps.ClientFrame.Quit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |

### `newport.remote.apps.ClientFrame.Hello`

The client's first message.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `versions` | `uint32` | repeated | The protocol versions the client speaks. |
| 2 | `features` | `newport.remote.apps.Features` | message presence | What the client will do in this session. |
| 3 | `performance` | `newport.remote.apps.PerformanceSettings` | message presence |  |
| 4 | `display` | `newport.remote.apps.DisplayProfile` | message presence |  |
| 5 | `license` | `newport.wire.Secret` | message presence | The Newport Pro license the agent checks before `Welcome`. |

### `newport.remote.apps.ServerFrame`

Everything the display sends.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `frame` | `newport.remote.apps.ServerFrame.Frame` | oneof `message` |  |
| 2 | `audio` | `newport.remote.apps.ServerFrame.Audio` | oneof `message` |  |
| 3 | `cursor` | `newport.remote.apps.ServerFrame.Cursor` | oneof `message` |  |
| 4 | `surface_changed` | `newport.remote.apps.ServerFrame.SurfaceChanged` | oneof `message` |  |
| 5 | `window_updated` | `newport.remote.apps.ServerFrame.WindowUpdated` | oneof `message` |  |
| 6 | `window_created` | `newport.remote.apps.ServerFrame.WindowCreated` | oneof `message` |  |
| 7 | `window_destroyed` | `newport.remote.apps.ServerFrame.WindowDestroyed` | oneof `message` |  |
| 8 | `window_action` | `newport.remote.apps.ServerFrame.WindowAction` | oneof `message` |  |
| 9 | `operation_result` | `newport.remote.apps.ServerFrame.OperationResult` | oneof `message` |  |
| 10 | `text_input` | `newport.remote.apps.ServerFrame.TextInput` | oneof `message` |  |
| 11 | `audio_state` | `newport.remote.apps.ServerFrame.AudioState` | oneof `message` |  |
| 12 | `drag` | `newport.remote.apps.ServerFrame.Drag` | oneof `message` |  |
| 13 | `reverse_drag` | `newport.remote.apps.ServerFrame.ReverseDrag` | oneof `message` |  |
| 14 | `file_upload` | `newport.remote.apps.ServerFrame.FileUpload` | oneof `message` |  |
| 15 | `reverse_files` | `newport.remote.apps.ServerFrame.ReverseFiles` | oneof `message` |  |
| 16 | `welcome` | `newport.remote.apps.ServerFrame.Welcome` | oneof `message` |  |
| 17 | `rejected` | `newport.remote.apps.ServerFrame.Rejected` | oneof `message` |  |
| 18 | `ended` | `newport.remote.apps.ServerFrame.Ended` | oneof `message` |  |
| 19 | `closed` | `newport.remote.apps.ServerFrame.Closed` | oneof `message` |  |
| 20 | `error` | `newport.remote.apps.ServerFrame.Error` | oneof `message` |  |
| 21 | `application_lifecycle` | `newport.remote.apps.Lifecycle.Update` | oneof `message` |  |
| 22 | `session_applications` | `newport.remote.apps.ServerFrame.SessionApplications` | oneof `message` |  |
| 23 | `session_activity` | `newport.remote.apps.ServerFrame.SessionActivity` | oneof `message` |  |
| 24 | `launch_result` | `newport.remote.apps.ServerFrame.LaunchResult` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ServerControl` | oneof `message` |  |

### `newport.remote.apps.ServerFrame.Frame`

The changed tiles of one surface.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 3 | `surface` | `uint64` | implicit |  |
| 4 | `sequence` | `uint64` | implicit |  |
| 5 | `keyframe` | `bool` | implicit |  |
| 6 | `tiles` | `newport.remote.apps.Tile` | repeated |  |
| 7 | `captured` | `newport.remote.apps.MediaTime` | message presence |  |
| 8 | `diagnostics` | `newport.remote.apps.FrameDiagnostics` | message presence |  |

### `newport.remote.apps.ServerFrame.Audio`

One Opus packet.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `sequence` | `uint64` | implicit |  |
| 3 | `sample_time` | `uint64` | implicit |  |
| 4 | `data` | `bytes` | implicit |  |
| 5 | `captured` | `newport.remote.apps.MediaTime` | message presence |  |
| 6 | `hole` | `bool` | implicit |  |
| 7 | `epoch` | `uint64` | implicit | The stream this packet belongs to, numbered from one. |

### `newport.remote.apps.ServerFrame.Cursor`

The cursor a window wants while the pointer is over it.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 3 | `cursor` | `newport.remote.apps.Cursor` | message presence |  |

### `newport.remote.apps.ServerFrame.SurfaceChanged`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 3 | `surface` | `newport.remote.apps.Surface` | message presence |  |

### `newport.remote.apps.ServerFrame.WindowUpdated`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `window` | `newport.remote.apps.Window` | message presence |  |

### `newport.remote.apps.ServerFrame.WindowCreated`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `window` | `newport.remote.apps.Window` | message presence |  |
| 3 | `surface` | `newport.remote.apps.Surface` | message presence |  |
| 4 | `moved` | `bool` | implicit | The window was on the display before this viewer took it over, and takes no focus. |

### `newport.remote.apps.ServerFrame.WindowDestroyed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `window` | `newport.remote.apps.WindowId` | message presence |  |

### `newport.remote.apps.ServerFrame.WindowAction`

An operation the display asks of the client's window.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 3 | `action` | `newport.remote.apps.Operation` | message presence |  |

### `newport.remote.apps.ServerFrame.OperationResult`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `request` | `uint64` | implicit |  |
| 3 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 4 | `applied` | `bool` | implicit |  |
| 5 | `message` | `string` | optional |  |

### `newport.remote.apps.ServerFrame.TextInput`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `message` | `newport.remote.apps.TextInput.Server` | message presence |  |

### `newport.remote.apps.ServerFrame.AudioState`

Whether the session's audio plays.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `waiting` | `newport.remote.apps.ServerFrame.AudioState.Waiting` | oneof `state` | No sound server plays for the session yet, or it stopped. |
| 3 | `playing` | `newport.wire.Present` | oneof `state` |  |

### `newport.remote.apps.ServerFrame.AudioState.Waiting`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `reason` | `string` | implicit |  |

### `newport.remote.apps.ServerFrame.Drag`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `message` | `newport.remote.apps.Drag.Server` | message presence |  |

### `newport.remote.apps.ServerFrame.ReverseDrag`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `message` | `newport.remote.apps.ReverseDrag.Server` | message presence |  |

### `newport.remote.apps.ServerFrame.FileUpload`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `message` | `newport.remote.apps.Upload.Server` | message presence |  |

### `newport.remote.apps.ServerFrame.ReverseFiles`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `message` | `newport.remote.apps.ReverseFiles.Server` | message presence |  |

### `newport.remote.apps.ServerFrame.Welcome`

The answer to `Hello`: what this session has.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `version` | `uint32` | implicit |  |
| 3 | `features` | `newport.remote.apps.Features` | message presence |  |
| 4 | `performance` | `newport.remote.apps.PerformanceSettings` | message presence |  |

### `newport.remote.apps.ServerFrame.Rejected`

Sent instead of `Welcome` when the session can't start. The code is
 stable; the message is for people. `display` names the display when it
 refused the relay.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `error` | `newport.wire.Error` | message presence |  |
| 2 | `display` | `newport.remote.display.DisplayIdentity` | message presence |  |

### `newport.remote.apps.ServerFrame.Ended`

The display ended a running session; the session closes next.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `reason` | `string` | implicit |  |
| 3 | `message` | `string` | implicit |  |
| 4 | `computer` | `string` | optional | After `taken-over`: the computer showing the display now. |
| 5 | `computer_session` | `string` | optional | That computer's Newport session. |

### `newport.remote.apps.ServerFrame.Closed`

The answer to `Quit`.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |

### `newport.remote.apps.ServerFrame.Error`

A failure that ends the session.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `error` | `newport.wire.Error` | message presence |  |

### `newport.remote.apps.ServerFrame.SessionApplications`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `snapshot` | `newport.remote.apps.Applications` | message presence |  |

### `newport.remote.apps.ServerFrame.SessionActivity`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `activity` | `newport.remote.apps.Activity` | message presence |  |

### `newport.remote.apps.ServerFrame.LaunchResult`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `request` | `uint64` | implicit |  |
| 3 | `outcome` | `newport.remote.apps.LaunchOutcome` | message presence |  |

### `newport.remote.apps.Features`

What a session can do beyond the base protocol. A missing field is no; a
 sub-feature is a field of its parent.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `text_input` | `newport.wire.Present` | message presence | Input method composition. |
| 2 | `upload` | `newport.wire.Present` | message presence | Files and folders from the client, by picking or dropping. |
| 3 | `drag_out` | `newport.remote.apps.DragOut` | message presence | Text and images dragged out of remote apps. |
| 4 | `audio` | `newport.wire.Present` | message presence | Opus streams with their state, and capture times on frames and audio. |

### `newport.remote.apps.DragOut`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `files` | `newport.wire.Present` | message presence | Files and folders too. |

### `newport.remote.apps.PerformanceSettings`

How the display encodes for this session.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `codec_mode` | `newport.remote.apps.CodecMode` | implicit |  |
| 2 | `max_fps` | `uint32` | implicit | Per-window frame rate goal and ceiling. |
| 3 | `jpeg_quality` | `uint32` | implicit | Ceiling for photographic tiles, also mapped to AV1 quantization. |
| 4 | `fast_trial` | `bool` | implicit | A bounded low-CPU codec trial, for local transports. |

### `newport.remote.apps.DisplayProfile`

The client's display metrics.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `scale` | `newport.remote.apps.Scale` | message presence |  |
| 2 | `work_area` | `newport.remote.apps.Size` | message presence | Usable screen size in logical points, excluding system UI. |

### `newport.remote.apps.PopupBounds`

The usable screen relative to a toplevel's content origin, in logical
 points.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `origin` | `newport.remote.apps.Point` | message presence |  |
| 2 | `size` | `newport.remote.apps.Size` | message presence |  |

### `newport.remote.apps.Point`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `x` | `sint32` | implicit |  |
| 2 | `y` | `sint32` | implicit |  |

### `newport.remote.apps.Location`

A point within a surface, in its pixels.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `x` | `uint32` | implicit |  |
| 2 | `y` | `uint32` | implicit |  |

### `newport.remote.apps.Size`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `width` | `uint32` | implicit |  |
| 2 | `height` | `uint32` | implicit |  |

### `newport.remote.apps.Rect`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `x` | `uint32` | implicit |  |
| 2 | `y` | `uint32` | implicit |  |
| 3 | `width` | `uint32` | implicit |  |
| 4 | `height` | `uint32` | implicit |  |

### `newport.remote.apps.Scale`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `numerator` | `uint32` | implicit |  |
| 2 | `denominator` | `uint32` | implicit |  |

### `newport.remote.apps.WindowId`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `generation` | `uint64` | implicit |  |

### `newport.remote.apps.Surface`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `generation` | `uint64` | implicit |  |
| 2 | `logical_width` | `uint32` | implicit |  |
| 3 | `logical_height` | `uint32` | implicit |  |
| 4 | `pixel_width` | `uint32` | implicit |  |
| 5 | `pixel_height` | `uint32` | implicit |  |
| 6 | `scale` | `newport.remote.apps.Scale` | message presence |  |

### `newport.remote.apps.Actions`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `close` | `bool` | implicit |  |
| 2 | `minimize` | `bool` | implicit |  |
| 3 | `maximize` | `bool` | implicit |  |
| 4 | `resize` | `bool` | implicit |  |
| 5 | `fullscreen` | `bool` | implicit |  |

### `newport.remote.apps.PixelConstraints`

ICCCM size hints in surface pixels.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `min` | `newport.remote.apps.Size` | message presence |  |
| 2 | `max` | `newport.remote.apps.Size` | message presence |  |
| 3 | `base` | `newport.remote.apps.Size` | message presence |  |
| 4 | `increment` | `newport.remote.apps.Size` | message presence |  |
| 5 | `aspect` | `newport.remote.apps.AspectRange` | message presence | The least and most width to height, each as width and height. |

### `newport.remote.apps.AspectRange`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `min` | `newport.remote.apps.Size` | message presence |  |
| 2 | `max` | `newport.remote.apps.Size` | message presence |  |

### `newport.remote.apps.Constraints`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `physical` | `newport.remote.apps.PixelConstraints` | message presence |  |
| 2 | `min` | `newport.remote.apps.Size` | message presence |  |
| 3 | `max` | `newport.remote.apps.Size` | message presence |  |

### `newport.remote.apps.WindowApplication`

Which Linux application owns a window.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `app_id` | `string` | optional |  |
| 2 | `desktop_id` | `string` | optional |  |
| 3 | `request` | `uint64` | optional |  |

### `newport.remote.apps.Window`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `key` | `newport.remote.apps.WindowId` | message presence |  |
| 2 | `parent` | `newport.remote.apps.WindowId` | message presence |  |
| 3 | `kind` | `newport.remote.apps.WindowKind` | implicit |  |
| 4 | `modal` | `bool` | implicit |  |
| 5 | `position` | `newport.remote.apps.Point` | message presence | Logical, relative to the parent for popups, the session's otherwise. |
| 6 | `title` | `string` | implicit |  |
| 7 | `decoration` | `newport.remote.apps.Decoration` | implicit |  |
| 8 | `actions` | `newport.remote.apps.Actions` | message presence |  |
| 9 | `constraints` | `newport.remote.apps.Constraints` | message presence |  |
| 10 | `minimized` | `bool` | implicit |  |
| 11 | `maximized` | `bool` | implicit |  |
| 12 | `fullscreen` | `bool` | implicit |  |
| 13 | `application` | `newport.remote.apps.WindowApplication` | message presence |  |

### `newport.remote.apps.MediaTime`

Microseconds from the session's media epoch.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `earliest_us` | `sint64` | implicit |  |
| 2 | `latest_us` | `sint64` | implicit |  |

### `newport.remote.apps.FrameDiagnostics`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `target_fps` | `uint32` | implicit |  |
| 2 | `fast_codec` | `bool` | implicit |  |
| 3 | `ack_ms` | `double` | implicit |  |
| 4 | `work_ms` | `double` | implicit |  |
| 5 | `capture_ms` | `double` | implicit |  |
| 6 | `dispatch_ms` | `double` | implicit |  |

### `newport.remote.apps.Tile`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint32` | implicit |  |
| 2 | `codec` | `newport.remote.apps.Codec` | implicit |  |
| 3 | `data` | `bytes` | implicit |  |
| 4 | `rect` | `newport.remote.apps.Rect` | message presence | The patch within the tile the data covers; the whole tile when absent. |
| 5 | `copy` | `newport.remote.apps.TileCopy` | message presence | Rows copied from the previous frame before the data is applied. |
| 6 | `video` | `newport.remote.apps.VideoGroup` | message presence | The AV1 group header, on the group's first tile only. |

### `newport.remote.apps.TileCopy`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `source_y` | `uint32` | oneof `source` | Surface row of the previous frame, in this tile's column. |
| 2 | `sy` | `uint32` | oneof `source` | Row of this tile in the previous frame. |
| 3 | `dy` | `uint32` | implicit |  |
| 4 | `h` | `uint32` | implicit |  |

### `newport.remote.apps.VideoGroup`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `rect` | `newport.remote.apps.Rect` | message presence |  |
| 2 | `stream` | `uint64` | implicit |  |
| 3 | `sequence` | `uint64` | implicit |  |
| 4 | `keyframe` | `bool` | implicit |  |

### `newport.remote.apps.Operation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `activate` | `newport.wire.Present` | oneof `action` |  |
| 2 | `move` | `newport.wire.Present` | oneof `action` |  |
| 3 | `close` | `newport.wire.Present` | oneof `action` |  |
| 4 | `minimize` | `newport.wire.Present` | oneof `action` |  |
| 5 | `maximize` | `newport.wire.Present` | oneof `action` |  |
| 6 | `restore` | `newport.wire.Present` | oneof `action` |  |
| 7 | `fullscreen` | `bool` | oneof `action` | Whether the window should be fullscreen. |
| 8 | `show_window_menu` | `newport.remote.apps.Point` | oneof `action` | Where to show the window menu, in logical window coordinates. |
| 9 | `start_resize` | `newport.remote.apps.ResizeEdge` | oneof `action` |  |
| 10 | `resize` | `newport.remote.apps.Operation.Resize` | oneof `action` |  |
| 11 | `set_display` | `newport.remote.apps.DisplayProfile` | oneof `action` |  |
| 12 | `set_popup_bounds` | `newport.remote.apps.PopupBounds` | oneof `action` |  |

### `newport.remote.apps.Operation.Resize`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `logical_width` | `uint32` | implicit |  |
| 2 | `logical_height` | `uint32` | implicit |  |
| 3 | `scale` | `newport.remote.apps.Scale` | message presence |  |

### `newport.remote.apps.Modifiers`

The Linux modifiers held during an event.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `shift` | `bool` | implicit |  |
| 2 | `control` | `bool` | implicit |  |
| 3 | `alt` | `bool` | implicit |  |
| 4 | `super` | `bool` | implicit |  |
| 5 | `caps_lock` | `bool` | implicit |  |

### `newport.remote.apps.Input`

Pointer coordinates and scroll deltas are surface pixels.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `pointer` | `newport.remote.apps.Input.Pointer` | oneof `kind` | Motion; it also means the pointer is in the window. |
| 2 | `held_pointer` | `newport.remote.apps.Input.HeldPointer` | oneof `kind` | The pointer while a press in this window is held. |
| 3 | `button` | `newport.remote.apps.Input.Button` | oneof `kind` |  |
| 4 | `scroll` | `newport.remote.apps.Input.Scroll` | oneof `kind` |  |
| 5 | `key` | `newport.remote.apps.Input.Key` | oneof `kind` |  |
| 6 | `focus` | `newport.wire.Present` | oneof `kind` |  |
| 7 | `blur` | `newport.wire.Present` | oneof `kind` |  |
| 8 | `leave` | `newport.wire.Present` | oneof `kind` |  |

### `newport.remote.apps.Input.Pointer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `x` | `uint32` | implicit |  |
| 2 | `y` | `uint32` | implicit |  |
| 3 | `modifiers` | `newport.remote.apps.Modifiers` | message presence |  |

### `newport.remote.apps.Input.HeldPointer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `x` | `sint32` | implicit |  |
| 2 | `y` | `sint32` | implicit |  |
| 3 | `modifiers` | `newport.remote.apps.Modifiers` | message presence |  |

### `newport.remote.apps.Input.Button`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `button` | `newport.remote.apps.MouseButton` | implicit |  |
| 2 | `down` | `bool` | implicit |  |
| 3 | `modifiers` | `newport.remote.apps.Modifiers` | message presence |  |

### `newport.remote.apps.Input.Scroll`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `dx` | `double` | implicit |  |
| 2 | `dy` | `double` | implicit |  |
| 3 | `discrete` | `newport.remote.apps.Point` | message presence | Wheel notches in 1/120 steps, for mouse wheels. |
| 4 | `phase` | `newport.remote.apps.ScrollPhase` | implicit |  |
| 5 | `modifiers` | `newport.remote.apps.Modifiers` | message presence |  |
| 6 | `position` | `newport.remote.apps.Location` | message presence | Required pointer position in surface pixels, delivered in the same frame. |

### `newport.remote.apps.Input.Key`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `key` | `string` | implicit | What the key means on the client's keyboard. |
| 2 | `code` | `uint32` | implicit | The physical key as a Linux evdev code. |
| 3 | `down` | `bool` | implicit |  |
| 4 | `repeat` | `bool` | implicit |  |
| 5 | `modifiers` | `newport.remote.apps.Modifiers` | message presence |  |

### `newport.remote.apps.Cursor`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `hidden` | `newport.wire.Present` | oneof `kind` |  |
| 2 | `shape` | `newport.remote.apps.CursorShape` | oneof `kind` |  |

### `newport.remote.apps.TextInput`

Input method composition, with `features.text_input`.

Empty message.

### `newport.remote.apps.TextInput.Context`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 2 | `revision` | `uint64` | implicit |  |
| 3 | `focus_epoch` | `uint64` | implicit | The first revision of this focus. |
| 4 | `preedit` | `bool` | implicit |  |
| 5 | `caret` | `newport.remote.apps.Point` | message presence | At the caret's bottom edge, in surface pixels; absent when unknown. |

### `newport.remote.apps.TextInput.Preedit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `request` | `uint64` | implicit |  |
| 2 | `context` | `newport.remote.apps.TextInput.Context` | message presence |  |
| 3 | `text` | `string` | implicit |  |
| 4 | `cursor` | `uint32` | implicit |  |

### `newport.remote.apps.TextInput.Commit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `request` | `uint64` | implicit |  |
| 2 | `context` | `newport.remote.apps.TextInput.Context` | message presence |  |
| 3 | `text` | `string` | implicit |  |

### `newport.remote.apps.TextInput.Client`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `preedit` | `newport.remote.apps.TextInput.Preedit` | oneof `kind` |  |
| 2 | `commit` | `newport.remote.apps.TextInput.Commit` | oneof `kind` |  |

### `newport.remote.apps.TextInput.Focus`

Absent context: no text field has focus.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `context` | `newport.remote.apps.TextInput.Context` | message presence |  |

### `newport.remote.apps.TextInput.Result`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `request` | `uint64` | implicit |  |
| 2 | `applied` | `bool` | implicit |  |

### `newport.remote.apps.TextInput.Server`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `focus` | `newport.remote.apps.TextInput.Focus` | oneof `kind` |  |
| 2 | `result` | `newport.remote.apps.TextInput.Result` | oneof `kind` |  |

### `newport.remote.apps.FileTreeEntry`

A file or folder of an upload's tree; parents come before children.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `parent` | `uint32` | optional |  |
| 2 | `name` | `string` | implicit |  |
| 3 | `directory` | `newport.wire.Present` | oneof `contents` |  |
| 4 | `file` | `uint64` | oneof `contents` | A file of this many bytes. |

### `newport.remote.apps.Upload`

Stop-and-wait file transfers: uploads with `features.upload`, and the files
 of a drag out with `drag_out.files`, which go the other way.

Empty message.

### `newport.remote.apps.Upload.File`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | implicit |  |
| 2 | `bytes` | `uint64` | implicit |  |

### `newport.remote.apps.Upload.Begin`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `transfer` | `uint64` | implicit |  |
| 2 | `files` | `newport.remote.apps.Upload.File` | repeated |  |

### `newport.remote.apps.Upload.BeginTree`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `transfer` | `uint64` | implicit |  |
| 2 | `entries` | `newport.remote.apps.FileTreeEntry` | repeated |  |

### `newport.remote.apps.Upload.Chunk`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `transfer` | `uint64` | implicit |  |
| 2 | `file` | `uint32` | implicit |  |
| 3 | `offset` | `uint64` | implicit |  |
| 4 | `data` | `bytes` | implicit |  |

### `newport.remote.apps.Upload.Transfer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `transfer` | `uint64` | implicit |  |

### `newport.remote.apps.Upload.Client`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `chunk` | `newport.remote.apps.Upload.Chunk` | oneof `kind` |  |
| 2 | `begin` | `newport.remote.apps.Upload.Begin` | oneof `kind` |  |
| 3 | `begin_tree` | `newport.remote.apps.Upload.BeginTree` | oneof `kind` |  |
| 4 | `finish` | `newport.remote.apps.Upload.Transfer` | oneof `kind` |  |
| 5 | `cancel` | `newport.remote.apps.Upload.Transfer` | oneof `kind` |  |

### `newport.remote.apps.Upload.Ack`

Only this file and offset may be sent next.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `transfer` | `uint64` | implicit |  |
| 2 | `file` | `uint32` | implicit |  |
| 3 | `offset` | `uint64` | implicit |  |

### `newport.remote.apps.Upload.Rejected`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `transfer` | `uint64` | implicit |  |
| 2 | `reason` | `string` | implicit |  |

### `newport.remote.apps.Upload.Server`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ack` | `newport.remote.apps.Upload.Ack` | oneof `kind` |  |
| 2 | `ready` | `newport.remote.apps.Upload.Transfer` | oneof `kind` |  |
| 3 | `cancelled` | `newport.remote.apps.Upload.Transfer` | oneof `kind` |  |
| 4 | `rejected` | `newport.remote.apps.Upload.Rejected` | oneof `kind` |  |

### `newport.remote.apps.Drag`

Dropping an upload into a window, with `features.upload`.

Empty message.

### `newport.remote.apps.Drag.Begin`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `drag` | `uint64` | implicit |  |
| 2 | `transfer` | `uint64` | implicit |  |
| 3 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 4 | `surface` | `uint64` | implicit |  |
| 5 | `point` | `newport.remote.apps.Location` | message presence |  |

### `newport.remote.apps.Drag.Motion`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `drag` | `uint64` | implicit |  |
| 2 | `point` | `newport.remote.apps.Location` | message presence |  |

### `newport.remote.apps.Drag.Ref`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `drag` | `uint64` | implicit |  |

### `newport.remote.apps.Drag.Client`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `motion` | `newport.remote.apps.Drag.Motion` | oneof `kind` |  |
| 2 | `begin` | `newport.remote.apps.Drag.Begin` | oneof `kind` |  |
| 3 | `drop` | `newport.remote.apps.Drag.Ref` | oneof `kind` |  |
| 4 | `cancel` | `newport.remote.apps.Drag.Ref` | oneof `kind` |  |

### `newport.remote.apps.Drag.Status`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `drag` | `uint64` | implicit |  |
| 2 | `revision` | `uint64` | implicit |  |
| 3 | `accepted` | `bool` | implicit |  |

### `newport.remote.apps.Drag.Finished`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `drag` | `uint64` | implicit |  |
| 2 | `copied` | `bool` | implicit |  |
| 3 | `reason` | `string` | implicit |  |

### `newport.remote.apps.Drag.Server`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `status` | `newport.remote.apps.Drag.Status` | oneof `kind` |  |
| 2 | `finished` | `newport.remote.apps.Drag.Finished` | oneof `kind` |  |

### `newport.remote.apps.ReverseDrag`

Dragging text and images out of remote apps, with `features.drag_out`.

Empty message.

### `newport.remote.apps.ReverseDrag.Ref`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `drag` | `uint64` | implicit |  |

### `newport.remote.apps.ReverseDrag.Read`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `drag` | `uint64` | implicit |  |
| 2 | `offset` | `uint32` | implicit |  |

### `newport.remote.apps.ReverseDrag.Complete`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `drag` | `uint64` | implicit |  |
| 2 | `copied` | `bool` | implicit |  |

### `newport.remote.apps.ReverseDrag.Client`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `read` | `newport.remote.apps.ReverseDrag.Read` | oneof `kind` |  |
| 2 | `ready` | `newport.remote.apps.ReverseDrag.Ref` | oneof `kind` | The preview is installed in a native drag. |
| 3 | `complete` | `newport.remote.apps.ReverseDrag.Complete` | oneof `kind` |  |
| 4 | `cancel` | `newport.remote.apps.ReverseDrag.Ref` | oneof `kind` |  |

### `newport.remote.apps.ReverseDrag.Offer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `drag` | `uint64` | implicit |  |
| 2 | `gesture` | `uint64` | implicit |  |
| 3 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 4 | `surface` | `uint64` | implicit |  |
| 5 | `mime` | `string` | implicit |  |
| 6 | `bytes` | `uint32` | implicit |  |

### `newport.remote.apps.ReverseDrag.Chunk`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `drag` | `uint64` | implicit |  |
| 2 | `offset` | `uint32` | implicit |  |
| 3 | `data` | `bytes` | implicit |  |

### `newport.remote.apps.ReverseDrag.ExitRefused`

The exit of a gesture can't apply.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `gesture` | `uint64` | implicit |  |

### `newport.remote.apps.ReverseDrag.Server`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `chunk` | `newport.remote.apps.ReverseDrag.Chunk` | oneof `kind` |  |
| 2 | `offer` | `newport.remote.apps.ReverseDrag.Offer` | oneof `kind` |  |
| 3 | `drop` | `newport.remote.apps.ReverseDrag.Ref` | oneof `kind` |  |
| 4 | `cancel` | `newport.remote.apps.ReverseDrag.Ref` | oneof `kind` |  |
| 5 | `exit_refused` | `newport.remote.apps.ReverseDrag.ExitRefused` | oneof `kind` |  |

### `newport.remote.apps.ReverseFiles`

The files of a drag out, with `drag_out.files`: upload messages the other
 way, each for the drag it belongs to.

Empty message.

### `newport.remote.apps.ReverseFiles.Context`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `drag` | `uint64` | implicit |  |
| 2 | `gesture` | `uint64` | implicit |  |
| 3 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 4 | `surface` | `uint64` | implicit |  |

### `newport.remote.apps.ReverseFiles.Server`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `context` | `newport.remote.apps.ReverseFiles.Context` | message presence |  |
| 2 | `message` | `newport.remote.apps.Upload.Client` | message presence |  |

### `newport.remote.apps.ReverseFiles.Client`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `context` | `newport.remote.apps.ReverseFiles.Context` | message presence |  |
| 2 | `message` | `newport.remote.apps.Upload.Server` | message presence |  |

### `newport.remote.apps.LaunchTarget`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `command` | `newport.remote.apps.LaunchTarget.Command` | oneof `kind` |  |
| 2 | `desktop` | `newport.remote.apps.LaunchTarget.Desktop` | oneof `kind` |  |

### `newport.remote.apps.LaunchTarget.Command`

Literal arguments, with no shell expansion.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `argv` | `string` | repeated |  |

### `newport.remote.apps.LaunchTarget.Desktop`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |
| 2 | `uris` | `string` | repeated |  |
| 3 | `action` | `string` | optional |  |

### `newport.remote.apps.LaunchOutcome`

An owned launcher started; it says nothing of windows.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `accepted` | `newport.wire.Present` | oneof `status` |  |
| 2 | `failed` | `newport.remote.apps.LaunchOutcome.Failed` | oneof `status` |  |

### `newport.remote.apps.LaunchOutcome.Failed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `message` | `string` | implicit |  |

### `newport.remote.apps.Applications`

Launch groups the display owns for this session.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `uint64` | implicit |  |
| 2 | `applications` | `newport.remote.apps.Applications.Application` | repeated |  |

### `newport.remote.apps.Applications.Application`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `request` | `uint64` | implicit |  |
| 2 | `opening` | `newport.wire.Present` | oneof `state` |  |
| 3 | `window` | `newport.remote.apps.WindowId` | oneof `state` |  |
| 4 | `stopping` | `newport.wire.Present` | oneof `state` |  |

### `newport.remote.apps.Activity`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `uint64` | implicit |  |
| 2 | `applications` | `uint32` | implicit | Supervised application groups, with or without windows. |

### `newport.remote.apps.Lifecycle`

How a launch starts up, apart from the groups the display owns.

Empty message.

### `newport.remote.apps.Lifecycle.Window`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 2 | `surface` | `uint64` | implicit |  |
| 3 | `existing_instance` | `bool` | implicit | The launch ended on a window of an instance already running. |

### `newport.remote.apps.Lifecycle.Outcome`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `presented` | `newport.remote.apps.Lifecycle.Window` | oneof `kind` |  |
| 2 | `completed_without_presentation` | `newport.remote.apps.Lifecycle.Outcome.CompletedWithoutPresentation` | oneof `kind` |  |
| 3 | `handed_off` | `newport.wire.Present` | oneof `kind` | Another process took the launch. |
| 4 | `failed` | `newport.remote.apps.Lifecycle.Outcome.Failed` | oneof `kind` |  |
| 5 | `timed_out` | `newport.remote.apps.Lifecycle.Outcome.TimedOut` | oneof `kind` |  |
| 6 | `cancelled` | `newport.remote.apps.Lifecycle.Outcome.Cancelled` | oneof `kind` |  |

### `newport.remote.apps.Lifecycle.Outcome.CompletedWithoutPresentation`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `output` | `string` | optional | The end of what the launch wrote. |

### `newport.remote.apps.Lifecycle.Outcome.Failed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `stage` | `newport.remote.apps.Lifecycle.Stage` | implicit |  |
| 2 | `message` | `string` | implicit |  |
| 3 | `outcome_unknown` | `bool` | implicit |  |

### `newport.remote.apps.Lifecycle.Outcome.TimedOut`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `stage` | `newport.remote.apps.Lifecycle.Stage` | implicit |  |
| 2 | `outcome_unknown` | `bool` | implicit |  |

### `newport.remote.apps.Lifecycle.Outcome.Cancelled`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `outcome_unknown` | `bool` | implicit |  |

### `newport.remote.apps.Lifecycle.State`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `starting` | `newport.remote.apps.Lifecycle.Stage` | oneof `kind` |  |
| 2 | `background` | `uint64` | oneof `kind` | The startup budget passed while the launch's processes run, after this many milliseconds. |
| 3 | `window` | `newport.remote.apps.Lifecycle.Window` | oneof `kind` |  |
| 4 | `finished` | `newport.remote.apps.Lifecycle.Outcome` | oneof `kind` |  |

### `newport.remote.apps.Lifecycle.Update`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `request` | `uint64` | implicit |  |
| 3 | `revision` | `uint64` | implicit |  |
| 4 | `state` | `newport.remote.apps.Lifecycle.State` | message presence |  |

### `newport.remote.apps.Lifecycle.Presented`

The client applied the exact offered surface.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `fixed64` | implicit |  |
| 2 | `request` | `uint64` | implicit |  |
| 3 | `window` | `newport.remote.apps.WindowId` | message presence |  |
| 4 | `surface` | `uint64` | implicit |  |

### `newport.remote.apps.Lifecycle.Stage`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `STAGE_PREPARING` |  |
| 1 | `STAGE_SERVICE_LOOKUP` |  |
| 2 | `STAGE_SERVICE_REGISTRATION` |  |
| 3 | `STAGE_ACTIVATION_CALL` |  |
| 4 | `STAGE_COMMAND` |  |
| 5 | `STAGE_AWAITING_WINDOW` |  |
| 6 | `STAGE_AWAITING_PRESENTATION` |  |

### `newport.remote.apps.CodecMode`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CODEC_MODE_AUTO` |  |
| 1 | `CODEC_MODE_ADAPTIVE` |  |
| 2 | `CODEC_MODE_FAST` |  |
| 3 | `CODEC_MODE_LOSSLESS` |  |

### `newport.remote.apps.WindowKind`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `WINDOW_KIND_NORMAL` |  |
| 1 | `WINDOW_KIND_DIALOG` |  |
| 2 | `WINDOW_KIND_MENU` |  |
| 3 | `WINDOW_KIND_TOOLTIP` |  |

### `newport.remote.apps.Decoration`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `DECORATION_CLIENT` |  |
| 1 | `DECORATION_SERVER` |  |
| 2 | `DECORATION_NONE` |  |

### `newport.remote.apps.Codec`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CODEC_SOLID` |  |
| 1 | `CODEC_RAW_BGRA` |  |
| 2 | `CODEC_LZ4` |  |
| 3 | `CODEC_ZLIB` |  |
| 4 | `CODEC_PNG` |  |
| 5 | `CODEC_JPEG` |  |
| 6 | `CODEC_COPY` |  |
| 7 | `CODEC_AV1` |  |

### `newport.remote.apps.ResizeEdge`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `RESIZE_EDGE_TOP` |  |
| 1 | `RESIZE_EDGE_BOTTOM` |  |
| 2 | `RESIZE_EDGE_LEFT` |  |
| 3 | `RESIZE_EDGE_RIGHT` |  |
| 4 | `RESIZE_EDGE_TOP_LEFT` |  |
| 5 | `RESIZE_EDGE_TOP_RIGHT` |  |
| 6 | `RESIZE_EDGE_BOTTOM_LEFT` |  |
| 7 | `RESIZE_EDGE_BOTTOM_RIGHT` |  |

### `newport.remote.apps.MouseButton`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `MOUSE_BUTTON_LEFT` |  |
| 1 | `MOUSE_BUTTON_MIDDLE` |  |
| 2 | `MOUSE_BUTTON_RIGHT` |  |
| 3 | `MOUSE_BUTTON_BACK` |  |
| 4 | `MOUSE_BUTTON_FORWARD` |  |

### `newport.remote.apps.ScrollPhase`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `SCROLL_PHASE_WHEEL` |  |
| 1 | `SCROLL_PHASE_BEGIN` |  |
| 2 | `SCROLL_PHASE_UPDATE` |  |
| 3 | `SCROLL_PHASE_END` |  |
| 4 | `SCROLL_PHASE_MOMENTUM` |  |
| 5 | `SCROLL_PHASE_MOMENTUM_END` |  |

### `newport.remote.apps.CursorShape`

The shapes `wp_cursor_shape_device_v1` names.

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `CURSOR_SHAPE_DEFAULT` |  |
| 1 | `CURSOR_SHAPE_CONTEXT_MENU` |  |
| 2 | `CURSOR_SHAPE_HELP` |  |
| 3 | `CURSOR_SHAPE_POINTER` |  |
| 4 | `CURSOR_SHAPE_PROGRESS` |  |
| 5 | `CURSOR_SHAPE_WAIT` |  |
| 6 | `CURSOR_SHAPE_CELL` |  |
| 7 | `CURSOR_SHAPE_CROSSHAIR` |  |
| 8 | `CURSOR_SHAPE_TEXT` |  |
| 9 | `CURSOR_SHAPE_VERTICAL_TEXT` |  |
| 10 | `CURSOR_SHAPE_ALIAS` |  |
| 11 | `CURSOR_SHAPE_COPY` |  |
| 12 | `CURSOR_SHAPE_MOVE` |  |
| 13 | `CURSOR_SHAPE_NO_DROP` |  |
| 14 | `CURSOR_SHAPE_NOT_ALLOWED` |  |
| 15 | `CURSOR_SHAPE_GRAB` |  |
| 16 | `CURSOR_SHAPE_GRABBING` |  |
| 17 | `CURSOR_SHAPE_E_RESIZE` |  |
| 18 | `CURSOR_SHAPE_N_RESIZE` |  |
| 19 | `CURSOR_SHAPE_NE_RESIZE` |  |
| 20 | `CURSOR_SHAPE_NW_RESIZE` |  |
| 21 | `CURSOR_SHAPE_S_RESIZE` |  |
| 22 | `CURSOR_SHAPE_SE_RESIZE` |  |
| 23 | `CURSOR_SHAPE_SW_RESIZE` |  |
| 24 | `CURSOR_SHAPE_W_RESIZE` |  |
| 25 | `CURSOR_SHAPE_EW_RESIZE` |  |
| 26 | `CURSOR_SHAPE_NS_RESIZE` |  |
| 27 | `CURSOR_SHAPE_NESW_RESIZE` |  |
| 28 | `CURSOR_SHAPE_NWSE_RESIZE` |  |
| 29 | `CURSOR_SHAPE_COL_RESIZE` |  |
| 30 | `CURSOR_SHAPE_ROW_RESIZE` |  |
| 31 | `CURSOR_SHAPE_ALL_SCROLL` |  |
| 32 | `CURSOR_SHAPE_ZOOM_IN` |  |
| 33 | `CURSOR_SHAPE_ZOOM_OUT` |  |

## Schema `newport/remote/catalogue.proto`

Source: [newport/remote/catalogue.proto](proto/newport/remote/catalogue.proto). Package: `newport.remote.catalogue`.

Imports: `newport/wire.proto`, `newport/worker.proto`.

### `newport.remote.catalogue.ClientFrame`

Runtime admission selects catalogue discovery; it has no further input.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 127 | `worker_control` | `newport.worker.ClientControl` | oneof `message` |  |

### `newport.remote.catalogue.ServerFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `catalogue` | `newport.remote.catalogue.Catalogue` | oneof `message` |  |
| 2 | `error` | `newport.wire.Error` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ServerControl` | oneof `message` |  |

### `newport.remote.catalogue.Catalogue`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `apps` | `newport.remote.catalogue.Application` | repeated |  |

### `newport.remote.catalogue.Application`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |
| 2 | `name` | `string` | implicit |  |
| 3 | `description` | `string` | implicit |  |
| 4 | `keywords` | `string` | repeated |  |
| 5 | `icon` | `string` | optional |  |
| 6 | `actions` | `newport.remote.catalogue.DesktopAction` | repeated |  |

### `newport.remote.catalogue.DesktopAction`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |
| 2 | `name` | `string` | implicit |  |

## Schema `newport/remote/display.proto`

Source: [newport/remote/display.proto](proto/newport/remote/display.proto). Package: `newport.remote.display`.

Credential-free display preparation, followed by typed clipboard admission.

Imports: `newport/wire.proto`, `newport/clipboard.proto`.

### `newport.remote.display.Envelope`

The first frame a connection to the display sends.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `op` | `string` | implicit | What the connection asks for: "prepare-clipboard-attach". |
| 2 | `revision` | `uint32` | implicit | The sender's protocol revision. |
| 3 | `build` | `string` | implicit | The sender's build, for diagnostics only. |
| 4 | `body` | `bytes` | implicit | Empty for preparation; no credentials precede Ready. |

### `newport.remote.display.EnvelopeReply`

The display's answer to an `Envelope`, before anything else.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `accepted` | `newport.wire.Present` | oneof `result` |  |
| 2 | `refused` | `newport.remote.display.Refused` | oneof `result` |  |
| 3 | `ready` | `newport.remote.display.DisplayIdentity` | oneof `result` |  |

### `newport.remote.display.Refused`

Why the display refused an envelope: `display-revision` when the sender's
 revision isn't the display's, `unknown-op`, or `refused`.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `error` | `newport.wire.Error` | message presence |  |
| 2 | `display` | `newport.remote.display.DisplayIdentity` | message presence |  |

### `newport.remote.display.DisplayIdentity`

Which display answered, so a peer knows which side is older.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `uint32` | implicit |  |
| 2 | `build` | `string` | implicit |  |

### `newport.remote.display.Attach`

The next frame after Ready, on the same connection. After authentication
 the display answers Accepted or Refused. Only then may Remote Apps follow.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session` | `string` | implicit | The relay's Newport session. |
| 2 | `computer` | `string` | implicit | The name of the relay's computer; empty when Newport didn't give one. |
| 3 | `clipboard` | `newport.clipboard.Access` | message presence | Access issued by the runtime for this worker. |

## Schema `newport/runtime.proto`

Source: [newport/runtime.proto](proto/newport/runtime.proto). Package: `newport.runtime`.

Imports: `newport/wire.proto`, `newport/clipboard.proto`.

### `newport.runtime.ClientFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `request` | `newport.runtime.Request` | oneof `message` |  |

### `newport.runtime.ServerFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `hello` | `newport.runtime.Hello` | oneof `message` |  |
| 2 | `reply` | `newport.runtime.Reply` | oneof `message` |  |

### `newport.runtime.Capability`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `versions` | `uint32` | repeated |  |
| 2 | `unavailable` | `string` | optional |  |
| 3 | `contract_id` | `string` | optional |  |

### `newport.runtime.Limits`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `max_control_bytes` | `uint32` | implicit |  |
| 2 | `max_inflight` | `uint32` | implicit |  |
| 3 | `max_workers` | `uint32` | implicit |  |
| 4 | `max_registrations` | `uint32` | implicit |  |

### `newport.runtime.Hello`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `versions` | `uint32` | repeated |  |
| 2 | `instance_id` | `string` | implicit |  |
| 3 | `build_sha256` | `string` | implicit |  |
| 4 | `worker_socket` | `string` | implicit |  |
| 5 | `product` | `newport.runtime.Product` | implicit |  |
| 6 | `services` | `map<string, newport.runtime.Capability>` | map |  |
| 7 | `dependencies` | `map<string, bool>` | map |  |
| 8 | `limits` | `newport.runtime.Limits` | message presence |  |
| 9 | `agent_version` | `string` | optional |  |

### `newport.runtime.Permissions`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `uint64` | implicit |  |
| 2 | `clipboard` | `bool` | implicit |  |
| 3 | `browser` | `bool` | implicit |  |

### `newport.runtime.Applied`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `permissions` | `newport.runtime.Permissions` | message presence |  |
| 2 | `error` | `newport.wire.Error` | optional |  |

### `newport.runtime.ClipboardRoute`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `uint64` | implicit |  |
| 2 | `worker_id` | `string` | optional |  |
| 3 | `exports` | `bool` | implicit |  |
| 4 | `focused` | `bool` | implicit |  |

### `newport.runtime.RouteReply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `revision` | `uint64` | implicit |  |
| 2 | `worker_id` | `string` | optional |  |
| 3 | `outcome` | `newport.runtime.RouteOutcome` | implicit |  |

### `newport.runtime.Endpoint`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `path` | `string` | implicit |  |
| 2 | `ticket` | `newport.wire.Secret` | message presence |  |

### `newport.runtime.Request`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `runtime_id` | `string` | implicit |  |
| 3 | `initialize` | `newport.runtime.Request.Initialize` | oneof `operation` |  |
| 4 | `prepare_worker_stream` | `newport.runtime.Request.PrepareWorkerStream` | oneof `operation` |  |
| 5 | `permissions` | `newport.runtime.Request.WorkerPermissionUpdate` | oneof `operation` |  |
| 6 | `clipboard_route` | `newport.runtime.ClipboardRoute` | oneof `operation` |  |
| 7 | `ping` | `newport.wire.Present` | oneof `operation` |  |
| 8 | `shutdown` | `newport.wire.Present` | oneof `operation` |  |

### `newport.runtime.Request.Initialize`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | implicit |  |
| 2 | `session_id` | `string` | implicit |  |
| 3 | `transport_generation` | `uint64` | implicit |  |

### `newport.runtime.Request.PrepareWorkerStream`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `worker_id` | `string` | implicit |  |
| 2 | `service` | `string` | implicit |  |
| 3 | `version` | `uint32` | implicit |  |
| 4 | `contract_id` | `string` | optional |  |

### `newport.runtime.Request.WorkerPermissionUpdate`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `worker_id` | `string` | implicit |  |
| 2 | `permissions` | `newport.runtime.Permissions` | message presence |  |

### `newport.runtime.Reply`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `uint64` | implicit |  |
| 2 | `runtime_id` | `string` | implicit |  |
| 3 | `accepted` | `newport.wire.Present` | oneof `result` |  |
| 4 | `worker_stream` | `newport.runtime.Endpoint` | oneof `result` |  |
| 5 | `permissions` | `newport.runtime.Applied` | oneof `result` |  |
| 6 | `clipboard_route` | `newport.runtime.RouteReply` | oneof `result` |  |
| 7 | `error` | `newport.wire.Error` | oneof `result` |  |

### `newport.runtime.LeaseWorkerFrame`

Registration-authenticated local lease channel, independent of service stdin.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `attach` | `newport.runtime.Attach` | oneof `message` |  |
| 2 | `applied` | `newport.runtime.Applied` | oneof `message` |  |

### `newport.runtime.LeaseRuntimeFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `attached` | `newport.runtime.Attached` | oneof `message` |  |
| 2 | `permissions` | `newport.runtime.WorkerPermissions` | oneof `message` |  |

### `newport.runtime.Attach`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `runtime_id` | `string` | implicit |  |
| 2 | `worker_id` | `string` | implicit |  |
| 3 | `build_sha256` | `string` | implicit |  |
| 4 | `service` | `string` | implicit |  |

### `newport.runtime.Lease`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `session_id` | `string` | implicit |  |
| 2 | `clipboard` | `newport.clipboard.Access` | optional |  |

### `newport.runtime.Attached`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `runtime_id` | `string` | implicit |  |
| 2 | `worker_id` | `string` | implicit |  |
| 3 | `lease` | `newport.runtime.Lease` | oneof `result` |  |
| 4 | `error` | `newport.wire.Error` | oneof `result` |  |

### `newport.runtime.WorkerPermissions`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `runtime_id` | `string` | implicit |  |
| 2 | `worker_id` | `string` | implicit |  |
| 3 | `permissions` | `newport.runtime.Permissions` | message presence |  |

### `newport.runtime.AdmissionClientFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `open` | `newport.runtime.Open` | oneof `message` |  |

### `newport.runtime.AdmissionServerFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `started` | `newport.runtime.Started` | oneof `result` |  |
| 2 | `error` | `newport.wire.Error` | oneof `result` |  |

### `newport.runtime.Open`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | implicit |  |
| 2 | `runtime_id` | `string` | implicit |  |
| 3 | `worker_id` | `string` | implicit |  |
| 4 | `build_sha256` | `string` | implicit |  |
| 5 | `ticket` | `newport.wire.Secret` | message presence |  |
| 6 | `service` | `string` | implicit |  |
| 7 | `service_version` | `uint32` | implicit |  |
| 8 | `arguments` | `string` | repeated |  |
| 9 | `contract_id` | `string` | optional |  |

### `newport.runtime.Started`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `runtime_id` | `string` | implicit |  |
| 2 | `worker_id` | `string` | implicit |  |
| 3 | `build_sha256` | `string` | implicit |  |

### `newport.runtime.Product`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `PRODUCT_UNKNOWN` |  |
| 1 | `PRODUCT_CORE` |  |
| 2 | `PRODUCT_FULL` |  |

### `newport.runtime.RouteOutcome`

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `ROUTE_OUTCOME_UNKNOWN` |  |
| 1 | `ROUTE_OUTCOME_APPLIED` |  |
| 2 | `ROUTE_OUTCOME_RETIRED` |  |

## Schema `newport/terminal.proto`

Source: [newport/terminal.proto](proto/newport/terminal.proto). Package: `newport.terminal`.

Imports: `newport/wire.proto`, `newport/worker.proto`.

### `newport.terminal.Computer`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |
| 2 | `name` | `string` | implicit |  |

### `newport.terminal.Attached`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |
| 2 | `name` | `string` | implicit |  |
| 3 | `active` | `bool` | implicit |  |

### `newport.terminal.Exit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `code` | `uint32` | oneof `outcome` |  |
| 2 | `signal` | `uint32` | oneof `outcome` |  |

### `newport.terminal.Holder`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |
| 2 | `socket` | `string` | implicit |  |
| 3 | `version` | `uint32` | implicit |  |
| 4 | `columns` | `uint32` | implicit |  |
| 5 | `rows` | `uint32` | implicit |  |
| 6 | `started` | `uint64` | implicit |  |
| 7 | `name` | `string` | optional |  |
| 8 | `title` | `string` | optional |  |
| 9 | `creator` | `newport.terminal.Computer` | message presence |  |
| 10 | `attached` | `newport.terminal.Attached` | repeated |  |
| 11 | `exited` | `newport.terminal.Exit` | message presence |  |
| 12 | `agent` | `string` | optional |  |

### `newport.terminal.Start`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |
| 2 | `columns` | `uint32` | implicit |  |
| 3 | `rows` | `uint32` | implicit |  |
| 4 | `command` | `string` | implicit |  |
| 5 | `creator` | `newport.terminal.Computer` | message presence |  |
| 6 | `stable_display` | `bool` | implicit |  |

### `newport.terminal.Rename`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |
| 2 | `name` | `string` | implicit |  |

### `newport.terminal.End`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `id` | `string` | implicit |  |

### `newport.terminal.Started`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `socket` | `string` | implicit |  |

### `newport.terminal.Listed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `holders` | `newport.terminal.Holder` | repeated |  |

### `newport.terminal.Renamed`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `renamed` | `bool` | implicit |  |

### `newport.terminal.Ended`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `ended` | `bool` | implicit |  |

### `newport.terminal.ClientFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `start` | `newport.terminal.Start` | oneof `message` |  |
| 2 | `list` | `newport.wire.Present` | oneof `message` |  |
| 3 | `rename` | `newport.terminal.Rename` | oneof `message` |  |
| 4 | `end` | `newport.terminal.End` | oneof `message` |  |
| 5 | `watch` | `newport.wire.Present` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ClientControl` | oneof `message` |  |

### `newport.terminal.ServerFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `started` | `newport.terminal.Started` | oneof `message` |  |
| 2 | `listed` | `newport.terminal.Listed` | oneof `message` |  |
| 3 | `renamed` | `newport.terminal.Renamed` | oneof `message` |  |
| 4 | `ended` | `newport.terminal.Ended` | oneof `message` |  |
| 5 | `error` | `newport.wire.Error` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ServerControl` | oneof `message` |  |

## Schema `newport/terminal_holder.proto`

Source: [newport/terminal_holder.proto](proto/newport/terminal_holder.proto). Package: `newport.terminal.holder`.

Imports: `newport/wire.proto`, `newport/terminal.proto`.

### `newport.terminal.holder.Hello`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | implicit |  |
| 2 | `terminal_id` | `string` | implicit |  |

### `newport.terminal.holder.Size`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `columns` | `uint32` | implicit |  |
| 2 | `rows` | `uint32` | implicit |  |

### `newport.terminal.holder.Colors`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `foreground` | `uint32` | implicit |  |
| 2 | `background` | `uint32` | implicit |  |
| 3 | `cursor` | `uint32` | implicit |  |

### `newport.terminal.holder.Attach`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `version` | `uint32` | implicit |  |
| 2 | `size` | `newport.terminal.holder.Size` | message presence |  |
| 3 | `control` | `bool` | implicit |  |
| 4 | `opened` | `bool` | implicit |  |
| 5 | `computer` | `newport.terminal.Computer` | message presence |  |
| 6 | `colors` | `newport.terminal.holder.Colors` | message presence |  |

### `newport.terminal.holder.Input`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `data` | `bytes` | implicit |  |

### `newport.terminal.holder.Rename`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | implicit |  |

### `newport.terminal.holder.Renamed`

The normalized name was persisted by this control connection.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `name` | `string` | implicit |  |

### `newport.terminal.holder.Attached`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `prompted` | `bool` | implicit |  |

### `newport.terminal.holder.Screen`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `data` | `bytes` | implicit |  |

### `newport.terminal.holder.FullSnapshot`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `deflate` | `bytes` | implicit |  |

### `newport.terminal.holder.Output`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `data` | `bytes` | implicit |  |

### `newport.terminal.holder.Active`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `active` | `bool` | implicit |  |

### `newport.terminal.holder.ClientFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `attach` | `newport.terminal.holder.Attach` | oneof `message` |  |
| 2 | `input` | `newport.terminal.holder.Input` | oneof `message` |  |
| 3 | `resize` | `newport.terminal.holder.Size` | oneof `message` |  |
| 4 | `end` | `newport.wire.Present` | oneof `message` |  |
| 5 | `rename` | `newport.terminal.holder.Rename` | oneof `message` |  |
| 6 | `activate` | `newport.wire.Present` | oneof `message` |  |

### `newport.terminal.holder.ServerFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `hello` | `newport.terminal.holder.Hello` | oneof `message` |  |
| 2 | `attached` | `newport.terminal.holder.Attached` | oneof `message` |  |
| 3 | `screen` | `newport.terminal.holder.Screen` | oneof `message` |  |
| 4 | `full` | `newport.terminal.holder.FullSnapshot` | oneof `message` |  |
| 5 | `output` | `newport.terminal.holder.Output` | oneof `message` |  |
| 6 | `exit` | `newport.terminal.Exit` | oneof `message` |  |
| 7 | `active` | `newport.terminal.holder.Active` | oneof `message` |  |
| 8 | `error` | `newport.wire.Error` | oneof `message` |  |
| 9 | `renamed` | `newport.terminal.holder.Renamed` | oneof `message` |  |
| 10 | `size` | `newport.terminal.holder.Size` | oneof `message` | The shell's size, before the screen at attach and whenever it changes: every attached tab draws the shell at this size. |

## Schema `newport/wire.proto`

Source: [newport/wire.proto](proto/newport/wire.proto). Package: `newport.wire`.

Messages every Newport protocol may use.

### `newport.wire.Error`

Why a request failed.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `code` | `string` | implicit | busy, refused, too_large, not_found, unsupported, failed, or the protocol's own. |
| 2 | `message` | `string` | implicit | For people. |
| 3 | `retry` | `newport.wire.Retry` | implicit |  |

### `newport.wire.Present`

A feature with no parameters.

Empty message.

### `newport.wire.Secret`

A value redacted in Debug output and in traces.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `value` | `bytes` | implicit |  |

### `newport.wire.Retry`

Whether the same request may succeed if sent again.

| Number | Value | Schema comment |
| --- | --- | --- |
| 0 | `RETRY_UNKNOWN` |  |
| 1 | `RETRY_YES` |  |
| 2 | `RETRY_NO` |  |

## Schema `newport/worker.proto`

Source: [newport/worker.proto](proto/newport/worker.proto). Package: `newport.worker`.

Imports: `newport/wire.proto`.

### `newport.worker.ClientControl`

Each carried service reserves field 127 of its directional oneof for
 these controls. The runtime consumes them before dispatching service data.
 Operation tags are disjoint across directions to detect misrouted controls.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `input_complete` | `newport.wire.Present` | oneof `operation` |  |
| 2 | `signal` | `uint32` | oneof `operation` |  |

### `newport.worker.ServerControl`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 3 | `stderr` | `bytes` | oneof `operation` |  |
| 4 | `exit` | `newport.worker.Exit` | oneof `operation` |  |

### `newport.worker.Exit`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `status` | `uint32` | oneof `outcome` |  |
| 2 | `signal` | `uint32` | oneof `outcome` |  |

### `newport.worker.OpaqueClientFrame`

Only registered external byte streams use these messages. Their bytes
 retain the external protocol's framing, including partial headers.

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `data` | `bytes` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ClientControl` | oneof `message` |  |

### `newport.worker.OpaqueServerFrame`

| Tag | Field | Type | Presence | Schema comment |
| --- | --- | --- | --- | --- |
| 1 | `data` | `bytes` | oneof `message` |  |
| 127 | `worker_control` | `newport.worker.ServerControl` | oneof `message` |  |
