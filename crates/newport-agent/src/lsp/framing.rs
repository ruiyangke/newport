//! LSP base-protocol messages: headers, a blank line, then `Content-Length` bytes.
use std::io::{self, BufRead, Read};

/// A larger message ends the server: nothing the editor asks for needs more.
pub(super) const MAX_MESSAGE: usize = 32 * 1024 * 1024;
const MAX_HEADER_LINE: usize = 1024;
const MAX_HEADERS: usize = 8;

/// The message as forwarded, with only its `Content-Length` header.
pub(super) fn encode(body: &[u8]) -> Vec<u8> {
    let mut message = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    message.extend_from_slice(body);
    message
}

/// Read one whole message, or `None` at a clean end of input.
pub(super) fn read_message(input: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut length = None;
    let mut headers = 0;
    loop {
        let mut line = Vec::new();
        let read = input
            .by_ref()
            .take(MAX_HEADER_LINE as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return if headers == 0 {
                Ok(None)
            } else {
                Err(invalid(
                    "The language server's output ended inside a message.",
                ))
            };
        }
        if line.len() > MAX_HEADER_LINE || !line.ends_with(b"\n") {
            return Err(invalid(
                "The language server sent an invalid message header.",
            ));
        }
        let line = line
            .strip_suffix(b"\r\n")
            .unwrap_or(&line[..line.len() - 1]);
        if line.is_empty() {
            if headers == 0 {
                // Tolerate blank lines between messages.
                continue;
            }
            break;
        }
        headers += 1;
        if headers > MAX_HEADERS {
            return Err(invalid(
                "The language server sent too many message headers.",
            ));
        }
        let line = std::str::from_utf8(line)
            .map_err(|_| invalid("The language server sent an invalid message header."))?;
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| invalid("The language server sent an invalid message header."))?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            let value: usize = value
                .trim()
                .parse()
                .map_err(|_| invalid("The language server sent an invalid message length."))?;
            if value > MAX_MESSAGE {
                return Err(invalid(
                    "The language server sent a message larger than 32 MiB.",
                ));
            }
            length = Some(value);
        }
    }
    let length =
        length.ok_or_else(|| invalid("The language server sent a message without a length."))?;
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    Ok(Some(encode(&body)))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_read_whole_and_reencoded_with_only_their_length() {
        let input = b"Content-Length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n{}\r\nContent-Length: 4\r\n\r\nnull";
        let mut input = &input[..];
        assert_eq!(
            read_message(&mut input).unwrap().unwrap(),
            b"Content-Length: 2\r\n\r\n{}"
        );
        assert_eq!(
            read_message(&mut input).unwrap().unwrap(),
            b"Content-Length: 4\r\n\r\nnull"
        );
        assert!(read_message(&mut input).unwrap().is_none());
    }

    #[test]
    fn malformed_or_oversized_messages_are_refused() {
        for input in [
            &b"Content-Length: x\r\n\r\n"[..],
            b"Content-Type: text\r\n\r\n{}",
            b"Content-Length: 99999999999\r\n\r\n",
            b"Content-Length: 4\r\n\r\n{}",
            b"Content-Length: 2\r\n",
        ] {
            let mut input = input;
            assert!(read_message(&mut input).is_err(), "{input:?}");
        }
        let long = format!("X-Long: {}\r\n\r\n", "a".repeat(2000));
        assert!(read_message(&mut long.as_bytes()).is_err());
    }
}
