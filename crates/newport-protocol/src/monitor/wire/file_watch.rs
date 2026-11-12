use super::super::file_watch as contract;
use super::{Invalid, Record, pb};

impl Record for contract::Request {
    type Wire = pb::WatchRequest;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            watch: self.watch.clone(),
            root: self.root.clone(),
            directories: self.directories.clone(),
            cursor: self.cursor.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            watch: value.watch,
            root: value.root,
            directories: value.directories,
            cursor: value.cursor,
        })
    }
}

impl Record for contract::Reply {
    type Wire = pb::WatchReply;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            cursor: self.cursor.clone(),
            paths: self.paths.clone(),
            reset: self.reset,
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            cursor: value.cursor,
            paths: value.paths,
            reset: value.reset,
        })
    }
}
