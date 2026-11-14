//! Typed feature routing. Service caches retain their original shared DTOs.
use super::failure::{invalid_result, operation};
use crate::{
    manager::Shared,
    metrics_db::MetricsDb,
    server_queries::{Cached, Read},
    terminal::Sessions,
};
use newport_protocol::{
    desktop::{Failure, containers, services},
    monitor,
};
pub struct Services<'a> {
    pub manager: &'a Shared,
    pub metrics: &'a MetricsDb,
    pub files: &'a crate::files::Operations,
    pub terminals: &'a Sessions,
    pub preferences: &'a crate::preferences::Preferences,
}

pub async fn services(
    command: &services::Operation,
    state: &Services<'_>,
) -> Result<services::Reply, Failure> {
    use services::{Operation as O, Reply as R};
    let server = match command {
        O::Snapshot(args) => args.server,
        O::Logs(args) => args.server,
        O::UnitFile(args) => args.server,
        O::Action(args) => args.server,
    };
    let owner = crate::server_session::resolve(state.manager, server)
        .await
        .map_err(operation)?;
    let cached = match command {
        O::Snapshot(args) => Some((Read::Services, args.refresh)),
        O::UnitFile(args) => Some((
            Read::UnitFile {
                unit: args.unit.clone(),
                user: args.scope == monitor::services::Scope::User,
            },
            args.refresh,
        )),
        _ => None,
    };
    let result = if let Some((key, refresh)) = cached {
        match crate::server_queries::read(&owner, key, refresh)
            .await
            .map_err(operation)?
        {
            Cached::Services(reply) => Ok(reply),
            _ => return Err(invalid_result("services cache kind")),
        }
    } else {
        crate::services::request(&owner, command.service_request()).await
    };
    if matches!(command, O::Action(_)) {
        owner.queries.invalidate_services();
        if let Some(query) = owner.ports.get() {
            query.invalidate();
        }
    }
    Ok(match (command, result.map_err(operation)?) {
        (O::Snapshot(_), monitor::services::Reply::Snapshot(value)) => R::Snapshot(Box::new(value)),
        (O::Logs(_), monitor::services::Reply::Logs(value)) => R::Logs(Box::new(value)),
        (O::UnitFile(_), monitor::services::Reply::Unit(value)) => R::UnitFile(Box::new(value)),
        (O::Action(_), monitor::services::Reply::Action(value)) => R::Action(Box::new(value)),
        _ => return Err(invalid_result("services reply operation")),
    })
}

pub async fn containers(
    command: &containers::Operation,
    state: &Services<'_>,
) -> Result<containers::Reply, Failure> {
    use containers::{Operation as O, Reply as R};
    let server = match command {
        O::Snapshot(args) => args.server,
        O::Inspect(args) => args.server,
        O::Logs(args) => args.server,
        O::Action(args) => args.server,
        O::ProjectAction(args) => args.server,
        O::Remove(args) => args.server,
        O::Disk(args) => args.server,
        O::Prune(args) => args.server,
        O::ComposeProject(args) => args.server,
        O::ComposeRun(args) => args.server,
        O::ComposeJob(args) => args.server,
        O::ComposeCancel(args) => args.server,
        O::ComposeJobs(args) => args.server,
    };
    let owner = crate::server_session::resolve(state.manager, server)
        .await
        .map_err(operation)?;
    let cached = match command {
        O::Snapshot(args) => Some((Read::Containers, args.refresh)),
        O::Inspect(args) => Some((Read::Inspect(args.container.clone()), args.refresh)),
        O::Disk(args) => Some((
            Read::Disk(match args.part {
                containers::DiskPart::Full => crate::server_queries::DiskPart::Full,
                containers::DiskPart::Quick => crate::server_queries::DiskPart::Quick,
                containers::DiskPart::Volumes => crate::server_queries::DiskPart::Volumes,
            }),
            args.refresh,
        )),
        O::ComposeProject(args) => Some((Read::Compose(args.project.clone()), args.refresh)),
        _ => None,
    };
    let result = if let Some((key, refresh)) = cached {
        match crate::server_queries::read(&owner, key, refresh)
            .await
            .map_err(operation)?
        {
            Cached::Containers(reply) => Ok(monitor::Reply::Containers(reply)),
            Cached::Compose(reply) => Ok(monitor::Reply::Compose(reply)),
            _ => return Err(invalid_result("container cache kind")),
        }
    } else {
        match command.service_request() {
            monitor::Request::Containers(request) => crate::containers::request(&owner, request)
                .await
                .map(monitor::Reply::Containers),
            monitor::Request::Compose(request) => crate::compose::request(&owner, request)
                .await
                .map(monitor::Reply::Compose),
            _ => return Err(invalid_result("container request kind")),
        }
    };
    if matches!(
        command,
        O::Action(_)
            | O::ProjectAction(_)
            | O::Remove(_)
            | O::Prune(_)
            | O::ComposeRun(_)
            | O::ComposeCancel(_)
    ) {
        owner.queries.invalidate_containers();
        if let Some(query) = owner.ports.get() {
            query.invalidate();
        }
    }
    Ok(match (command, result.map_err(operation)?) {
        (
            O::Snapshot(_),
            monitor::Reply::Containers(monitor::containers::Reply::Snapshot(value)),
        ) => R::Snapshot(Box::new(value)),
        (O::Inspect(_), monitor::Reply::Containers(monitor::containers::Reply::Inspect(value))) => {
            R::Inspect(value)
        }
        (O::Logs(_), monitor::Reply::Containers(monitor::containers::Reply::Logs(value))) => {
            R::Logs(Box::new(value))
        }
        (O::Action(_), monitor::Reply::Containers(monitor::containers::Reply::Action(value))) => {
            R::Action(Box::new(value))
        }
        (
            O::ProjectAction(_),
            monitor::Reply::Containers(monitor::containers::Reply::ProjectAction(value)),
        ) => R::ProjectAction(Box::new(value)),
        (O::Remove(_), monitor::Reply::Containers(monitor::containers::Reply::Removed(value))) => {
            R::Remove(Box::new(value))
        }
        (O::Disk(_), monitor::Reply::Containers(monitor::containers::Reply::Disk(value))) => {
            R::Disk(Box::new(value))
        }
        (O::Prune(_), monitor::Reply::Containers(monitor::containers::Reply::Pruned(value))) => {
            R::Prune(Box::new(value))
        }
        (
            O::ComposeProject(_),
            monitor::Reply::Compose(monitor::compose::Reply::Project(value)),
        ) => R::ComposeProject(Box::new(value)),
        (O::ComposeRun(_), monitor::Reply::Compose(monitor::compose::Reply::Started(value))) => {
            R::ComposeRun(Box::new(value))
        }
        (O::ComposeJob(_), monitor::Reply::Compose(monitor::compose::Reply::Job(value))) => {
            R::ComposeJob(Box::new(value))
        }
        (
            O::ComposeCancel(_),
            monitor::Reply::Compose(monitor::compose::Reply::Cancelled(value)),
        ) => R::ComposeCancel(Box::new(value)),
        (O::ComposeJobs(_), monitor::Reply::Compose(monitor::compose::Reply::Jobs(value))) => {
            R::ComposeJobs(Box::new(value))
        }
        _ => return Err(invalid_result("container reply operation")),
    })
}

#[cfg(test)]
mod tests;
