# Newport protocol

Shared Rust contracts and validation for Newport components. This public crate
has no application, UI, OS-adapter or private-repository dependency.

The [protocol reference](REFERENCE.md) describes framing, handshakes and every
compiled schema's messages, fields and comments. From the repository root, run
`cargo xtask protocol-reference` to regenerate it, or add `--check` to detect
stale documentation without rewriting it. The generator uses the same protox
descriptor set as Rust code generation and needs no system `protoc`. Edit
`reference-introduction.md` for the handshakes and `.proto` comments for schema
descriptions; do not edit the generated reference directly.

- `common`: shared errors, retry decisions, secrets and limits.
- `product`: build identity, product variant and compiled protocol versions.
- `runtime`: product capabilities, runtime identity and numbered control requests.
- `worker`: admission, permissions and transport controls for carried services.
- `clipboard`: runtime-scoped offers, routes and copy leases;
  `clipboard::imports` owns revision-tagged host data, and `clipboard::exports`
  owns native export commands, events and validation. `clipboard::source`
  carries local reads; `clipboard::helper` and `clipboard::provider` describe
  native clipboard worker messages without depending on native OS APIs.
- `desktop`: typed local application requests, replies and streams. Operation
  families cover host commands, accounts, configuration, connections, events,
  files, Git, metrics, services, containers, applications and terminals.
- `integration`: permissions, clipboard traffic, browser requests, GUI notices
  and heartbeats with correlated calls and responses.
- `browser`: local helper admission, validated web URLs and opening outcomes.
- `terminal`: holder discovery, lifecycle requests and watch results;
  `terminal::holder` owns the versioned holder attachment contract.
- `monitor`: metrics, services, containers, Compose, ports, display ownership and
  file watches, with numbered calls and operation-specific result validation.
- `git`: repository requests, operation-specific results, durable receipts,
  pagination, binary content and acknowledged diff streams.
- `display::environment`: environment names, values and validation.
- `display::attachment`: display admission, identities and attachment endpoints.
- `remote_apps`: messages, feature grants, geometry, input, audio and file trees;
  `remote_apps::apps` owns desktop catalogue entries, actions and optional icons.
- `encoding::protobuf`: bounded frames, checked decoding and descriptors.

Default features are empty. A contracts-only consumer uses the semantic values
without protobuf or schema compilation. Enable `protobuf` at byte-stream
boundaries, `tokio` for async framing, or `trace` for descriptor-driven traces.
The latter two imply `protobuf`. Enable `compression` for bounded clipboard
zlib chunks; it is independent of protobuf. Schema generators are build
dependencies only.

Service-specific protobuf representations are internal. Public encode/decode
functions use semantic Rust messages; direct Rust calls need no serialization.
The low-level generic codec exposes common protobuf primitives in its explicit
encoding namespace. The display transport preserves shared `bytes::Bytes` payloads.

Schemas live in `proto/newport`. Rust module names do not determine protobuf
package identities: `newport.wire` and `newport.remote.*` retain their field
numbers and compatibility rules. Structural validation belongs here; session
permissions, resource ownership and execution belong to the consuming runtime.
Application models and persisted-profile rules belong in `newport-domain`.
Neither crate depends on the other.

Desktop operations use semantic Rust requests and replies. Generated protobuf
records remain inside `desktop::wire`; descriptor-derived structural checks
reject duplicate known fields, ambiguous operations and excessive collections
before record allocation. The consuming IPC adapter owns OS authentication,
admission limits, deadlines and execution. UI projections happen after decoding.

The monitor codec checks field, collection and text budgets before allocating
records. It preserves optional values, skips unknown metadata, and rejects
duplicate known operations, mismatched response identifiers and worker controls
at a direct service endpoint. Consumers own feature scheduling and retry policy.

Catalogue discovery returns one typed catalogue or shared error through
`remote_apps::apps::wire`. Its decoder checks collection and field bounds before
allocation and rejects duplicate results, duplicate application IDs and trailing
output. The semantic byte budget reserves metadata before icons without requiring
protobuf; the desktop projects validated records into its UI data model.

`integration::wire` carries typed calls, responses and unsolicited events.
Runtime admission owns service negotiation; the integration channel uses a
correlated heartbeat for readiness. Export records are nested protobuf messages.
The decoder bounds collections, text, archives and chunks before allocation and
rejects duplicate fields, ambiguous operations and invalid scalar values.
Clipboard contracts preserve absent offers, rejected copies and failed reads
separately from empty completed data. Consumers own focus, permission epochs,
revocation, cancellation and read scheduling.

`git::wire` encodes directional messages and typed diff bodies. Runtime admission
selects the service; configuration binds the client's durable journal identity.
Validation checks field presence, bounded collections, binary content, response
identity and operation receipts. Diff chunks retain acknowledgement flow control.
The semantic request serialization preserves durable journal hashes, while
`Reply::projection` provides the UI's requested diff-line representation.

`worker::Transport` declares each service’s framing at registration. Native
protobuf service frames retain their original bytes through the relay; worker
controls share the stream and are forbidden inside child output. Explicit opaque
transports carry external protocols in bounded data chunks. Readers and writers
retain partial frames, enforce direction budgets and reject incomplete shutdowns.

From the Newport root, run `cargo xtask test rust -p newport-protocol
--all-features` for the complete protocol suite. `cargo xtask check` also builds
an isolated contracts-only consumer and rejects protobuf/codegen dependencies
in its normal/build graph.
