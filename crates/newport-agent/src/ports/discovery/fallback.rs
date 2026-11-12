use super::{Budget, Family, Inventory, budget::Failure};
use newport_protocol::monitor::ports::{
    MAX_FIELD, MAX_LISTENERS,
    discovery::{Endpoint, HolderState, Report, Socket, SocketSource},
};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::Path,
};

const TABLE_BYTES: usize = 4 << 20;

pub(super) fn collect(root: &Path, family: Family, budget: &Budget<'_>) -> Inventory {
    let mut sockets = Vec::new();
    let result = (|| {
        budget.remaining()?;
        let path = match family {
            Family::Ipv4 => "proc/net/tcp",
            Family::Ipv6 => "proc/net/tcp6",
        };
        let file = File::open(root.join(path))?;
        table(&mut BufReader::new(file), family, budget, &mut sockets)
    })();
    let report = match result {
        Ok(()) => Report::complete(),
        Err(error) => Failure::report(&error, !sockets.is_empty()),
    };
    Inventory {
        source: SocketSource::Proc,
        sockets,
        report,
    }
}

fn table(
    reader: &mut impl BufRead,
    family: Family,
    budget: &Budget<'_>,
    sockets: &mut Vec<Socket>,
) -> Result<(), Failure> {
    let mut used = 0;
    let header = line(reader, budget, &mut used)?
        .ok_or(Failure::Invalid("Missing proc socket table header"))?;
    let header = std::str::from_utf8(&header)
        .map_err(|_| Failure::Invalid("Invalid proc socket table header"))?;
    let mut columns = header.split_whitespace();
    if columns.next() != Some("sl") || columns.next() != Some("local_address") {
        return Err(Failure::Invalid("Invalid proc socket table header"));
    }
    while let Some(bytes) = line(reader, budget, &mut used)? {
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| Failure::Invalid("Non-ASCII proc socket record"))?;
        if let Some(mut socket) = record(text, family)? {
            if sockets.len() >= MAX_LISTENERS {
                return Err(Failure::Exhausted("Socket count limit reached"));
            }
            socket.id = u32::try_from(sockets.len() + 1)
                .map_err(|_| Failure::Exhausted("Socket identity limit reached"))?;
            sockets.push(socket);
        }
    }
    Ok(())
}

fn line(
    reader: &mut impl BufRead,
    budget: &Budget<'_>,
    used: &mut usize,
) -> Result<Option<Vec<u8>>, Failure> {
    let mut line = Vec::new();
    loop {
        budget.remaining()?;
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return Ok((!line.is_empty()).then_some(line));
        }
        let end = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1);
        let count = end.unwrap_or(bytes.len());
        if count > MAX_FIELD.saturating_sub(line.len()) {
            return Err(Failure::Exhausted(
                "Proc socket line exceeded its size limit",
            ));
        }
        if count > TABLE_BYTES.saturating_sub(*used) {
            return Err(Failure::Exhausted(
                "Proc socket table exceeded its size limit",
            ));
        }
        line.extend_from_slice(&bytes[..count]);
        *used += count;
        reader.consume(count);
        if end.is_some() {
            return Ok(Some(line));
        }
    }
}

fn record(text: &str, family: Family) -> Result<Option<Socket>, Failure> {
    let mut fields = text.split_whitespace();
    let _slot = fields
        .next()
        .ok_or(Failure::Invalid("Missing proc socket slot"))?;
    let local = fields
        .next()
        .ok_or(Failure::Invalid("Missing proc socket endpoint"))?;
    let _remote = fields
        .next()
        .ok_or(Failure::Invalid("Missing proc socket peer"))?;
    let state = fields
        .next()
        .and_then(|s| u8::from_str_radix(s, 16).ok())
        .ok_or(Failure::Invalid("Invalid proc socket state"))?;
    if state != 10 {
        return Ok(None);
    }
    let uid = fields
        .nth(3)
        .and_then(|s| s.parse().ok())
        .ok_or(Failure::Invalid("Invalid proc socket UID"))?;
    let inode = fields
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or(Failure::Invalid("Invalid proc socket inode"))?;
    let (host, port) = local
        .split_once(':')
        .ok_or(Failure::Invalid("Invalid proc socket endpoint"))?;
    let address = address(host, family, cfg!(target_endian = "little"))?;
    let port = u16::from_str_radix(port, 16)
        .ok()
        .filter(|port| *port != 0)
        .ok_or(Failure::Invalid("Invalid proc socket port"))?;
    Ok(Some(Socket {
        id: 0,
        endpoint: Endpoint {
            address,
            port,
            interface: None,
        },
        source: SocketSource::Proc,
        inode,
        cookie: None,
        uid,
        account: None,
        ipv6_only: None,
        cgroup: None,
        holders: Vec::new(),
        holder_state: HolderState::Unresolved,
        holders_truncated: false,
        services: Vec::new(),
    }))
}

