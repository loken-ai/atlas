//! Where a node address comes from, for both binaries.
//!
//! None are compiled in: this tool exists partly because the harness it replaces curled two
//! hardcoded addresses that would rot in a repository. The flags and the config file are
//! unioned. The one-shot commands add what one listening window hears; the views keep
//! listening and refresh the list every round.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::discovery::{Heard, Listener};
use crate::model::ClusterSnapshot;

/// Endpoints and the cluster name, from the flags and an optional config file. The file has
/// the same `[cluster]` block a node reads, so an operator points atlas at the node's own
/// configuration rather than restating it.
pub fn from_flags_and_file(
    nodes: &[String],
    config: Option<&std::path::Path>,
) -> anyhow::Result<(Vec<String>, Option<String>)> {
    let mut out = nodes.to_vec();
    let mut cluster = None;
    if let Some(path) = config {
        #[derive(serde::Deserialize)]
        struct File {
            cluster: Option<Section>,
        }
        #[derive(serde::Deserialize)]
        struct Section {
            name: Option<String>,
            #[serde(default)]
            join: Vec<String>,
            advertise: Option<String>,
        }
        let file: File = toml::from_str(&std::fs::read_to_string(path)?)?;
        if let Some(section) = file.cluster {
            cluster = section.name;
            out.extend(section.advertise);
            out.extend(section.join);
        }
    }
    let mut out: Vec<String> = out.iter().map(|e| normalise(e)).collect();
    out.sort();
    out.dedup();
    Ok((out, cluster))
}

/// One spelling per address: a scheme when none was given, no trailing slash. A bare
/// `host:port` is what an operator types, and a client refuses it without a scheme.
pub fn normalise(endpoint: &str) -> String {
    let e = endpoint.trim().trim_end_matches('/');
    if e.contains("://") {
        e.to_string()
    } else {
        format!("http://{e}")
    }
}

/// Listen once for announcements and fold what was heard into the seed list.
///
/// Listening, never announcing. Returns the clusters that are not ours separately, because no
/// node can report them: each drops a name mismatch in silence.
pub fn add_heard(
    endpoints: &mut Vec<String>,
    cluster: Option<&str>,
    window: Duration,
) -> (BTreeMap<String, String>, Option<String>) {
    match crate::discovery::listen(cluster, window) {
        Ok(heard) => {
            endpoints.extend(heard.ours.into_values().map(|e| normalise(&e)));
            endpoints.sort();
            endpoints.dedup();
            (heard.foreign, None)
        }
        // A node on this host already holds the port, on the machine most likely to be watched
        // from. Report it as text rather than swallowing it: the caller decides where it goes.
        Err(e) => (
            BTreeMap::new(),
            Some(format!("no multicast listening ({e}); seeds only")),
        ),
    }
}

/// The endpoints for one round of a view.
///
/// The seeds always stay, drawn unreached when down. A node heard announcing is added. A node
/// found earlier that has gone silent stays while it still answers, since multicast can be
/// lost on a path that carries HTTP; silent and unreachable, it leaves.
pub fn current<'a>(
    seeds: &[String],
    heard: &Heard,
    answered: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut out: Vec<String> = seeds.to_vec();
    out.extend(heard.ours.values().map(|e| normalise(e)));
    out.extend(answered.into_iter().map(normalise));
    out.sort();
    out.dedup();
    out
}

/// What a long-running view polls, refreshed every round from the seeds and from what the
/// listener still hears.
pub struct Roster {
    seeds: Vec<String>,
    listener: Option<Listener>,
    every: Duration,
    /// When the previous round started: what was heard since then is still announcing.
    since: Instant,
    endpoints: Vec<String>,
}

impl Roster {
    /// Listening, never announcing, and only when `discover` is set.
    pub fn new(
        seeds: Vec<String>,
        cluster: Option<String>,
        discover: bool,
        every: Duration,
    ) -> Self {
        Self {
            listener: discover.then(|| Listener::spawn(cluster, every)),
            endpoints: seeds.clone(),
            seeds,
            every,
            since: Instant::now(),
        }
    }

    pub fn endpoints(&self) -> &[String] {
        &self.endpoints
    }

