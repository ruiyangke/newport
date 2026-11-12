use super::*;
impl Convert for crate::desktop::account::Phase {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Loading => 1,
            Self::SignedOut => 2,
            Self::Waiting => 3,
            Self::SignedIn => 4,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Loading,
            2 => Self::SignedOut,
            3 => Self::Waiting,
            4 => Self::SignedIn,
            _ => return Err(Invalid("Phase").into()),
        })
    }
}

impl Convert for crate::desktop::account::NoticeKind {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::Success => 1,
            Self::Info => 2,
            Self::Warning => 3,
            Self::Error => 4,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Success,
            2 => Self::Info,
            3 => Self::Warning,
            4 => Self::Error,
            _ => return Err(Invalid("NoticeKind").into()),
        })
    }
}

impl Convert for crate::desktop::account::NoticeAction {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::ManageComputers => 1,
            Self::TryAgain => 2,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::ManageComputers,
            2 => Self::TryAgain,
            _ => return Err(Invalid("NoticeAction").into()),
        })
    }
}

impl Convert for crate::desktop::account::RefusalReason {
    type Proto = i32;
    fn to_proto(&self) -> Result<i32, Error> {
        Ok(match self {
            Self::SignedOut => 1,
            Self::TrialElsewhere => 2,
            Self::FreeSeat => 3,
            Self::Free => 4,
            Self::Unavailable => 5,
            Self::Clock => 6,
            Self::Offline => 7,
        })
    }
    fn from_proto(value: i32) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::SignedOut,
            2 => Self::TrialElsewhere,
            3 => Self::FreeSeat,
            4 => Self::Free,
            5 => Self::Unavailable,
            6 => Self::Clock,
            7 => Self::Offline,
            _ => return Err(Invalid("RefusalReason").into()),
        })
    }
}

impl Convert for crate::desktop::account::Notice {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::account::Notice;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::account::Notice {
                id: Some(self.id),
                kind: Some(self.kind.to_proto()?),
                message: Some(self.message.clone()),
                action: self
                    .action
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            id: required(value.id)?,
            kind: <crate::desktop::account::NoticeKind as Convert>::from_proto(required(
                value.kind,
            )?)?,
            message: required(value.message)?,
            action: value
                .action
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::account::NoticeAction as Convert>::from_proto(v)
                })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::account::License {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::account::License;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::account::License {
                state: self
                    .state
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                plan: self
                    .plan
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                seat: self
                    .seat
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                valid_until: self
                    .valid_until
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                renews_at: self
                    .renews_at
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                ends_at: self
                    .ends_at
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                trial_used: self
                    .trial_used
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
                message: self
                    .message
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                trial_used_here: self
                    .trial_used_here
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(*v) })
                    .transpose()?,
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            state: value
                .state
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            plan: value
                .plan
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            seat: value
                .seat
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            valid_until: value
                .valid_until
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            renews_at: value
                .renews_at
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            ends_at: value
                .ends_at
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            trial_used: value
                .trial_used
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            message: value
                .message
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            trial_used_here: value
                .trial_used_here
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
        })
    }
}

impl Convert for crate::desktop::account::Status {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::account::Status;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::account::Status {
                state: Some(self.state.to_proto()?),
                email: self
                    .email
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                name: self
                    .name
                    .as_ref()
                    .map(|v| -> Result<_, Error> { Ok(v.clone()) })
                    .transpose()?,
                license: self
                    .license
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                signing_out: Some(self.signing_out),
                notice: self
                    .notice
                    .as_ref()
                    .map(|v| -> Result<_, Error> { v.to_proto() })
                    .transpose()?,
                license_revision: Some(self.license_revision),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            state: <crate::desktop::account::Phase as Convert>::from_proto(required(value.state)?)?,
            email: value
                .email
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            name: value
                .name
                .map(|v| -> Result<_, Error> { Ok(v) })
                .transpose()?,
            license: value
                .license
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::account::License as Convert>::from_proto(v)
                })
                .transpose()?,
            signing_out: required(value.signing_out)?,
            notice: value
                .notice
                .map(|v| -> Result<_, Error> {
                    <crate::desktop::account::Notice as Convert>::from_proto(v)
                })
                .transpose()?,
            license_revision: required(value.license_revision)?,
        })
    }
}

