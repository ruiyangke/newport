//! Framework-independent Newport domain types and rules.
//! Shared models, validation, and deterministic feature rules belong here.
//! UI, IO, transport contracts, runtime ownership, and generic utilities do not.
//! Desktop IPC contracts live in `newport_platform::ipc`.
pub mod compose;
pub mod connectivity;
pub mod container_status;
pub mod containers;
pub mod containers_log;
pub mod external_link;
pub mod forwarding;
pub mod forwarding_batch;
pub mod git_operation;
pub mod model;
pub mod port_context;
pub mod port_discovery;
pub mod port_grouping;
pub mod remote_app_identity;
pub mod remote_app_registry;
pub mod remote_app_shortcuts;
pub mod requirements;
pub mod server_os;
pub mod services;
pub mod terminal;
pub mod text_file;
pub mod windows_file_names;
