use super::*;
impl Convert for crate::desktop::git::CollectedDiff {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::git::CollectedDiff;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::git::CollectedDiff {
                snapshot: Some(self.snapshot.clone()),
                diff: Some(self.diff.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            snapshot: required(value.snapshot)?,
            diff: <crate::git::Diff as Convert>::from_proto(required(value.diff)?)?,
        })
    }
}

impl Convert for crate::desktop::git::GitResult {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::git::GitResult;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::git::GitResult { value: Some(match self { crate::desktop::git::GitResult::Reply(v) => crate::encoding::protobuf::generated::newport::desktop::git::git_result::Value::Reply(Box::new(v.as_ref().to_proto()?)), crate::desktop::git::GitResult::Diff(v) => crate::encoding::protobuf::generated::newport::desktop::git::git_result::Value::Diff(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::git::git_result::Value::Reply(v) => crate::desktop::git::GitResult::Reply(Box::new(<crate::git::Reply as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::git::git_result::Value::Diff(v) => crate::desktop::git::GitResult::Diff(Box::new(<crate::desktop::git::CollectedDiff as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::git::Refreshed {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::git::Refreshed;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::git::Refreshed {
                repo_id: Some(self.repo_id.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            repo_id: required(value.repo_id)?,
        })
    }
}

impl Convert for crate::desktop::git::ReceiptState {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Pending => 1,
            Self::Succeeded => 2,
            Self::Failed => 3,
            Self::Rejected => 4,
            Self::NeedsResolution => 5,
            Self::OutcomeUnknown => 6,
            Self::ReviewedUnknown => 7,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Pending,
            2 => Self::Succeeded,
            3 => Self::Failed,
            4 => Self::Rejected,
            5 => Self::NeedsResolution,
            6 => Self::OutcomeUnknown,
            7 => Self::ReviewedUnknown,
            _ => return Err(Invalid("ReceiptState").into()),
        })
    }
}

impl Convert for crate::desktop::git::Receipt {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::git::Receipt;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::git::Receipt {
                operation_id: Some(self.operation_id.to_string()),
                server: Some(self.server.to_string()),
                action: Some(self.action.clone()),
                state: Some(self.state.to_proto()?),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            operation_id: uuid(required(value.operation_id)?)?,
            server: uuid(required(value.server)?)?,
            action: required(value.action)?,
            state: <crate::desktop::git::ReceiptState as Convert>::from_proto(required(
                value.state,
            )?)?,
        })
    }
}

impl Convert for crate::desktop::git::Receipts {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::git::Receipts;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::git::Receipts {
                receipts: self
                    .receipts
                    .iter()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .collect::<Result<_, _>>()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            receipts: value
                .receipts
                .into_iter()
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::git::Receipt as Convert>::from_proto(v)
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Convert for crate::desktop::git::RequestArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::git::RequestArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::git::RequestArgs {
                server: Some(self.server.to_string()),
                request: Some(self.request.to_proto()?),
                retry_connection: Some(self.retry_connection),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            request: <crate::git::Request as Convert>::from_proto(required(value.request)?)?,
            retry_connection: value.retry_connection.unwrap_or(false),
        })
    }
}

impl Convert for crate::desktop::git::RefreshRepositoryArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::git::RefreshRepositoryArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::git::RefreshRepositoryArgs {
                server: Some(self.server.to_string()),
                repo_id: Some(self.repo_id.clone()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            repo_id: required(value.repo_id)?,
        })
    }
}

impl Convert for crate::desktop::git::PendingOperationsArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::git::PendingOperationsArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::git::PendingOperationsArgs {
                server: Some(self.server.to_string()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
        })
    }
}

impl Convert for crate::desktop::git::AcknowledgeOperationArgs {
    type Proto =
        crate::encoding::protobuf::generated::newport::desktop::git::AcknowledgeOperationArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::git::AcknowledgeOperationArgs {
                server: Some(self.server.to_string()),
                operation_id: Some(self.operation_id.to_string()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            operation_id: uuid(required(value.operation_id)?)?,
        })
    }
}

