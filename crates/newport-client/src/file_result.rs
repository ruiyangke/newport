//! Explicit file-result projections used by the existing render state.
use base64::Engine;
use newport_protocol::desktop::files;
use serde_json::{Value, json};

pub fn project(reply: files::Reply) -> Result<Value, String> {
    Ok(match reply {
        files::Reply::List(value) => {
            let entries: Vec<_> = value.entries.into_iter().map(|value| {
                let kind = match value.kind { files::EntryKind::Directory=>"directory",files::EntryKind::Symlink=>"symlink",files::EntryKind::File=>"file",files::EntryKind::Other=>"other" };
                json!({"name":value.name,"path":value.path,"kind":kind,"size":value.size,"modified":value.modified,"permissions":value.permissions})
            }).collect();
            json!({"path":value.path,"entries":entries})
        }
        files::Reply::Preview(value) => {
            let kind = match value.kind {
                files::PreviewKind::Text => "text",
                files::PreviewKind::Image => "image",
            };
            let content = match value.content {
                files::Content::Text(value) => *value,
                files::Content::Binary(value) => {
                    base64::engine::general_purpose::STANDARD.encode(value.as_ref())
                }
            };
            json!({"kind":kind,"mime":value.mime,"content":content,"truncated":value.truncated})
        }
        files::Reply::ReadText(value) | files::Reply::WriteText(value) => {
            let page = value.page.map(|value| {
                let reason = match value.reason {
                    files::PageReason::Size => "size",
                    files::PageReason::LongLines => "long_lines",
                };
                json!({"offset":value.offset,"next":value.next,"size":value.size,"reason":reason})
            });
            json!({"path":value.path,"content":value.content,"revision":value.revision,"page":page,"cached":value.cached})
        }
        files::Reply::ProjectIndex(value) => json!({"paths":value.paths,"skipped":value.skipped}),
        files::Reply::Watch(value) => {
            let mode = match value.mode {
                files::WatchMode::Native => "native",
                files::WatchMode::Poll => "poll",
            };
            json!({"mode":mode,"cursor":value.cursor,"paths":value.paths,"reset":value.reset})
        }
        files::Reply::Cancel(_) => Value::Null,
        files::Reply::Progress(value) => value
            .progress
            .map(|value| json!({"name":value.name,"completed":value.completed,"total":value.total}))
            .unwrap_or(Value::Null),
        files::Reply::Upload(_) | files::Reply::Download(_) => {
            return Err("File transfer paths require the native transfer adapter".into());
        }
    })
}