    /// Starts a round after `previous`, and returns the endpoints that joined with it along
    /// with what was heard. A sender counts as announcing if heard since the previous round
    /// started, or within one period when that round was cut short.
    pub fn refresh(&mut self, previous: &ClusterSnapshot) -> (Vec<String>, Heard) {
        let now = Instant::now();
        let since = now
            .checked_sub(self.every)
            .map_or(self.since, |t| t.min(self.since));
        let heard = self
            .listener
            .as_ref()
            .map(|l| l.heard_since(since))
            .unwrap_or_default();
        let answered = previous
            .nodes
            .iter()
            .filter(|n| n.rtt_ms.is_some())
            .map(|n| n.endpoint.as_str());
        let next = current(&self.seeds, &heard, answered);
        let joined = next
            .iter()
            .filter(|e| !self.endpoints.contains(e))
            .cloned()
            .collect();
        self.endpoints = next;
        self.since = now;
        (joined, heard)
    }

    /// Whether a node is announcing that this round does not poll yet, so the view can start
    /// the next round now rather than at the end of its period.
    pub fn newcomer(&self) -> bool {
        self.listener.as_ref().is_some_and(|l| {
            l.heard_since(self.since)
                .ours
                .values()
                .any(|e| !self.endpoints.contains(&normalise(e)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An operator types `host:port`; a client refuses it without a scheme. One spelling
    /// per address is also what lets two sources of the same node be told apart.
    #[test]
    fn a_bare_host_and_port_gets_a_scheme_and_loses_its_slash() {
        assert_eq!(normalise("localhost:11435"), "http://localhost:11435");
        assert_eq!(
            normalise("http://192.0.2.10:11435/"),
            "http://192.0.2.10:11435"
        );
        assert_eq!(normalise(" https://a:1 "), "https://a:1");
    }

    /// Flags and the file are unioned and deduplicated, and the cluster name comes from the
    /// file. A node listed in both must appear once.
    #[test]
    fn flags_and_file_are_unioned() {
        let dir = std::env::temp_dir().join("atlas-seeds-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("cluster.toml");
        std::fs::write(
            &path,
            "[cluster]\nname = \"home\"\njoin = [\"http://192.0.2.2:11435\", \
             \"http://192.0.2.1:11435\"]\nadvertise = \"http://192.0.2.1:11435\"\n",
        )
        .expect("write");
        let (out, cluster) =
            from_flags_and_file(&["http://192.0.2.3:11435".into()], Some(&path)).expect("parse");
        assert_eq!(cluster.as_deref(), Some("home"));
        assert_eq!(
            out,
            vec![
                "http://192.0.2.1:11435",
                "http://192.0.2.2:11435",
                "http://192.0.2.3:11435"
            ]
        );
        std::fs::remove_file(&path).ok();
    }

    /// No file: the flags stand alone and no cluster name is invented.
    #[test]
    fn flags_alone_carry_no_cluster_name() {
        let (out, cluster) = from_flags_and_file(&["http://192.0.2.9:11435".into()], None)
            .expect("no file is not an error");
        assert_eq!(out, vec!["http://192.0.2.9:11435"]);
        assert!(cluster.is_none());
    }

    fn heard(pairs: &[(&str, &str)]) -> Heard {
        Heard {
            ours: pairs
                .iter()
                .map(|(id, e)| (id.to_string(), e.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    /// A seed stays when nothing is heard and nothing answers: the operator named it.
    #[test]
    fn a_seed_stays_when_silent() {
        let out = current(&["http://192.0.2.1:11435".into()], &Heard::default(), []);
        assert_eq!(out, vec!["http://192.0.2.1:11435"]);
    }

    /// A node heard announcing joins, in the one spelling, once.
    #[test]
    fn a_heard_node_joins_once() {
        let out = current(
            &["http://192.0.2.1:11435".into()],
            &heard(&[("a", "192.0.2.1:11435/"), ("b", "http://192.0.2.2:11435")]),
            ["http://192.0.2.2:11435"],
        );
        assert_eq!(
            out,
            vec!["http://192.0.2.1:11435", "http://192.0.2.2:11435"]
        );
    }

    /// Silent but answering stays; silent and not answering leaves.
    #[test]
    fn a_silent_node_leaves_only_when_it_stops_answering() {
        let out = current(&[], &Heard::default(), ["http://192.0.2.2:11435"]);
        assert_eq!(out, vec!["http://192.0.2.2:11435"]);
        assert!(current(&[], &Heard::default(), []).is_empty());
    }
}
