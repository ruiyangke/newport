use super::super::compose as contract;
use super::{Invalid, Record, pb, required};

impl Record for contract::CliKind {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Plugin => 1,
            Self::Standalone => 2,
            Self::Podman => 3,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Plugin,
            2 => Self::Standalone,
            3 => Self::Podman,
            _ => return Err(Invalid("compose CliKind")),
        })
    }
}

impl Record for contract::State {
    type Wire = i32;
    fn to_wire(&self) -> i32 {
        match self {
            Self::Running => 1,
            Self::Succeeded => 2,
            Self::Failed => 3,
            Self::Cancelled => 4,
        }
    }
    fn from_wire(value: i32) -> Result<Self, Invalid> {
        Ok(match value {
            1 => Self::Running,
            2 => Self::Succeeded,
            3 => Self::Failed,
            4 => Self::Cancelled,
            _ => return Err(Invalid("compose State")),
        })
    }
}

impl Record for contract::Cli {
    type Wire = pb::ComposeCli;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            kind: self.kind.to_wire(),
            version: self.version.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            kind: contract::CliKind::from_wire(value.kind)?,
            version: value.version,
        })
    }
}

impl Record for contract::File {
    type Wire = pb::ComposeFile;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            path: self.path.clone(),
            text: self.text.clone(),
            error: self.error.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            path: value.path,
            text: value.text,
            error: value.error,
        })
    }
}

impl Record for contract::Service {
    type Wire = pb::ComposeService;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            name: self.name.clone(),
            image: self.image.clone(),
            build: self.build,
            profiles: self.profiles.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            name: value.name,
            image: value.image,
            build: value.build,
            profiles: value.profiles,
        })
    }
}

impl Record for contract::Project {
    type Wire = pb::ComposeProject;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            cli: self.cli.as_ref().map(Record::to_wire),
            cli_error: self.cli_error.clone(),
            working_dir: self.working_dir.clone(),
            config_files: self.config_files.clone(),
            files: self.files.iter().map(Record::to_wire).collect(),
            services: self.services.as_ref().map(|values| pb::ComposeServiceList {
                values: values.iter().map(Record::to_wire).collect(),
            }),
            config_error: self.config_error.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            cli: value.cli.map(contract::Cli::from_wire).transpose()?,
            cli_error: value.cli_error,
            working_dir: value.working_dir,
            config_files: value.config_files,
            files: value
                .files
                .into_iter()
                .map(contract::File::from_wire)
                .collect::<Result<_, _>>()?,
            services: value
                .services
                .map(|v| {
                    v.values
                        .into_iter()
                        .map(contract::Service::from_wire)
                        .collect::<Result<_, _>>()
                })
                .transpose()?,
            config_error: value.config_error,
        })
    }
}

impl Record for contract::Started {
    type Wire = pb::ComposeStarted;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            job: self.job.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self { job: value.job })
    }
}

impl Record for contract::Job {
    type Wire = pb::ComposeJob;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            job: self.job.clone(),
            project: self.project.clone(),
            command: self.command.clone(),
            state: self.state.to_wire(),
            exit_code: self.exit_code,
            started_at: self.started_at,
            finished_at: self.finished_at,
            output: self.output.clone(),
            offset: self.offset,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            job: value.job,
            project: value.project,
            command: value.command,
            state: contract::State::from_wire(value.state)?,
            exit_code: value.exit_code,
            started_at: value.started_at,
            finished_at: value.finished_at,
            output: value.output,
            offset: value.offset,
        })
    }
}

impl Record for contract::Jobs {
    type Wire = pb::ComposeJobs;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            jobs: self.jobs.iter().map(Record::to_wire).collect(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            jobs: value
                .jobs
                .into_iter()
                .map(contract::Job::from_wire)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl Record for contract::Done {
    type Wire = pb::ComposeDone;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire { ok: self.ok }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self { ok: value.ok })
    }
}

