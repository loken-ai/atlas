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
use std::sync::{Arc, Mutex};
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
    /// Why nothing can be heard right now, when the socket could not be opened.
    pub problem: Option<String>,
}

impl Heard {
    /// Files an announcement under ours or foreign. With no cluster name asked for, every
    /// announcement is ours.
    pub fn record(&mut self, a: Announcement, cluster: Option<&str>) {
        match cluster {
            Some(name) if a.cluster != name => {
                self.foreign.insert(a.cluster, a.endpoint);
            }
            _ => {
                self.ours.insert(a.node_id, a.endpoint);
            }
        }
    }
}

/// Joins the group on the discovery port.
///
/// Both `SO_REUSEADDR` and `SO_REUSEPORT` are set, because a node on this host already holds
/// the port and on Linux the first alone is not enough - the bind still fails with "address
/// already in use" on exactly the machine an operator is most likely to watch from. The caller
/// must still cope with a failure here, since a second listener is not guaranteed to receive
/// on every platform.
fn bind() -> io::Result<UdpSocket> {
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
    Ok(socket)
}

/// Listen for `window`, then stop.
pub fn listen(cluster: Option<&str>, window: Duration) -> io::Result<Heard> {
    let socket = bind()?;
    let mut heard = Heard::default();
    let deadline = Instant::now() + window;
    let mut buffer = [0u8; 512];
    while Instant::now() < deadline {
        let Ok((n, _)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        if let Some(a) = Announcement::decode(&buffer[..n]) {
            heard.record(a, cluster);
        }
    }
    Ok(heard)
}

/// Every announcement heard so far, each with when it was last heard.
#[derive(Debug, Default)]
struct Table {
    ours: BTreeMap<String, (String, Instant)>,
    foreign: BTreeMap<String, (String, Instant)>,
    problem: Option<String>,
}

impl Table {
    fn record(&mut self, a: Announcement, cluster: Option<&str>, at: Instant) {
        match cluster {
            Some(name) if a.cluster != name => {
                self.foreign.insert(a.cluster, (a.endpoint, at));
            }
            _ => {
                self.ours.insert(a.node_id, (a.endpoint, at));
            }
        }
    }

    fn since(&self, t: Instant) -> Heard {
        let recent = |m: &BTreeMap<String, (String, Instant)>| {
            m.iter()
                .filter(|(_, (_, at))| *at >= t)
                .map(|(k, (e, _))| (k.clone(), e.clone()))
                .collect()
        };
        Heard {
            ours: recent(&self.ours),
            foreign: recent(&self.foreign),
            problem: self.problem.clone(),
        }
    }
}

/// Listening for as long as the view runs, on its own thread: the socket read blocks for its
/// timeout, which a runtime worker must not do.
pub struct Listener {
    table: Arc<Mutex<Table>>,
}

impl Listener {
    /// Starts listening. A socket that cannot be opened is retried every `retry`, and the
    /// reason is reported by `heard_since` until it opens.
    pub fn spawn(cluster: Option<String>, retry: Duration) -> Self {
        let table = Arc::new(Mutex::new(Table::default()));
        let shared = table.clone();
        std::thread::spawn(move || loop {
            let socket = match bind() {
                Ok(s) => s,
                Err(e) => {
                    lock(&shared).problem = Some(format!("no multicast listening ({e})"));
                    std::thread::sleep(retry);
                    continue;
                }
            };
            lock(&shared).problem = None;
            let mut buffer = [0u8; 512];
            loop {
                let Ok((n, _)) = socket.recv_from(&mut buffer) else {
                    continue;
                };
                if let Some(a) = Announcement::decode(&buffer[..n]) {
                    lock(&shared).record(a, cluster.as_deref(), Instant::now());
                }
            }
        });
        Self { table }
    }

    /// What was heard at or after `t`, with the reason nothing can be heard if there is one.
    pub fn heard_since(&self, t: Instant) -> Heard {
        lock(&self.table).since(t)
    }
}

fn lock(table: &Mutex<Table>) -> std::sync::MutexGuard<'_, Table> {
    table.lock().unwrap_or_else(|e| e.into_inner())
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
            heard.record(
                Announcement {
                    cluster: cluster.into(),
                    node_id: node.into(),
                    endpoint: endpoint.into(),
                },
                Some("home"),
            );
        }
        assert_eq!(heard.ours.len(), 1);
        assert_eq!(
            heard.foreign.get("staging").map(String::as_str),
            Some("http://192.0.2.9")
        );
    }

    /// A node heard before the window asked for is left out: it may have gone since.
    #[test]
    fn only_what_was_heard_since_counts() {
        let start = Instant::now();
        let later = start + Duration::from_secs(2);
        let mut table = Table::default();
        let a = |node: &str, endpoint: &str| Announcement {
            cluster: "home".into(),
            node_id: node.into(),
            endpoint: endpoint.into(),
        };
        table.record(a("old", "http://192.0.2.1"), Some("home"), start);
        table.record(a("new", "http://192.0.2.2"), Some("home"), later);
        let heard = table.since(start + Duration::from_secs(1));
        assert_eq!(heard.ours.keys().collect::<Vec<_>>(), vec!["new"]);
        assert_eq!(table.since(start).ours.len(), 2);
    }
}
