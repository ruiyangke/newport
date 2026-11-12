use super::{Direction, MAX_CHUNK};
use crate::{common::Limits, git, integration, monitor, remote_apps, terminal};

/// The carried protocol, chosen by the compiled service registration. Native
/// frames pass through unchanged; opaque streams retain their external framing
/// inside bounded chunks. Neither client input nor payload sniffing selects it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Protobuf { client: Limits, server: Limits },
    Opaque,
}

impl Transport {
    pub const GIT: Self = Self::Protobuf {
        client: Limits {
            message: git::MAX_FRAME,
            metadata: git::MAX_FRAME,
        },
        server: Limits {
            message: git::MAX_FRAME,
            metadata: git::MAX_FRAME,
        },
    };
    pub const MONITOR: Self = Self::Protobuf {
        client: monitor::CLIENT_LIMITS,
        server: monitor::SERVER_LIMITS,
    };
    pub const TERMINAL: Self = Self::Protobuf {
        client: terminal::CLIENT_LIMITS,
        server: terminal::SERVER_LIMITS,
    };
    pub const INTEGRATION: Self = Self::Protobuf {
        client: integration::CLIENT_LIMITS,
        server: integration::SERVER_LIMITS,
    };
    pub const REMOTE_APPS: Self = Self::Protobuf {
        client: remote_apps::CLIENT_LIMITS,
        server: remote_apps::SERVER_LIMITS,
    };

    /// Bounds a socket frame, allowing process controls even when the service
    /// has smaller limits. A service body still obeys its own directional limit.
    pub const fn limits(self, direction: Direction) -> Limits {
        let controls = Limits {
            message: MAX_CHUNK + 64,
            metadata: 64,
        };
        let service = match self {
            Self::Opaque => return controls,
            Self::Protobuf { client, server } => match direction {
                Direction::ToWorker => client,
                Direction::FromWorker => server,
            },
        };
        Limits {
            message: max(service.message, controls.message),
            metadata: max(service.metadata, controls.metadata),
        }
    }
}

const fn max(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_directions_keep_their_bounds_and_leave_room_for_controls() {
        for (transport, client, server) in [
            (
                Transport::MONITOR,
                monitor::CLIENT_LIMITS,
                monitor::SERVER_LIMITS,
            ),
            (
                Transport::TERMINAL,
                terminal::CLIENT_LIMITS,
                terminal::SERVER_LIMITS,
            ),
            (
                Transport::INTEGRATION,
                integration::CLIENT_LIMITS,
                integration::SERVER_LIMITS,
            ),
            (
                Transport::REMOTE_APPS,
                remote_apps::CLIENT_LIMITS,
                remote_apps::SERVER_LIMITS,
            ),
        ] {
            assert_eq!(transport.limits(Direction::ToWorker), client);
            assert_eq!(transport.limits(Direction::FromWorker), server);
        }
        let tiny = Limits {
            message: 1,
            metadata: 1,
        };
        let transport = Transport::Protobuf {
            client: tiny,
            server: tiny,
        };
        for direction in [Direction::ToWorker, Direction::FromWorker] {
            assert_eq!(
                transport.limits(direction),
                Transport::Opaque.limits(direction)
            );
            assert_eq!(Transport::GIT.limits(direction).message, git::MAX_FRAME);
        }
    }
}
