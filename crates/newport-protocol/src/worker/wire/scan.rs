use super::{CONTROL_FIELD, Error, FrameError};
use crate::encoding::protobuf::fields::{self, field};

/// Parse all top-level fields before returning a control. Unknown service
/// fields are preserved, but none may accompany a control.
pub(super) fn control(mut body: &[u8], limit: usize) -> Result<Option<&[u8]>, Error> {
    if body.len() > limit {
        return Err(FrameError::TooLarge {
            length: body.len() as u64,
            limit,
        }
        .into());
    }
    let mut control = None;
    let mut data = false;
    while !body.is_empty() {
        let field = field(&mut body)?;
        if field.number == CONTROL_FIELD {
            if control.is_some() {
                return Err(Error::DuplicateControl);
            }
            control = Some(field.bytes()?);
        } else {
            data = true;
        }
    }
    if control.is_some() && data {
        return Err(Error::MixedControl);
    }
    Ok(control)
}

pub(super) fn single(body: &[u8]) -> Result<fields::Field<'_>, Error> {
    fields::single(body).map_err(|_| Error::InvalidOperation)
}
