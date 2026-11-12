use super::*;
impl Convert for crate::desktop::files::EntryKind {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Directory => 1,
            Self::Symlink => 2,
            Self::File => 3,
            Self::Other => 4,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Directory,
            2 => Self::Symlink,
            3 => Self::File,
            4 => Self::Other,
            _ => return Err(Invalid("EntryKind").into()),
        })
    }
}

impl Convert for crate::desktop::files::Entry {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::Entry;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::Entry {
                name: Some(self.name.clone()),
                path: Some(self.path.clone()),
                kind: Some(self.kind.to_proto()?),
                size: self
                    .size
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                modified: self
                    .modified
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                permissions: self
                    .permissions
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            name: required(value.name)?,
            path: required(value.path)?,
            kind: <crate::desktop::files::EntryKind as Convert>::from_proto(required(value.kind)?)?,
            size: value
                .size
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            modified: value
                .modified
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            permissions: value
                .permissions
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::files::Listing {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::Listing;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::Listing {
                path: Some(self.path.clone()),
                entries: self
                    .entries
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            path: required(value.path)?,
            entries: value
                .entries
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::files::Entry as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::files::PreviewKind {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Text => 1,
            Self::Image => 2,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Text,
            2 => Self::Image,
            _ => return Err(Invalid("PreviewKind").into()),
        })
    }
}

impl Convert for crate::desktop::files::Content {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::Content;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::files::Content { value: Some(match self { crate::desktop::files::Content::Text(v) => crate::encoding::protobuf::generated::newport::desktop::files::content::Value::Text(bytes::Bytes::copy_from_slice(v.as_ref().as_bytes())), crate::desktop::files::Content::Binary(v) => crate::encoding::protobuf::generated::newport::desktop::files::content::Value::Binary(v.as_ref().clone()) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::files::content::Value::Text(v) => crate::desktop::files::Content::Text(Box::new(String::from_utf8(v.to_vec()).map_err(Error::Utf8)?)), crate::encoding::protobuf::generated::newport::desktop::files::content::Value::Binary(v) => crate::desktop::files::Content::Binary(Box::new(v)) })
    }
}

impl Convert for crate::desktop::files::Preview {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::Preview;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::Preview {
                kind: Some(self.kind.to_proto()?),
                mime: Some(self.mime.clone()),
                content: Some(self.content.to_proto()?),
                truncated: Some(self.truncated),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            kind: <crate::desktop::files::PreviewKind as Convert>::from_proto(required(
                value.kind,
            )?)?,
            mime: required(value.mime)?,
            content: <crate::desktop::files::Content as Convert>::from_proto(required(
                value.content,
            )?)?,
            truncated: required(value.truncated)?,
        })
    }
}

impl Convert for crate::desktop::files::PageReason {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Size => 1,
            Self::LongLines => 2,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Size,
            2 => Self::LongLines,
            _ => return Err(Invalid("PageReason").into()),
        })
    }
}

impl Convert for crate::desktop::files::Page {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::Page;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::Page {
                offset: Some(self.offset),
                next: self
                    .next
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                size: Some(self.size),
                reason: Some(self.reason.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            offset: required(value.offset)?,
            next: value
                .next
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            size: required(value.size)?,
            reason: <crate::desktop::files::PageReason as Convert>::from_proto(required(
                value.reason,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::files::Document {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::Document;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::Document {
                path: Some(self.path.clone()),
                content: Some(bytes::Bytes::copy_from_slice(self.content.as_bytes())),
                revision: Some(self.revision.clone()),
                page: self
                    .page
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                cached: Some(self.cached),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            path: required(value.path)?,
            content: String::from_utf8(required(value.content)?.to_vec()).map_err(Error::Utf8)?,
            revision: required(value.revision)?,
            page: value
                .page
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::files::Page as Convert>::from_proto(v)
                })
                .transpose()?,
            cached: required(value.cached)?,
        })
    }
}

impl Convert for crate::desktop::files::Index {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::Index;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::Index {
                paths: self
                    .paths
                    .iter()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .collect::<Result<_, _>>()?,
                skipped: Some(self.skipped),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            paths: value
                .paths
                .into_iter()
                .map(|v| -> Result<_, Error> { Ok(v) })
                .collect::<Result<_, _>>()?,
            skipped: required(value.skipped)?,
        })
    }
}