impl Convert for crate::desktop::git::ReviewOperationArgs {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::git::ReviewOperationArgs;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::git::ReviewOperationArgs {
                server: Some(self.server.to_string()),
                operation_id: Some(self.operation_id.to_string()),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            operation_id: uuid(required(value.operation_id)?)?,
        })
    }
}

impl Convert for crate::desktop::git::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::git::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::git::Operation { value: Some(match self { crate::desktop::git::Operation::Request(v) => crate::encoding::protobuf::generated::newport::desktop::git::operation::Value::Request(Box::new(v.as_ref().to_proto()?)), crate::desktop::git::Operation::RefreshRepository(v) => crate::encoding::protobuf::generated::newport::desktop::git::operation::Value::RefreshRepository(Box::new(v.as_ref().to_proto()?)), crate::desktop::git::Operation::PendingOperations(v) => crate::encoding::protobuf::generated::newport::desktop::git::operation::Value::PendingOperations(Box::new(v.as_ref().to_proto()?)), crate::desktop::git::Operation::AcknowledgeOperation(v) => crate::encoding::protobuf::generated::newport::desktop::git::operation::Value::AcknowledgeOperation(Box::new(v.as_ref().to_proto()?)), crate::desktop::git::Operation::ReviewOperation(v) => crate::encoding::protobuf::generated::newport::desktop::git::operation::Value::ReviewOperation(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::git::operation::Value::Request(v) => crate::desktop::git::Operation::Request(Box::new(<crate::desktop::git::RequestArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::git::operation::Value::RefreshRepository(v) => crate::desktop::git::Operation::RefreshRepository(Box::new(<crate::desktop::git::RefreshRepositoryArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::git::operation::Value::PendingOperations(v) => crate::desktop::git::Operation::PendingOperations(Box::new(<crate::desktop::git::PendingOperationsArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::git::operation::Value::AcknowledgeOperation(v) => crate::desktop::git::Operation::AcknowledgeOperation(Box::new(<crate::desktop::git::AcknowledgeOperationArgs as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::git::operation::Value::ReviewOperation(v) => crate::desktop::git::Operation::ReviewOperation(Box::new(<crate::desktop::git::ReviewOperationArgs as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::git::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::git::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::git::Reply { value: Some(match self { crate::desktop::git::Reply::Request(v) => crate::encoding::protobuf::generated::newport::desktop::git::reply::Value::Request(Box::new(v.as_ref().to_proto()?)), crate::desktop::git::Reply::RefreshRepository(v) => crate::encoding::protobuf::generated::newport::desktop::git::reply::Value::RefreshRepository(Box::new(v.as_ref().to_proto()?)), crate::desktop::git::Reply::PendingOperations(v) => crate::encoding::protobuf::generated::newport::desktop::git::reply::Value::PendingOperations(Box::new(v.as_ref().to_proto()?)), crate::desktop::git::Reply::AcknowledgeOperation(v) => crate::encoding::protobuf::generated::newport::desktop::git::reply::Value::AcknowledgeOperation(Box::new(v.as_ref().to_proto()?)), crate::desktop::git::Reply::ReviewOperation(v) => crate::encoding::protobuf::generated::newport::desktop::git::reply::Value::ReviewOperation(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::git::reply::Value::Request(v) => crate::desktop::git::Reply::Request(Box::new(<crate::desktop::git::GitResult as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::git::reply::Value::RefreshRepository(v) => crate::desktop::git::Reply::RefreshRepository(Box::new(<crate::desktop::git::Refreshed as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::git::reply::Value::PendingOperations(v) => crate::desktop::git::Reply::PendingOperations(Box::new(<crate::desktop::git::Receipts as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::git::reply::Value::AcknowledgeOperation(v) => crate::desktop::git::Reply::AcknowledgeOperation(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::git::reply::Value::ReviewOperation(v) => crate::desktop::git::Reply::ReviewOperation(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)) })
    }
}
