//! Listening for announcements, and never making one.
//!
//! A probe that joins the group and speaks becomes a peer the router can hand work to. This
//! only receives.
//!
//! Listening is also the only way to see a cluster that is not this one. A node decodes an
//! announcement, compares the cluster name and drops a mismatch without a word - correct for a
//! router, and the reason no node can ever report the development machine sharing a switch
//! with production.

use std::collections::BTreeMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

/// The group and port a loken node announces on.
pub const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 42, 99);
pub const PORT: u16 = 41999;

/// What a node says about itself, in the four tab-separated fields it sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub cluster: String,
    pub node_id: String,
    pub endpoint: String,
}

impl Announcement {
    /// Parsed strictly: this arrives as an unauthenticated datagram from anyone on the
    /// network, so anything that is not exactly the expected shape is dropped rather than
    /// interpreted.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut parts = text.split('\t');
        if parts.next()? != "loken1" {
            return None;
        }
        let cluster = parts.next()?.to_string();
        let node_id = parts.next()?.to_string();
        let endpoint = parts.next()?.to_string();
        if parts.next().is_some() || node_id.is_empty() || endpoint.is_empty() {
            return None;
        }
        Some(Self {
            cluster,
            node_id,
            endpoint,
        })
    }
}

/// What one listening window heard.
#[derive(Debug, Default)]
pub struct Heard {
    /// Endpoints announcing the cluster asked for.
    pub ours: BTreeMap<String, String>,
    /// Endpoints announcing some other cluster, keyed by that cluster's name.
    pub foreign: BTreeMap<String, String>,
}

/// Listen for `window`, then stop.
///
/// Both `SO_REUSEADDR` and `SO_REUSEPORT` are set, because a node on this host already holds
/// the port and on Linux the first alone is not enough - the bind still fails with "address
/// already in use" on exactly the machine an operator is most likely to watch from. The caller
/// must still cope with a failure here, since a second listener is not guaranteed to receive
/// on every platform.
pub fn listen(cluster: Option<&str>, window: Duration) -> io::Result<Heard> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT).into())?;
    let socket: UdpSocket = socket.into();
    socket.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;

    let mut heard = Heard::default();
    let deadline = Instant::now() + window;
    let mut buffer = [0u8; 512];
    while Instant::now() < deadline {
        let Ok((n, _)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        let Some(a) = Announcement::decode(&buffer[..n]) else {
            continue;
        };
        match cluster {
            Some(name) if a.cluster != name => {
                heard.foreign.insert(a.cluster, a.endpoint);
            }
            _ => {
                heard.ours.insert(a.node_id, a.endpoint);
            }
        }
    }
    Ok(heard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_well_formed_announcement_parses() {
        let a =
            Announcement::decode(b"loken1\thome\tdesktop\thttp://192.0.2.1:11435").expect("parses");
        assert_eq!(a.cluster, "home");
        assert_eq!(a.node_id, "desktop");
        assert_eq!(a.endpoint, "http://192.0.2.1:11435");
    }

    /// Every malformed shape is dropped rather than half-read. These arrive from anyone.
    #[test]
    fn anything_else_is_dropped() {
        for bad in [
            &b""[..],
            b"loken1",
            b"loken1\thome\tdesktop",                  // no endpoint
            b"loken1\thome\t\thttp://x",               // empty node id
            b"loken1\thome\tdesktop\thttp://x\textra", // a fifth field
            b"loken2\thome\tdesktop\thttp://x",        // another version
            &[0xff, 0xfe, 0xfd],                       // not utf-8
        ] {
            assert!(Announcement::decode(bad).is_none(), "accepted {bad:?}");
        }
    }

    /// An announcement naming another cluster is kept apart, because that is the finding: no
    /// node reports it, since each drops the mismatch in silence.
    #[test]
    fn a_foreign_cluster_is_separated_from_ours() {
        let mut heard = Heard::default();
        for (cluster, node, endpoint) in [
            ("home", "a", "http://192.0.2.1"),
            ("staging", "b", "http://192.0.2.9"),
        ] {
            let a = Announcement {
                cluster: cluster.into(),
                node_id: node.into(),
                endpoint: endpoint.into(),
            };
            if a.cluster == "home" {
                heard.ours.insert(a.node_id, a.endpoint);
            } else {
                heard.foreign.insert(a.cluster, a.endpoint);
            }
        }
        assert_eq!(heard.ours.len(), 1);
        assert_eq!(
            heard.foreign.get("staging").map(String::as_str),
            Some("http://192.0.2.9")
        );
    }
}