fn address(hex: &str, family: Family, little_endian: bool) -> Result<IpAddr, Failure> {
    let size = match family {
        Family::Ipv4 => 4,
        Family::Ipv6 => 16,
    };
    if hex.len() != size * 2 {
        return Err(Failure::Invalid("Invalid proc socket address length"));
    }
    let mut bytes = [0; 16];
    for (word, output) in hex
        .as_bytes()
        .as_chunks::<8>()
        .0
        .iter()
        .zip(bytes.as_chunks_mut::<4>().0.iter_mut())
    {
        let value = std::str::from_utf8(word)
            .ok()
            .and_then(|text| u32::from_str_radix(text, 16).ok())
            .ok_or(Failure::Invalid("Invalid proc socket address"))?;
        output.copy_from_slice(&if little_endian {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        });
    }
    Ok(match family {
        Family::Ipv4 => Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]).into(),
        Family::Ipv6 => Ipv6Addr::from(bytes).into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Cursor,
        time::{Duration, Instant},
    };
    const HEADER: &str = "  sl local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n";
    const LISTENER: &str =
        " 0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 1234 1 0\n";

    fn scan(text: &str) -> (Vec<Socket>, Result<(), Failure>) {
        let mut sockets = Vec::new();
        let result = table(
            &mut Cursor::new(text),
            Family::Ipv4,
            &Budget::new(Instant::now() + Duration::from_secs(1), &|| false),
            &mut sockets,
        );
        (sockets, result)
    }

    #[test]
    fn proc_addresses_follow_native_word_order_for_both_families() {
        for (little, v4, v6) in [
            (true, "0100007F", "B80D0120000000000000000001000000"),
            (false, "7F000001", "20010DB8000000000000000000000001"),
        ] {
            assert_eq!(
                address(v4, Family::Ipv4, little).unwrap().to_string(),
                "127.0.0.1"
            );
            assert_eq!(
                address(v6, Family::Ipv6, little).unwrap().to_string(),
                "2001:db8::1"
            );
        }
        assert!(address("0100007F", Family::Ipv6, true).is_err());
        assert!(address("0100007Fextra", Family::Ipv4, true).is_err());
    }

    #[test]
    fn fallback_preserves_uid_without_inventing_optional_identity() {
        let (sockets, result) = scan(&format!("{HEADER}{LISTENER}"));
        result.unwrap();
        assert_eq!(sockets.len(), 1);
        let socket = &sockets[0];
        assert_eq!(socket.uid, 0);
        assert_eq!(socket.inode, 1234);
        assert_eq!(socket.endpoint.port, 8080);
        assert!(socket.cookie.is_none());
        assert!(socket.cgroup.is_none());
        assert!(socket.holders.is_empty());
    }

    #[test]
    fn empty_success_requires_a_valid_header_and_complete_read() {
        assert!(scan("").1.is_err());
        assert!(scan("nonsense\n").1.is_err());
        assert!(scan(HEADER).1.is_ok());
        let (sockets, result) = scan(&format!("{HEADER}{LISTENER}broken\n"));
        assert_eq!(sockets.len(), 1);
        assert_eq!(
            result.unwrap_err().report(true).outcome,
            newport_protocol::monitor::ports::discovery::Outcome::Partial
        );
    }

    #[test]
    fn proc_reads_are_bounded_before_allocating_a_whole_file_or_line() {
        let (sockets, result) = scan(&format!("{HEADER}{LISTENER}{}", "0".repeat(MAX_FIELD + 1)));
        assert_eq!(sockets.len(), 1);
        assert!(matches!(result, Err(Failure::Exhausted(_))));
        let (sockets, result) = scan(&format!("{HEADER}{}", LISTENER.repeat(MAX_LISTENERS + 1)));
        assert_eq!(sockets.len(), MAX_LISTENERS);
        assert!(matches!(result, Err(Failure::Exhausted(_))));
        let mut used = TABLE_BYTES;
        assert!(
            line(
                &mut Cursor::new(HEADER),
                &Budget::new(Instant::now() + Duration::from_secs(1), &|| false),
                &mut used
            )
            .is_err()
        );
    }

    #[test]
    fn missing_family_table_is_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let result = collect(
            root.path(),
            Family::Ipv6,
            &Budget::new(Instant::now() + Duration::from_secs(1), &|| false),
        );
        assert_eq!(
            result.report.outcome,
            newport_protocol::monitor::ports::discovery::Outcome::Unavailable
        );
        assert!(!result.report.issues.is_empty());
    }
}
