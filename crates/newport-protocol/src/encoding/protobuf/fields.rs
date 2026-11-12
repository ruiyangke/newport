//! Strict, allocation-free fields used before decoding protocol oneofs.
use super::FrameError;

pub(crate) struct Field<'a> {
    pub number: u32,
    value: Value<'a>,
}

enum Value<'a> {
    Integer(u64),
    Bytes(&'a [u8]),
    Fixed(usize),
}

impl<'a> Field<'a> {
    pub fn u64(&self) -> Result<u64, FrameError> {
        match self.value {
            Value::Integer(value) => Ok(value),
            _ => Err(FrameError::Malformed),
        }
    }

    pub fn bytes(&self) -> Result<&'a [u8], FrameError> {
        match self.value {
            Value::Bytes(bytes) => Ok(bytes),
            _ => Err(FrameError::Malformed),
        }
    }

    pub fn u32(&self) -> Result<u32, FrameError> {
        match self.value {
            Value::Integer(value) => u32::try_from(value).map_err(|_| FrameError::Malformed),
            _ => Err(FrameError::Malformed),
        }
    }

    pub fn fixed64_count(&self) -> Result<usize, FrameError> {
        match self.value {
            Value::Fixed(8) => Ok(1),
            Value::Bytes(bytes) if bytes.len().is_multiple_of(8) => Ok(bytes.len() / 8),
            _ => Err(FrameError::Malformed),
        }
    }
}

pub(crate) fn single(mut body: &[u8]) -> Result<Field<'_>, FrameError> {
    if body.is_empty() {
        return Err(FrameError::Malformed);
    }
    let value = field(&mut body)?;
    if !body.is_empty() {
        return Err(FrameError::Malformed);
    }
    Ok(value)
}

pub(crate) fn field<'a>(body: &mut &'a [u8]) -> Result<Field<'a>, FrameError> {
    let key = varint(body)?;
    let number = u32::try_from(key >> 3).map_err(|_| FrameError::Malformed)?;
    if number == 0 || number > (1 << 29) - 1 {
        return Err(FrameError::Malformed);
    }
    let value = match key & 7 {
        0 => Value::Integer(varint(body)?),
        1 => {
            take(body, 8)?;
            Value::Fixed(8)
        }
        2 => {
            let length = usize::try_from(varint(body)?).map_err(|_| FrameError::Malformed)?;
            Value::Bytes(take(body, length)?)
        }
        5 => {
            take(body, 4)?;
            Value::Fixed(4)
        }
        _ => return Err(FrameError::Malformed),
    };
    Ok(Field { number, value })
}

fn take<'a>(body: &mut &'a [u8], length: usize) -> Result<&'a [u8], FrameError> {
    let (value, rest) = body.split_at_checked(length).ok_or(FrameError::Malformed)?;
    *body = rest;
    Ok(value)
}

fn varint(body: &mut &[u8]) -> Result<u64, FrameError> {
    let mut value = 0;
    for index in 0..10 {
        let byte = *take(body, 1)?.first().ok_or(FrameError::Malformed)?;
        if index == 9 && byte > 1 {
            return Err(FrameError::Malformed);
        }
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(FrameError::Malformed)
}