impl Convert for crate::desktop::files::WatchMode {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Native => 1,
            Self::Poll => 2,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Native,
            2 => Self::Poll,
            _ => return Err(Invalid("WatchMode").into()),
        })
    }
}

impl Convert for crate::desktop::files::WatchReply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::WatchReply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::WatchReply {
                mode: Some(self.mode.to_proto()?),
                cursor: self
                    .cursor
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                paths: self
                    .paths
                    .iter()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .collect::<Result<_, _>>()?,
                reset: Some(self.reset),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            mode: <crate::desktop::files::WatchMode as Convert>::from_proto(required(value.mode)?)?,
            cursor: value
                .cursor
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            paths: value
                .paths
                .into_iter()
                .map(|v| -> Result<_, Error> { Ok(v) })
                .collect::<Result<_, _>>()?,
            reset: required(value.reset)?,
        })
    }
}

impl Convert for crate::desktop::files::TransferProgress {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::TransferProgress;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::TransferProgress {
                name: Some(self.name.clone()),
                completed: Some(self.completed),
                total: self
                    .total
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            name: required(value.name)?,
            completed: required(value.completed)?,
            total: value
                .total
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::files::ProgressLookup {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::ProgressLookup;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::ProgressLookup {
                progress: self
                    .progress
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            progress: value
                .progress
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::files::TransferProgress as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::files::NativePath {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::NativePath;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::files::NativePath { value: Some(match self { crate::desktop::files::NativePath::Unix(v) => crate::encoding::protobuf::generated::newport::desktop::files::native_path::Value::Unix(v.as_ref().clone()), crate::desktop::files::NativePath::Windows(v) => crate::encoding::protobuf::generated::newport::desktop::files::native_path::Value::Windows(bytes::Bytes::from(v.as_ref().iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>())) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::files::native_path::Value::Unix(v) => crate::desktop::files::NativePath::Unix(Box::new(v)), crate::encoding::protobuf::generated::newport::desktop::files::native_path::Value::Windows(v) => crate::desktop::files::NativePath::Windows(Box::new(utf16(v)?)) })
    }
}

impl Convert for crate::desktop::files::TransferPath {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::TransferPath;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::files::TransferPath { value: Some(match self { crate::desktop::files::TransferPath::Remote(v) => crate::encoding::protobuf::generated::newport::desktop::files::transfer_path::Value::Remote(v.as_ref().clone()), crate::desktop::files::TransferPath::Local(v) => crate::encoding::protobuf::generated::newport::desktop::files::transfer_path::Value::Local(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::files::transfer_path::Value::Remote(v) => crate::desktop::files::TransferPath::Remote(Box::new(v)), crate::encoding::protobuf::generated::newport::desktop::files::transfer_path::Value::Local(v) => crate::desktop::files::TransferPath::Local(Box::new(<crate::desktop::files::NativePath as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::files::TransferResult {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::TransferResult;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::TransferResult {
                path: self
                    .path
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            path: value
                .path
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::files::TransferPath as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::files::ListArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::ListArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::ListArgs {
                server: Some(self.server.to_string()),
                operation: Some(self.operation.to_string()),
                path: Some(self.path.clone()),
                expected: self
                    .expected
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                refresh: Some(self.refresh),
                project_root: self
                    .project_root
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            operation: uuid(required(value.operation)?)?,
            path: required(value.path)?,
            expected: value
                .expected
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(v)
                })
                .transpose()?,
            refresh: value.refresh.unwrap_or(false),
            project_root: value
                .project_root
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::files::WatchArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::WatchArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::WatchArgs {
                server: Some(self.server.to_string()),
                operation: Some(self.operation.to_string()),
                expected: Some(self.expected.to_proto()?),
                root: Some(self.root.clone()),
                watch: Some(self.watch.to_string()),
                directories: self
                    .directories
                    .iter()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .collect::<Result<_, _>>()?,
                cursor: self
                    .cursor
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            operation: uuid(required(value.operation)?)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
            root: required(value.root)?,
            watch: uuid(required(value.watch)?)?,
            directories: value
                .directories
                .into_iter()
                .map(|v| -> Result<_, Error> { Ok(v) })
                .collect::<Result<_, _>>()?,
            cursor: value
                .cursor
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::files::PreviewArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::PreviewArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::PreviewArgs {
                server: Some(self.server.to_string()),
                operation: Some(self.operation.to_string()),
                path: Some(self.path.clone()),
                expected: self
                    .expected
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            operation: uuid(required(value.operation)?)?,
            path: required(value.path)?,
            expected: value
                .expected
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::files::ReadTextArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::ReadTextArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::ReadTextArgs {
                server: Some(self.server.to_string()),
                operation: Some(self.operation.to_string()),
                root: Some(self.root.clone()),
                path: Some(self.path.clone()),
                expected: Some(self.expected.to_proto()?),
                revision: self
                    .revision
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                offset: self
                    .offset
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                allow_cached: Some(self.allow_cached),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            operation: uuid(required(value.operation)?)?,
            root: required(value.root)?,
            path: required(value.path)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
            revision: value
                .revision
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            offset: value
                .offset
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            allow_cached: value.allow_cached.unwrap_or(false),
        })
    }
}

impl Convert for crate::desktop::files::ProjectIndexArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::ProjectIndexArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::ProjectIndexArgs {
                server: Some(self.server.to_string()),
                operation: Some(self.operation.to_string()),
                root: Some(self.root.clone()),
                expected: Some(self.expected.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            operation: uuid(required(value.operation)?)?,
            root: required(value.root)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::files::WriteTextArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::WriteTextArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::WriteTextArgs {
                server: Some(self.server.to_string()),
                operation: Some(self.operation.to_string()),
                root: Some(self.root.clone()),
                path: Some(self.path.clone()),
                expected: Some(self.expected.to_proto()?),
                content: Some(bytes::Bytes::copy_from_slice(self.content.as_bytes())),
                revision: Some(self.revision.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            operation: uuid(required(value.operation)?)?,
            root: required(value.root)?,
            path: required(value.path)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
            content: String::from_utf8(required(value.content)?.to_vec()).map_err(Error::Utf8)?,
            revision: required(value.revision)?,
        })
    }
}

impl Convert for crate::desktop::files::CancelArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::CancelArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::CancelArgs {
                operation: Some(self.operation.to_string()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            operation: uuid(required(value.operation)?)?,
        })
    }
}

impl Convert for crate::desktop::files::ProgressArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::ProgressArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::ProgressArgs {
                operation: Some(self.operation.to_string()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            operation: uuid(required(value.operation)?)?,
        })
    }
}

impl Convert for crate::desktop::files::UploadArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::UploadArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::UploadArgs {
                server: Some(self.server.to_string()),
                operation: Some(self.operation.to_string()),
                path: Some(self.path.clone()),
                local_path: Some(self.local_path.to_proto()?),
                expected: Some(self.expected.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            operation: uuid(required(value.operation)?)?,
            path: required(value.path)?,
            local_path: <crate::desktop::files::NativePath as Convert>::from_proto(required(
                value.local_path,
            )?)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::files::DownloadArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::DownloadArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::files::DownloadArgs {
                server: Some(self.server.to_string()),
                operation: Some(self.operation.to_string()),
                path: Some(self.path.clone()),
                local_path: Some(self.local_path.to_proto()?),
                expected: Some(self.expected.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            operation: uuid(required(value.operation)?)?,
            path: required(value.path)?,
            local_path: <crate::desktop::files::NativePath as Convert>::from_proto(required(
                value.local_path,
            )?)?,
            expected: <crate::desktop::configuration::ConnectionScope as Convert>::from_proto(
                required(value.expected)?,
            )?,
        })
    }
}

impl Convert for crate::desktop::files::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::files::Operation { value: Some(match self { crate::desktop::files::Operation::List(v) => crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::List(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Operation::Watch(v) => crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::Watch(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Operation::Preview(v) => crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::Preview(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Operation::ReadText(v) => crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::ReadText(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Operation::ProjectIndex(v) => crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::ProjectIndex(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Operation::WriteText(v) => crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::WriteText(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Operation::Cancel(v) => crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::Cancel(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Operation::Progress(v) => crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::Progress(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Operation::Upload(v) => crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::Upload(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Operation::Download(v) => crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::Download(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::List(v) => crate::desktop::files::Operation::List(Box::new(<crate::desktop::files::ListArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::Watch(v) => crate::desktop::files::Operation::Watch(Box::new(<crate::desktop::files::WatchArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::Preview(v) => crate::desktop::files::Operation::Preview(Box::new(<crate::desktop::files::PreviewArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::ReadText(v) => crate::desktop::files::Operation::ReadText(Box::new(<crate::desktop::files::ReadTextArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::ProjectIndex(v) => crate::desktop::files::Operation::ProjectIndex(Box::new(<crate::desktop::files::ProjectIndexArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::WriteText(v) => crate::desktop::files::Operation::WriteText(Box::new(<crate::desktop::files::WriteTextArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::Cancel(v) => crate::desktop::files::Operation::Cancel(Box::new(<crate::desktop::files::CancelArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::Progress(v) => crate::desktop::files::Operation::Progress(Box::new(<crate::desktop::files::ProgressArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::Upload(v) => crate::desktop::files::Operation::Upload(Box::new(<crate::desktop::files::UploadArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::operation::Value::Download(v) => crate::desktop::files::Operation::Download(Box::new(<crate::desktop::files::DownloadArgs as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::files::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::files::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::files::Reply { value: Some(match self { crate::desktop::files::Reply::List(v) => crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::List(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Reply::Watch(v) => crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::Watch(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Reply::Preview(v) => crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::Preview(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Reply::ReadText(v) => crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::ReadText(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Reply::ProjectIndex(v) => crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::ProjectIndex(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Reply::WriteText(v) => crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::WriteText(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Reply::Cancel(v) => crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::Cancel(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Reply::Progress(v) => crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::Progress(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Reply::Upload(v) => crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::Upload(Box::new(v.as_ref().to_proto()?)), crate::desktop::files::Reply::Download(v) => crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::Download(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::List(v) => crate::desktop::files::Reply::List(Box::new(<crate::desktop::files::Listing as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::Watch(v) => crate::desktop::files::Reply::Watch(Box::new(<crate::desktop::files::WatchReply as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::Preview(v) => crate::desktop::files::Reply::Preview(Box::new(<crate::desktop::files::Preview as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::ReadText(v) => crate::desktop::files::Reply::ReadText(Box::new(<crate::desktop::files::Document as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::ProjectIndex(v) => crate::desktop::files::Reply::ProjectIndex(Box::new(<crate::desktop::files::Index as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::WriteText(v) => crate::desktop::files::Reply::WriteText(Box::new(<crate::desktop::files::Document as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::Cancel(v) => crate::desktop::files::Reply::Cancel(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::Progress(v) => crate::desktop::files::Reply::Progress(Box::new(<crate::desktop::files::ProgressLookup as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::Upload(v) => crate::desktop::files::Reply::Upload(Box::new(<crate::desktop::files::TransferResult as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::files::reply::Value::Download(v) => crate::desktop::files::Reply::Download(Box::new(<crate::desktop::files::TransferResult as Convert>::from_proto(*v)?)) })
    }
}
