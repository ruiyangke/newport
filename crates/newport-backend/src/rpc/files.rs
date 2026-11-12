//! File DTOs are independent of SFTP and persisted editor/cache models.
use super::{
    configuration,
    failure::{invalid_result, operation},
    features::Services,
    projection as p,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use newport_protocol::{
    common::Present,
    desktop::{Failure, files as dto},
};
use std::ffi::OsString;

fn scope(
    value: &newport_protocol::desktop::configuration::ConnectionScope,
) -> Result<Vec<serde_json::Value>, Failure> {
    match configuration::scope(value) {
        serde_json::Value::Array(values) => Ok(values),
        _ => Err(invalid_result("connection scope")),
    }
}
fn listing(value: crate::files::Listing) -> Result<dto::Listing, Failure> {
    Ok(dto::Listing {
        path: value.path,
        entries: value
            .entries
            .into_iter()
            .map(|entry| {
                Ok(dto::Entry {
                    name: entry.name,
                    path: entry.path,
                    kind: match entry.kind {
                        "directory" => dto::EntryKind::Directory,
                        "symlink" => dto::EntryKind::Symlink,
                        "file" => dto::EntryKind::File,
                        "other" => dto::EntryKind::Other,
                        _ => return Err(invalid_result("entry kind")),
                    },
                    size: entry.size,
                    modified: entry.modified,
                    permissions: entry.permissions,
                })
            })
            .collect::<Result<_, _>>()?,
    })
}
fn preview(value: crate::files::Preview) -> Result<dto::Preview, Failure> {
    let (kind, content) = match value.kind {
        "text" => (
            dto::PreviewKind::Text,
            dto::Content::Text(Box::new(value.content)),
        ),
        "image" => (
            dto::PreviewKind::Image,
            dto::Content::Binary(Box::new(
                STANDARD
                    .decode(value.content)
                    .map_err(|_| invalid_result("preview image"))?
                    .into(),
            )),
        ),
        _ => return Err(invalid_result("preview kind")),
    };
    Ok(dto::Preview {
        kind,
        content,
        mime: value.mime.into(),
        truncated: value.truncated,
    })
}
fn document(value: crate::files::editor::Document) -> Result<dto::Document, Failure> {
    Ok(dto::Document {
        path: value.path,
        content: value.content,
        revision: value.revision,
        cached: value.cached,
        page: value
            .page
            .map(|page| {
                Ok(dto::Page {
                    offset: page.offset,
                    next: page.next,
                    size: page.size,
                    reason: match page.reason {
                        "size" => dto::PageReason::Size,
                        "long_lines" => dto::PageReason::LongLines,
                        _ => return Err(invalid_result("document page reason")),
                    },
                })
            })
            .transpose()?,
    })
}
fn native(value: &dto::NativePath) -> Result<OsString, Failure> {
    match value {
        #[cfg(unix)]
        dto::NativePath::Unix(bytes) => {
            use std::os::unix::ffi::OsStringExt;
            Ok(OsString::from_vec(bytes.to_vec()))
        }
        #[cfg(windows)]
        dto::NativePath::Windows(units) => {
            use std::os::windows::ffi::OsStringExt;
            Ok(OsString::from_wide(units))
        }
        _ => Err(operation(
            "Native file path belongs to another operating system",
        )),
    }
}

pub async fn dispatch(
    command: &dto::Operation,
    state: &Services<'_>,
) -> Result<dto::Reply, Failure> {
    use dto::{Operation as O, Reply as R};
    Ok(match command {
        O::List(args) => R::List(Box::new(listing(
            crate::files::files_list_query(
                state.manager,
                state.files,
                args.server,
                args.operation,
                args.path.clone(),
                args.expected.as_ref().map(configuration::scope),
                args.refresh,
                args.project_root.clone(),
            )
            .await
            .map_err(operation)?,
        )?)),
        O::Watch(args) => {
            let value = crate::files::vfs::dispatch(
                state.manager,
                state.files,
                crate::files::vfs::Request {
                    id: args.server,
                    operation: args.operation,
                    expected_connection: scope(&args.expected)?,
                    root: args.root.clone(),
                    watch: args.watch,
                    directories: args.directories.iter().cloned().collect(),
                    cursor: args.cursor.clone(),
                },
            )
            .await
            .map_err(operation)?;
            R::Watch(Box::new(dto::WatchReply {
                mode: match p::text(p::field(&value, "mode")?)?.as_str() {
                    "native" => dto::WatchMode::Native,
                    "poll" => dto::WatchMode::Poll,
                    _ => return Err(invalid_result("watch mode")),
                },
                cursor: p::optional(&value, "cursor", p::text)?,
                paths: p::list(p::field(&value, "paths")?, p::text)?,
                reset: p::bool_value(p::field(&value, "reset")?)?,
            }))
        }
        O::Preview(args) => R::Preview(Box::new(preview(
            crate::files::files_preview(
                state.manager,
                state.files,
                args.server,
                args.operation,
                args.path.clone(),
                args.expected.as_ref().map(configuration::scope),
            )
            .await
            .map_err(operation)?,
        )?)),
        O::ReadText(args) => R::ReadText(Box::new(document(
            crate::files::editor::dispatch(
                state.manager,
                state.files,
                crate::files::editor::Request {
                    id: args.server,
                    operation: args.operation,
                    root: args.root.clone(),
                    path: args.path.clone(),
                    expected_connection: scope(&args.expected)?,
                    content: None,
                    revision: args.revision.clone(),
                    offset: args.offset,
                    allow_cached: args.allow_cached,
                },
                false,
            )
            .await
            .map_err(operation)?,
        )?)),
        O::WriteText(args) => R::WriteText(Box::new(document(
            crate::files::editor::dispatch(
                state.manager,
                state.files,
                crate::files::editor::Request {
                    id: args.server,
                    operation: args.operation,
                    root: args.root.clone(),
                    path: args.path.clone(),
                    expected_connection: scope(&args.expected)?,
                    content: Some(args.content.clone()),
                    revision: Some(args.revision.clone()),
                    offset: None,
                    allow_cached: false,
                },
                true,
            )
            .await
            .map_err(operation)?,
        )?)),
        O::ProjectIndex(args) => {
            let value = crate::files::project_index::dispatch(
                state.manager,
                state.files,
                crate::files::project_index::Request {
                    id: args.server,
                    operation: args.operation,
                    root: args.root.clone(),
                    expected_connection: scope(&args.expected)?,
                },
            )
            .await
            .map_err(operation)?;
            R::ProjectIndex(Box::new(dto::Index {
                paths: value.paths,
                skipped: u64::try_from(value.skipped)
                    .map_err(|_| invalid_result("index skipped count"))?,
            }))
        }
        O::Cancel(args) => {
            crate::files::operations::files_cancel(state.files, args.operation);
            R::Cancel(Box::new(Present {}))
        }
        O::Progress(args) => R::Progress(Box::new(dto::ProgressLookup {
            progress: crate::files::operations::files_progress(state.files, args.operation).map(
                |value| dto::TransferProgress {
                    name: value.name,
                    completed: value.completed,
                    total: value.total,
                },
            ),
        })),
        O::Upload(args) => {
            let result = crate::files::transfer::native_transfer(
                state.manager,
                state.files,
                crate::files::transfer::NativeTransfer {
                    id: args.server,
                    operation: args.operation,
                    path: args.path.clone(),
                    local_path: native(&args.local_path)?,
                    expected_connection: scope(&args.expected)?,
                },
                true,
            )
            .await
            .map_err(operation)?;
            R::Upload(Box::new(dto::TransferResult {
                path: result.map(|value| dto::TransferPath::Remote(Box::new(value))),
            }))
        }
        O::Download(args) => {
            let result = crate::files::transfer::native_transfer(
                state.manager,
                state.files,
                crate::files::transfer::NativeTransfer {
                    id: args.server,
                    operation: args.operation,
                    path: args.path.clone(),
                    local_path: native(&args.local_path)?,
                    expected_connection: scope(&args.expected)?,
                },
                false,
            )
            .await
            .map_err(operation)?;
            // The operation wrote this exact selected path; do not return its
            // legacy lossy display string over the native transport.
            R::Download(Box::new(dto::TransferResult {
                path: result.map(|_| dto::TransferPath::Local(Box::new(args.local_path.clone()))),
            }))
        }
    })
}
