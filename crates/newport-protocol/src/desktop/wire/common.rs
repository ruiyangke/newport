use super::*;
impl Convert for crate::desktop::Hello {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::common::Hello;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::common::Hello {
                version: Some(self.version),
                contract_id: self.contract_id.clone(),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            version: required(value.version)?,
            contract_id: value.contract_id,
        })
    }
}

impl Convert for crate::desktop::Ready {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::common::Ready;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::common::Ready {
                version: Some(self.version),
                backend_instance: Some(self.backend_instance.to_string()),
                contract_id: self.contract_id.clone(),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            version: required(value.version)?,
            backend_instance: uuid(required(value.backend_instance)?)?,
            contract_id: value.contract_id,
        })
    }
}

impl Convert for crate::desktop::FailureDetail {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::common::FailureDetail;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::common::FailureDetail { value: Some(match self { crate::desktop::FailureDetail::LicenseRefusal(v) => crate::encoding::protobuf::generated::newport::desktop::common::failure_detail::Value::LicenseRefusal(v.as_ref().to_proto()?) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::common::failure_detail::Value::LicenseRefusal(v) => crate::desktop::FailureDetail::LicenseRefusal(Box::new(<crate::desktop::account::RefusalReason as Convert>::from_proto(v)?)) })
    }
}

impl Convert for crate::desktop::Failure {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::common::Failure;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::common::Failure {
                error: Some(self.error.to_proto()?),
                detail: self
                    .detail
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            error: <crate::common::Error as Convert>::from_proto(required(value.error)?)?,
            detail: value
                .detail
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::FailureDetail as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::AgentApproval {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::common::AgentApproval;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::common::AgentApproval {
                server: Some(self.server.to_string()),
                awaiting: Some(self.awaiting),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            server: uuid(required(value.server)?)?,
            awaiting: required(value.awaiting)?,
        })
    }
}

impl Convert for crate::desktop::ProgressState {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::common::ProgressState;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::common::ProgressState { value: Some(match self { crate::desktop::ProgressState::AgentApproval(v) => crate::encoding::protobuf::generated::newport::desktop::common::progress_state::Value::AgentApproval(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::common::progress_state::Value::AgentApproval(v) => crate::desktop::ProgressState::AgentApproval(Box::new(<crate::desktop::AgentApproval as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::Progress {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::common::Progress;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::common::Progress { id: Some(self.id.get()), state: Some(match &self.state { crate::desktop::ProgressState::AgentApproval(v) => crate::encoding::protobuf::generated::newport::desktop::common::progress::State::AgentApproval(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self { id: crate::desktop::RequestId::new(required(value.id)?)?, state: match required(value.state)? { crate::encoding::protobuf::generated::newport::desktop::common::progress::State::AgentApproval(v) => crate::desktop::ProgressState::AgentApproval(Box::new(<crate::desktop::AgentApproval as Convert>::from_proto(*v)?)) } })
    }
}
