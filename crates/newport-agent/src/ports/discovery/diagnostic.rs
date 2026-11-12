use super::{Budget, Family, Inventory, budget::Failure};
use netlink_packet_core::{
    NLM_F_DUMP, NLM_F_DUMP_INTR, NLM_F_MULTIPART, NLM_F_REQUEST, NetlinkBuffer, NetlinkHeader,
    NetlinkMessage, NetlinkPayload, Nla as _,
};
use netlink_packet_sock_diag::{
    SockDiagMessage,
    inet::{ExtensionFlags, InetRequest, InetResponse, SocketId, StateFlags, nlas::Nla},
};
use netlink_sys::{Socket as NetlinkSocket, SocketAddr, protocols::NETLINK_SOCK_DIAG};
use newport_protocol::monitor::ports::{
    MAX_LISTENERS,
    discovery::{Cgroup, Endpoint, HolderState, Report, Socket, SocketSource},
};
use std::{collections::BTreeSet, io, os::fd::AsRawFd, time::Duration};

const DATAGRAM_LIMIT: usize = 64 << 10;
const DUMP_LIMIT: usize = 4 << 20;
const FAMILY_TIME: Duration = Duration::from_secs(2);
const SEQUENCE: u32 = 1;
const TCP_LISTEN: u8 = 10;
const INET_DIAG_CGROUP_ID: u16 = 21;

pub(super) fn collect(family: Family, budget: &Budget<'_>) -> Inventory {
    let budget = budget.limited(FAMILY_TIME);
    let mut dump = Dump::new(family);
    let mut result = query(&mut dump, &budget);
    if result.as_ref().is_err_and(Failure::retryable) && budget.remaining().is_ok() {
        // A retry is a separate enumeration; never combine its records with a lost dump.
        dump = Dump::new(family);
        result = query(&mut dump, &budget);
    }
    let report = match result {
        Ok(()) => Report::complete(),
        Err(error) => error.report(!dump.sockets.is_empty()),
    };
    Inventory {
        source: SocketSource::Diagnostic,
        sockets: dump.sockets,
        report,
    }
}

fn query(dump: &mut Dump, budget: &Budget<'_>) -> Result<(), Failure> {
    budget.remaining()?;
    let mut socket = NetlinkSocket::new(NETLINK_SOCK_DIAG)?;
    socket.bind_auto()?;
    socket.set_non_blocking(true)?;
    let request = request(dump.family);
    if socket.send_to(&request, &SocketAddr::new(0, 0), 0)? != request.len() {
        return Err(Failure::Invalid("Incomplete socket diagnostic request"));
    }
    let mut storage = vec![0; DATAGRAM_LIMIT];
    while !dump.done {
        wait_readable(&socket, budget)?;
        let mut buffer = storage.as_mut_slice();
        let (received, sender) = match socket.recv_from(&mut buffer, libc::MSG_TRUNC) {
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            result => result?,
        };
        if sender.port_number() != 0 || sender.multicast_groups() != 0 {
            return Err(Failure::Invalid(
                "Socket diagnostics came from a non-kernel sender",
            ));
        }
        if received > storage.len() {
            return Err(Failure::Exhausted(
                "Socket diagnostic datagram exceeded its size limit",
            ));
        }
        dump.consume(&storage[..received])?;
    }
    Ok(())
}

fn request(family: Family) -> Vec<u8> {
    let mut socket_id = match family {
        Family::Ipv4 => SocketId::new_v4(),
        Family::Ipv6 => SocketId::new_v6(),
    };
    socket_id.cookie = [0xff; 8];
    let mut header = NetlinkHeader::default();
    header.flags = NLM_F_REQUEST | NLM_F_DUMP;
    header.sequence_number = SEQUENCE;
    let request = InetRequest {
        family: family.number(),
        protocol: 6,
        extensions: ExtensionFlags::empty(),
        states: StateFlags::LISTEN,
        socket_id,
    };
    let mut message = NetlinkMessage::new(header, SockDiagMessage::InetRequest(request).into());
    message.finalize();
    let mut bytes = vec![0; message.buffer_len()];
    message.serialize(&mut bytes);
    bytes
}

fn wait_readable(socket: &NetlinkSocket, budget: &Budget<'_>) -> Result<(), Failure> {
    loop {
        let remaining = budget.remaining()?.min(Duration::from_millis(50));
        let timeout = i32::try_from(remaining.as_millis())
            .map_err(|_| Failure::TimedOut)?
            .max(1);
        let mut descriptor = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: the borrowed socket stays open; poll receives one initialized descriptor.
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if descriptor.revents & (libc::POLLNVAL | libc::POLLHUP) != 0 {
            return Err(Failure::Invalid(
                "Socket diagnostic descriptor closed during enumeration",
            ));
        }
        // Reading POLLERR exposes ENOBUFS instead of silently accepting a lost dump.
        if descriptor.revents & (libc::POLLIN | libc::POLLERR) != 0 {
            return Ok(());
        }
    }
}

struct Dump {
    family: Family,
    sockets: Vec<Socket>,
    identities: BTreeSet<(u64, Option<u64>)>,
    bytes: usize,
    done: bool,
}

impl Dump {
    fn new(family: Family) -> Self {
        Self {
            family,
            sockets: Vec::new(),
            identities: BTreeSet::new(),
            bytes: 0,
            done: false,
        }
    }

