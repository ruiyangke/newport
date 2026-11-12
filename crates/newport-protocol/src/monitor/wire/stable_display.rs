use super::super::stable_display as contract;
use super::{Invalid, Record, pb};

impl Record for contract::State {
    type Wire = pb::DisplayState;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            running: self.running,
            viewer: self.viewer.as_ref().map(Record::to_wire),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            running: value.running,
            viewer: value.viewer.map(contract::Viewer::from_wire).transpose()?,
        })
    }
}

impl Record for contract::Viewer {
    type Wire = pb::DisplayViewer;
    fn to_wire(&self) -> Self::Wire {
        Self::Wire {
            session: self.session.clone(),
            computer: self.computer.clone(),
        }
    }
    fn from_wire(value: Self::Wire) -> Result<Self, Invalid> {
        Ok(Self {
            session: value.session,
            computer: value.computer,
        })
    }
}
