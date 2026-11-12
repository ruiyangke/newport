#![allow(dead_code)]
use newport_protocol::{
    clipboard::RouteReply,
    runtime::{Applied, ReplyValue},
    worker::admission::Endpoint,
};

pub trait Expected {
    fn permissions(self) -> Applied;
    fn route(self) -> RouteReply;
    fn endpoint(self) -> Endpoint;
}
impl Expected for ReplyValue {
    fn permissions(self) -> Applied {
        match self {
            Self::Permissions(value) => value,
            other => panic!("Expected permissions: {other:?}"),
        }
    }
    fn route(self) -> RouteReply {
        match self {
            Self::ClipboardRoute(value) => value,
            other => panic!("Expected route: {other:?}"),
        }
    }
    fn endpoint(self) -> Endpoint {
        match self {
            Self::WorkerStream(value) => value,
            other => panic!("Expected endpoint: {other:?}"),
        }
    }
}
