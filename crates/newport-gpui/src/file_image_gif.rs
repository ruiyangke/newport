//! Read only GIF repeat metadata; pixel decoding remains in the image crate.
//!
//! image 0.25 conflates an absent extension with infinite repeats. Walk framed
//! blocks rather than searching bytes, which could match compressed pixels or
//! comments. Each read is bounded by the already size-limited input.
pub(super) fn plays(bytes: &[u8]) -> Result<Option<u32>, String> {
    let mut reader = Reader(bytes);
    let header = reader.take(13)?;
    if !matches!(&header[..6], b"GIF87a" | b"GIF89a") {
        return Err(INVALID.into());
    }
    reader.palette(header[10])?;
    // No extension means one presentation. Netscape counts *repeats* after it.
    let mut plays = Some(1);
    loop {
        match reader.byte()? {
            0x3b => return Ok(plays),
            0x2c => {
                let descriptor = reader.take(9)?;
                reader.palette(descriptor[8])?;
                reader.byte()?; // LZW minimum code size; image validates it.
                reader.sub_blocks()?;
            }
            0x21 => {
                let label = reader.byte()?;
                if label != 0xff {
                    reader.sub_blocks()?;
                    continue;
                }
                let app = reader.sub_block()?;
                if app.is_empty() {
                    continue;
                }
                let recognized = matches!(app, b"NETSCAPE2.0" | b"ANIMEXTS1.0");
                loop {
                    let block = reader.sub_block()?;
                    if block.is_empty() {
                        break;
                    }
                    if recognized && block.len() == 3 && block[0] == 1 {
                        let repeats = u16::from_le_bytes([block[1], block[2]]);
                        plays = (repeats != 0).then_some(u32::from(repeats) + 1);
                    }
                }
            }
            _ => return Err(INVALID.into()),
        }
    }
}

const INVALID: &str = "The GIF animation metadata is invalid.";
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let (taken, rest) = self.0.split_at_checked(count).ok_or(INVALID)?;
        self.0 = rest;
        Ok(taken)
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn palette(&mut self, flags: u8) -> Result<(), String> {
        if flags & 0x80 != 0 {
            self.take(3 * (1usize << ((flags & 7) + 1)))?;
        }
        Ok(())
    }
    fn sub_block(&mut self) -> Result<&'a [u8], String> {
        let count = usize::from(self.byte()?);
        self.take(count)
    }
    fn sub_blocks(&mut self) -> Result<(), String> {
        while !self.sub_block()?.is_empty() {}
        Ok(())
    }
}