impl Record for contract::Action {
    type Wire = pb::ComposeAction;
    fn to_wire(&self) -> Self::Wire {
        use pb::compose_action::Operation;
        let operation = match self {
            Self::Up {
                services,
                build,
                pull,
                force_recreate,
            } => Operation::Up(pb::ComposeUp {
                services: services.clone(),
                build: *build,
                pull: *pull,
                force_recreate: *force_recreate,
            }),
            Self::Down {
                volumes,
                remove_orphans,
            } => Operation::Down(pb::ComposeDown {
                volumes: *volumes,
                remove_orphans: *remove_orphans,
            }),
            Self::Pull { services } => Operation::Pull(pb::ComposePull {
                services: services.clone(),
            }),
            Self::Build { services, no_cache } => Operation::Build(pb::ComposeBuild {
                services: services.clone(),
                no_cache: *no_cache,
            }),
            Self::Scale { service, replicas } => Operation::Scale(pb::ComposeScale {
                service: service.clone(),
                replicas: *replicas,
            }),
        };
        Self::Wire {
            operation: Some(operation),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        use pb::compose_action::Operation;
        Ok(match required(value.operation)? {
            Operation::Up(v) => Self::Up {
                services: v.services,
                build: v.build,
                pull: v.pull,
                force_recreate: v.force_recreate,
            },
            Operation::Down(v) => Self::Down {
                volumes: v.volumes,
                remove_orphans: v.remove_orphans,
            },
            Operation::Pull(v) => Self::Pull {
                services: v.services,
            },
            Operation::Build(v) => Self::Build {
                services: v.services,
                no_cache: v.no_cache,
            },
            Operation::Scale(v) => Self::Scale {
                service: v.service,
                replicas: v.replicas,
            },
        })
    }
}
impl Record for contract::Request {
    type Wire = pb::ComposeRequest;
    fn to_wire(&self) -> Self::Wire {
        use pb::compose_request::Operation;
        let operation = match self {
            Self::Project { project } => Operation::Project(pb::ComposeProjectRequest {
                project: project.clone(),
            }),
            Self::Run { project, action } => Operation::Run(pb::ComposeRun {
                project: project.clone(),
                action: Some(action.to_wire()),
            }),
            Self::Job { job, offset } => Operation::Job(pb::ComposeJobRequest {
                job: job.clone(),
                offset: *offset,
            }),
            Self::Cancel { job } => Operation::Cancel(pb::ComposeCancel { job: job.clone() }),
            Self::Jobs => Operation::Jobs(crate::encoding::protobuf::Present {}),
        };
        Self::Wire {
            operation: Some(operation),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        use pb::compose_request::Operation;
        Ok(match required(value.operation)? {
            Operation::Project(v) => Self::Project { project: v.project },
            Operation::Run(v) => Self::Run {
                project: v.project,
                action: contract::Action::from_wire(required(v.action)?)?,
            },
            Operation::Job(v) => Self::Job {
                job: v.job,
                offset: v.offset,
            },
            Operation::Cancel(v) => Self::Cancel { job: v.job },
            Operation::Jobs(_) => Self::Jobs,
        })
    }
}
impl Record for contract::Reply {
    type Wire = pb::ComposeReply;
    fn to_wire(&self) -> Self::Wire {
        use pb::compose_reply::Result;
        let result = match self {
            Self::Project(v) => Result::Project(v.to_wire()),
            Self::Started(v) => Result::Started(v.to_wire()),
            Self::Job(v) => Result::Job(v.to_wire()),
            Self::Cancelled(v) => Result::Cancelled(v.to_wire()),
            Self::Jobs(v) => Result::Jobs(v.to_wire()),
        };
        Self::Wire {
            result: Some(result),
        }
    }
    fn from_wire(value: Self::Wire) -> std::result::Result<Self, Invalid> {
        use pb::compose_reply::Result;
        Ok(match required(value.result)? {
            Result::Project(v) => Self::Project(contract::Project::from_wire(v)?),
            Result::Started(v) => Self::Started(contract::Started::from_wire(v)?),
            Result::Job(v) => Self::Job(contract::Job::from_wire(v)?),
            Result::Cancelled(v) => Self::Cancelled(contract::Done::from_wire(v)?),
            Result::Jobs(v) => Self::Jobs(contract::Jobs::from_wire(v)?),
        })
    }
}
