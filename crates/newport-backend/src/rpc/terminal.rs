//! Persistent terminal administration, separate from a desktop stream lease.
use super::{
    failure::{operation, runtime},
    features::Services,
};
use newport_protocol::{
    common::Present,
    desktop::{Failure, terminal as dto},
    terminal as service,
};

pub async fn dispatch(
    command: &dto::Operation,
    state: &Services<'_>,
) -> Result<dto::Reply, Failure> {
    use dto::{Operation as O, Reply as R};
    Ok(match command {
        O::Close(args) => {
            crate::commands::terminal_close(state.terminals, args.session)
                .await
                .map_err(operation)?;
            R::Close(Box::new(Present {}))
        }
        O::List(args) => {
            let owner = crate::server_session::resolve(state.manager, args.server)
                .await
                .map_err(operation)?;
            let value = crate::terminal::held_list::read(&owner);
            R::List(Box::new(dto::HeldShells {
                desktop: value.desktop,
                holds: value.holds,
                shells: value.shells.map(|shells| dto::ShellList {
                    shells: shells
                        .into_iter()
                        .map(|shell| dto::HeldShell {
                            terminal: shell.terminal,
                            name: shell.name,
                            title: shell.title,
                            creator: service::Computer {
                                id: shell.creator.id,
                                name: shell.creator.name,
                            },
                            attached: shell
                                .attached
                                .into_iter()
                                .map(|computer| service::Attached {
                                    id: computer.id,
                                    name: computer.name,
                                    active: computer.active,
                                })
                                .collect(),
                            started: shell.started,
                            exited: shell.exited,
                        })
                        .collect(),
                }),
            }))
        }
        O::Rename(args) => {
            let owner = crate::server_session::resolve(state.manager, args.server)
                .await
                .map_err(operation)?;
            R::Rename(Box::new(dto::Renamed {
                renamed: crate::terminal::holder_channel::rename(&owner, args.terminal, &args.name)
                    .await
                    .map_err(runtime)?,
            }))
        }
        O::End(args) => {
            let owner = crate::server_session::resolve(state.manager, args.server)
                .await
                .map_err(operation)?;
            R::End(Box::new(dto::Ended {
                ended: crate::terminal::holder_channel::end(&owner, args.terminal)
                    .await
                    .map_err(runtime)?,
            }))
        }
    })
}