    fn consume(&mut self, mut bytes: &[u8]) -> Result<(), Failure> {
        if bytes.is_empty() {
            return Err(Failure::Invalid("Empty socket diagnostic datagram"));
        }
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or(Failure::Exhausted("Socket diagnostic byte limit reached"))?;
        if self.bytes > DUMP_LIMIT {
            return Err(Failure::Exhausted("Socket diagnostic byte limit reached"));
        }
        while !bytes.is_empty() {
            if self.done {
                return Err(Failure::Invalid(
                    "Socket records arrived after multipart completion",
                ));
            }
            let buffer = NetlinkBuffer::new_checked(bytes)?;
            let length = usize::try_from(buffer.length())
                .map_err(|_| Failure::Invalid("Invalid socket diagnostic length"))?;
            let packet = NetlinkMessage::<SockDiagMessage>::deserialize(&bytes[..length])?;
            if packet.header.sequence_number != SEQUENCE {
                return Err(Failure::Invalid("Socket diagnostic sequence mismatch"));
            }
            if packet.header.flags & NLM_F_DUMP_INTR != 0 {
                return Err(Failure::Interrupted);
            }
            self.packet(packet)?;
            let aligned = length
                .checked_add(3)
                .map(|n| n & !3)
                .ok_or(Failure::Invalid("Invalid socket diagnostic alignment"))?;
            if length == bytes.len() {
                break;
            }
            bytes = bytes
                .get(aligned..)
                .ok_or(Failure::Invalid("Truncated socket diagnostic padding"))?;
        }
        Ok(())
    }

    fn packet(&mut self, packet: NetlinkMessage<SockDiagMessage>) -> Result<(), Failure> {
        match packet.payload {
            NetlinkPayload::InnerMessage(SockDiagMessage::InetResponse(response)) => {
                if packet.header.flags & NLM_F_MULTIPART == 0 {
                    return Err(Failure::Invalid("Socket dump response is not multipart"));
                }
                if self.sockets.len() >= MAX_LISTENERS {
                    return Err(Failure::Exhausted("Socket count limit reached"));
                }
                let mut socket = observation(*response, self.family)?;
                if (socket.cookie.is_some() || socket.inode != 0)
                    && !self.identities.insert((socket.inode, socket.cookie))
                {
                    return Err(Failure::Invalid("Duplicate socket in diagnostic dump"));
                }
                socket.id = u32::try_from(self.sockets.len() + 1)
                    .map_err(|_| Failure::Exhausted("Socket identity limit reached"))?;
                self.sockets.push(socket);
            }
            NetlinkPayload::Done(done) => {
                if done.code != 0 {
                    let code = done
                        .code
                        .checked_neg()
                        .filter(|code| *code > 0)
                        .ok_or(Failure::Invalid("Invalid socket dump completion code"))?;
                    return Err(io::Error::from_raw_os_error(code).into());
                }
                self.done = true;
            }
            NetlinkPayload::Error(error) if error.code.is_none() => {}
            NetlinkPayload::Error(error) => {
                let code = error
                    .raw_code()
                    .checked_neg()
                    .filter(|code| *code > 0)
                    .ok_or(Failure::Invalid("Invalid socket diagnostic error code"))?;
                return Err(io::Error::from_raw_os_error(code).into());
            }
            NetlinkPayload::Overrun(_) => return Err(Failure::Interrupted),
            NetlinkPayload::Noop => {}
            _ => return Err(Failure::Invalid("Unexpected socket diagnostic message")),
        }
        Ok(())
    }
}

fn observation(response: InetResponse, family: Family) -> Result<Socket, Failure> {
    let header = response.header;
    let identity = header.socket_id;
    if header.family != family.number()
        || !family.contains(identity.source_address)
        || header.state != TCP_LISTEN
        || identity.source_port == 0
    {
        return Err(Failure::Invalid(
            "Unexpected family, state, or port in socket inventory",
        ));
    }
    let mut cgroup = None;
    let mut ipv6_only = None;
    for attribute in response.nlas {
        match attribute {
            Nla::SkV6Only(value) => {
                if family != Family::Ipv6 || ipv6_only.replace(value).is_some() {
                    return Err(Failure::Invalid(
                        "Invalid or repeated IPv6-only socket attribute",
                    ));
                }
            }
            Nla::Other(value) if value.kind() == INET_DIAG_CGROUP_ID => {
                if value.value_len() != 8 || cgroup.is_some() {
                    return Err(Failure::Invalid(
                        "Invalid or repeated socket cgroup identity",
                    ));
                }
                let mut bytes = [0; 8];
                value.emit_value(&mut bytes);
                let id = u64::from_ne_bytes(bytes);
                if id == 0 {
                    return Err(Failure::Invalid("Empty socket cgroup identity"));
                }
                cgroup = Some(Cgroup { id, path: None });
            }
            _ => {}
        }
    }
    Ok(Socket {
        id: 0,
        endpoint: Endpoint {
            address: identity.source_address,
            port: identity.source_port,
            interface: (identity.interface_id != 0).then_some(identity.interface_id),
        },
        source: SocketSource::Diagnostic,
        inode: u64::from(header.inode),
        cookie: cookie(identity.cookie),
        uid: header.uid,
        account: None,
        ipv6_only,
        cgroup,
        holders: Vec::new(),
        holder_state: HolderState::Unresolved,
        holders_truncated: false,
        services: Vec::new(),
    })
}

fn cookie(bytes: [u8; 8]) -> Option<u64> {
    if bytes == [0xff; 8] {
        return None;
    }
    // Linux stores two native-endian u32 words, low word first on both endiannesses.
    let low = u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let high = u32::from_ne_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    Some(u64::from(low) | (u64::from(high) << 32))
}

#[cfg(test)]
mod tests;
