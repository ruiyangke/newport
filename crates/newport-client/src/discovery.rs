//! Typed port observations retain source coverage and every verified holder.
use crate::{backend::Backend, request_error::RequestError};
use newport_protocol::desktop::{self, connection};

mod view;
pub use view::View;
mod reading;
pub use reading::Reading;

impl Backend {
    pub fn discover_ports(
        &self,
        server: uuid::Uuid,
        refresh: bool,
    ) -> Result<connection::PortsObservation, RequestError> {
        let reply = self.execute(desktop::Operation::Connection(Box::new(
            connection::Operation::ConnectionsQuery(Box::new(connection::ConnectionsQueryArgs {
                server,
                refresh,
            })),
        )))?;
        match reply {
            desktop::Reply::Connection(reply) => match *reply {
                connection::Reply::ConnectionsQuery(value) => Ok(*value),
                _ => Err("Unexpected discovery response operation.".into()),
            },
            _ => Err("Unexpected discovery response family.".into()),
        }
    }
}
