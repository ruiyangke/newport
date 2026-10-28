//! Local retained attachment policy, deliberately absent from helper IPC.
use newport_domain::terminal::SharedGuiEnvironment;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Policy {
    #[default]
    PreserveInherited,
    OwnedNative,
}

/// Created by the GUI adapter only after its private policy handshake. The
/// containing Prepared/Lease supplies runtime, worker and generation identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attachment {
    owned_address: Option<String>,
}
impl Attachment {
    pub fn owned(environment: &SharedGuiEnvironment) -> Result<Self, String> {
        environment.validate()?;
        Ok(Self {
            owned_address: Some(
                environment
                    .fallback_bus
                    .clone()
                    .ok_or("Owned terminal bus requires the display's private bus address")?,
            ),
        })
    }
    pub fn policy(&self) -> Policy {
        if self.owned_address.is_some() {
            Policy::OwnedNative
        } else {
            Policy::PreserveInherited
        }
    }
    pub fn address(&self) -> Option<&str> {
        self.owned_address.as_deref()
    }
    pub(crate) fn validate(
        &self,
        policy: Policy,
        environment: &SharedGuiEnvironment,
    ) -> Result<(), String> {
        environment.validate()?;
        if self.policy() != policy
            || (policy == Policy::OwnedNative && self.owned_address != environment.fallback_bus)
        {
            return Err(
                "Graphical attachment does not match the selected terminal bus policy".into(),
            );
        }
        Ok(())
    }
}