impl Convert for crate::desktop::account::RemoteLicense {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::account::RemoteLicense;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(
            crate::encoding::protobuf::generated::newport::desktop::account::RemoteLicense {
                token: Some(self.token.to_proto()?),
                machine: Some(self.machine.clone()),
                trusted_time: Some(self.trusted_time),
                expires_at: Some(self.expires_at),
            },
        )
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(Self {
            token: <crate::common::Secret as Convert>::from_proto(required(value.token)?)?,
            machine: required(value.machine)?,
            trusted_time: required(value.trusted_time)?,
            expires_at: required(value.expires_at)?,
        })
    }
}

impl Convert for crate::desktop::account::Operation {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::account::Operation;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::account::Operation { value: Some(match self { crate::desktop::account::Operation::Status(v) => crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::Status(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Operation::SignIn(v) => crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::SignIn(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Operation::CancelSignIn(v) => crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::CancelSignIn(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Operation::SignOut(v) => crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::SignOut(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Operation::OpenPortal(v) => crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::OpenPortal(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Operation::OpenBilling(v) => crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::OpenBilling(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Operation::Refresh(v) => crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::Refresh(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Operation::StartTrial(v) => crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::StartTrial(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Operation::RemoteLicense(v) => crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::RemoteLicense(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::Status(v) => crate::desktop::account::Operation::Status(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::SignIn(v) => crate::desktop::account::Operation::SignIn(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::CancelSignIn(v) => crate::desktop::account::Operation::CancelSignIn(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::SignOut(v) => crate::desktop::account::Operation::SignOut(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::OpenPortal(v) => crate::desktop::account::Operation::OpenPortal(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::OpenBilling(v) => crate::desktop::account::Operation::OpenBilling(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::Refresh(v) => crate::desktop::account::Operation::Refresh(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::StartTrial(v) => crate::desktop::account::Operation::StartTrial(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::operation::Value::RemoteLicense(v) => crate::desktop::account::Operation::RemoteLicense(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)) })
    }
}

impl Convert for crate::desktop::account::Reply {
    type Proto = crate::encoding::protobuf::generated::newport::desktop::account::Reply;
    fn to_proto(&self) -> Result<Self::Proto, Error> {
        Ok(crate::encoding::protobuf::generated::newport::desktop::account::Reply { value: Some(match self { crate::desktop::account::Reply::Status(v) => crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::Status(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Reply::SignIn(v) => crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::SignIn(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Reply::CancelSignIn(v) => crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::CancelSignIn(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Reply::SignOut(v) => crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::SignOut(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Reply::OpenPortal(v) => crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::OpenPortal(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Reply::OpenBilling(v) => crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::OpenBilling(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Reply::Refresh(v) => crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::Refresh(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Reply::StartTrial(v) => crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::StartTrial(Box::new(v.as_ref().to_proto()?)), crate::desktop::account::Reply::RemoteLicense(v) => crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::RemoteLicense(Box::new(v.as_ref().to_proto()?)) }) })
    }
    fn from_proto(value: Self::Proto) -> Result<Self, Error> {
        Ok(match required(value.value)? { crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::Status(v) => crate::desktop::account::Reply::Status(Box::new(<crate::desktop::account::Status as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::SignIn(v) => crate::desktop::account::Reply::SignIn(Box::new(<crate::desktop::account::Status as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::CancelSignIn(v) => crate::desktop::account::Reply::CancelSignIn(Box::new(<crate::desktop::account::Status as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::SignOut(v) => crate::desktop::account::Reply::SignOut(Box::new(<crate::desktop::account::Status as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::OpenPortal(v) => crate::desktop::account::Reply::OpenPortal(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::OpenBilling(v) => crate::desktop::account::Reply::OpenBilling(Box::new(<crate::common::Present as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::Refresh(v) => crate::desktop::account::Reply::Refresh(Box::new(<crate::desktop::account::Status as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::StartTrial(v) => crate::desktop::account::Reply::StartTrial(Box::new(<crate::desktop::account::Status as Convert>::from_proto(*v)?)), crate::encoding::protobuf::generated::newport::desktop::account::reply::Value::RemoteLicense(v) => crate::desktop::account::Reply::RemoteLicense(Box::new(<crate::desktop::account::RemoteLicense as Convert>::from_proto(*v)?)) })
    }
}
