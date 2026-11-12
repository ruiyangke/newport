//! Optional copy-only remote drag referencing an already uploaded batch.
use crate::remote_apps::v1::WindowId;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Client {
    Begin {
        drag: u64,
        transfer: u64,
        window: WindowId,
        surface: u64,
        point: [u32; 2],
    },
    Motion {
        drag: u64,
        point: [u32; 2],
    },
    Drop {
        drag: u64,
    },
    Cancel {
        drag: u64,
    },
}
impl Client {
    pub fn id(&self) -> u64 {
        match self {
            Self::Begin { drag, .. }
            | Self::Motion { drag, .. }
            | Self::Drop { drag }
            | Self::Cancel { drag } => *drag,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.id() == 0 {
            return Err("Invalid drag identity".into());
        }
        if let Self::Begin {
            transfer,
            window,
            surface,
            ..
        } = self
        {
            window.validate()?;
            if *transfer == 0 || *surface == 0 {
                return Err("Invalid drag source or surface".into());
            }
        }
        if let Self::Begin { point, .. } | Self::Motion { point, .. } = self
            && point.iter().any(|p| *p >= 4096)
        {
            return Err("Invalid drag coordinates".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Server {
    Status {
        drag: u64,
        /// Zero for Begin; incremented for every Motion on the ordered stream.
        revision: u64,
        accepted: bool,
    },
    Finished {
        drag: u64,
        copied: bool,
        reason: String,
    },
}
impl Server {
    pub fn validate(&self) -> Result<(), String> {
        let id = match self {
            Self::Status { drag, .. } | Self::Finished { drag, .. } => *drag,
        };
        if id == 0 {
            return Err("Invalid drag response".into());
        }
        if let Self::Finished { reason, .. } = self
            && reason.len() > 256
        {
            return Err("Invalid drag completion".into());
        }
        Ok(())
    }
}
