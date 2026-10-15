# Newport domain

Shared application models and deterministic feature rules used by the backend,
client, terminal, and UI. This crate has no dependency on another Newport crate.

Keep code here when multiple layers need the same application concept or rule:
server and tunnel configuration, connection identities, forwarding validation,
port grouping, container/service models, Compose state, Git operation outcomes,
and the policy for accepting remote links.

Keep feature-specific code with its owning feature until it needs to be shared.
Do not put UI state, IO, persistence, SSH execution, runtime ownership, generic
utilities, or transport contracts here. Desktop framing and SSH-agent relay
contracts live in `newport-platform::ipc::{protocol, agent_relay}` alongside the
local transport implementation.

Previously named `newport-core`; no compatibility alias is retained.
