//! Desktop backend client, independent of any UI framework.
//! Owns IPC, operation dispatch, and terminal session transport.
pub mod account;
pub mod appearance;
pub mod backend;
pub mod compose_activity;
pub mod container_ports;
pub mod containers;
pub mod containers_stats;
pub mod file_browser;
pub mod file_transfer;
pub mod git_blob;
pub mod git_branch;
pub mod git_configuration;
pub mod git_conflict;
pub mod git_diff;
pub mod git_diff_page;
pub mod git_discard;
pub mod git_history;
pub mod git_page;
pub mod git_read_recovery;
pub mod git_remote_refs;
pub mod git_remotes;
pub mod git_selection;
pub mod lsp;
pub mod profile;
pub mod resources;
pub mod services;
pub mod settings;
pub mod terminal_command;
pub mod terminal_transport;
pub mod tunnel_operations;
pub mod window_preferences;
pub use newport_domain::git_operation;

pub mod git_checkout_recovery;
pub mod git_checkout_status;
pub mod git_worktree;

pub mod project_bookmark;

pub mod project_creation;

pub mod git_clean;

pub mod project_checks;

pub mod git_sync;
pub mod request_error;
