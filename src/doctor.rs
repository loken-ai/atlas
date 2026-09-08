//! Inconsistencies a cluster swallows in silence.
//!
//! Every rule is a pure function of a `ClusterSnapshot`, so it is tested on snapshots built
//! by hand and needs no cluster to run. The exit status is the number of rules that fired,
//! which is the convention `preflight.sh` uses.

use std::collections::{HashMap, HashSet};

use crate::model::{ClusterSnapshot, Health, Node};

/// One thing that is wrong, and how it was seen.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub rule: &'static str,
    pub detail: String,
}

fn f(rule: &'static str, detail: impl Into<String>) -> Finding {
    Finding {
        rule,
        detail: detail.into(),
    }
}

/// What a rule means, in a sentence. The slug names the rule in metrics and exit codes; a
/// person reading the report gets the sentence.
pub fn title(rule: &str) -> &'static str {
    match rule {
        "foreign-cluster" => "Another cluster is announcing itself on this network",
        "duplicate-node-id" => "Two nodes share one id, so each hides the other",
        "asymmetric-membership" => "The nodes do not all see each other",
        "no-peer-view" => "A node does not report who it sees",
        "catalogue-silent" => "A node publishes no catalogue",
        "catalogue-skew" => "The nodes do not serve the same models",
        "partial-energy" => "Energy is reported on part of the cluster only",
        "not-clustered" => "A node answers but is not in a cluster",
        "version-skew" => "The nodes run different versions",
        _ => "Unnamed finding",
    }
}

/// How a node is named in a finding: by the id it gives itself, or by its address until it
/// has answered.
fn name(n: &Node) -> &str {
    n.state
        .as_ref()
        .map(|s| s.node_id.as_str())
        .filter(|id| !id.is_empty())
        .unwrap_or(n.endpoint.as_str())
}

/// A foreign cluster on the same network.
///
/// No node can report this: discovery decodes an announcement, compares the cluster name and
/// drops a mismatch without a word - which is correct for a router and useless for an
/// operator. Only a passive listener sees both.
pub fn foreign_cluster(s: &ClusterSnapshot) -> Vec<Finding> {
    s.foreign
        .iter()
        .map(|(name, endpoint)| {
            f(
                "foreign-cluster",
                format!("{name} announcing at {endpoint}"),
            )
        })
        .collect()
}

/// Two nodes announcing the same id.
///
/// A real trap rather than a theoretical one: a node filters its own announcement ON THE ID,
/// so a duplicate makes each of the pair invisible to the other while both look healthy to
/// anyone else.
pub fn duplicate_node_id(s: &ClusterSnapshot) -> Vec<Finding> {
    let mut seen: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in &s.nodes {
        if let Some(state) = &n.state {
            if !state.node_id.is_empty() {
                seen.entry(&state.node_id).or_default().push(&n.endpoint);
            }
        }
    }
    seen.into_iter()
        .filter(|(_, eps)| eps.len() > 1)
        .map(|(id, eps)| f("duplicate-node-id", format!("{id} at {}", eps.join(", "))))
        .collect()
}

/// A node every observer can reach that cannot reach its neighbour.
///
/// The pair of endpoints is what makes this visible: `/api/cluster/state` says what a node is,
/// `/api/cluster/peers` says who it sees. Without the second, a partition where each half
/// answers the observer looks like a healthy cluster.
pub fn asymmetric_membership(s: &ClusterSnapshot) -> Vec<Finding> {
    let live: HashSet<&str> = s
        .nodes
        .iter()
        .filter(|n| n.health == Health::Online)
        .filter_map(|n| n.state.as_ref())
        .map(|st| st.node_id.as_str())
        .filter(|id| !id.is_empty())
        .collect();
    let mut out = Vec::new();
    for n in &s.nodes {
        let Some(state) = &n.state else { continue };
        if n.peers.is_empty() {
            continue; // an older build, which is a different finding
        }
        let sees: HashSet<&str> = n.peers.iter().map(|p| p.node_id.as_str()).collect();
        for missing in live
            .iter()
            .filter(|id| **id != state.node_id && !sees.contains(*id))
        {
            out.push(f(
                "asymmetric-membership",
                format!("{} does not see {missing}", state.node_id),
            ));
        }
    }
    out
}

/// A build that cannot answer the question above.
pub fn no_peer_endpoint(s: &ClusterSnapshot) -> Vec<Finding> {
    s.nodes
        .iter()
        .filter(|n| n.health == Health::Online && n.peers.is_empty() && n.state.is_some())
        .map(|n| {
            f(
                "no-peer-view",
                format!("{} publishes no peer view", n.endpoint),
            )
        })
        .collect()
}

/// Nodes that do not agree on what they can serve.
pub fn catalogue_skew(s: &ClusterSnapshot) -> Vec<Finding> {
    let mut said: Vec<(&str, usize)> = Vec::new();
    let mut silent = Vec::new();
    for n in &s.nodes {
        match n.state.as_ref().and_then(|st| st.serves.as_ref()) {
            // Silence is not an empty catalogue, and is reported as its own thing.
            None => silent.push(name(n)),
            Some(c) => said.push((name(n), c.len())),
        }
    }
    let mut out: Vec<Finding> = silent
        .into_iter()
        .map(|e| f("catalogue-silent", format!("{e} publishes no catalogue")))
        .collect();
    if said.len() > 1 {
        let (min, max) = (
            said.iter().min_by_key(|(_, n)| *n).unwrap(),
            said.iter().max_by_key(|(_, n)| *n).unwrap(),
        );
        if min.1 != max.1 {
            out.push(f(
                "catalogue-skew",
                format!(
                    "{} serves {} models, {} serves {}",
                    min.0, min.1, max.0, max.1
                ),
            ));
        }
    }
    out
}

/// Energy off on part of the cluster, which makes every total partial.
pub fn partial_energy(s: &ClusterSnapshot) -> Vec<Finding> {
    let off: Vec<&str> = s
        .nodes
        .iter()
        .filter(|n| n.health == Health::Online && n.energy_j.is_none())
        .map(|n| n.endpoint.as_str())
        .collect();
    if off.is_empty() || off.len() == s.nodes.len() {
        return vec![];
    }
    vec![f(
        "partial-energy",
        format!(
            "reporting disabled on {}, so any cluster total is partial",
            off.join(", ")
        ),
    )]
}

/// A node that is up but not in a cluster: `/health` answers, `/api/cluster/state` does not.
pub fn standing_but_alone(s: &ClusterSnapshot) -> Vec<Finding> {
    s.nodes
        .iter()
        .filter(|n| n.health != Health::Offline && n.state.is_none() && n.version.is_some())
        .map(|n| {
            f(
                "not-clustered",
                format!("{} answers but has no [cluster] block", n.endpoint),
            )
        })
        .collect()
}

/// Versions that differ across the cluster.
pub fn version_skew(s: &ClusterSnapshot) -> Vec<Finding> {
    let versions: HashSet<&str> = s
        .nodes
        .iter()
        .filter_map(|n| n.version.as_deref())
        .collect();
    if versions.len() <= 1 {
        return vec![];
    }
    let mut v: Vec<&str> = versions.into_iter().collect();
    v.sort_unstable();
    vec![f("version-skew", v.join(", "))]
}

/// Every rule, in the order a reader should meet them.
pub fn all(s: &ClusterSnapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for rule in [
        foreign_cluster,
        duplicate_node_id,
        asymmetric_membership,
        no_peer_endpoint,
        catalogue_skew,
        partial_energy,
        standing_but_alone,
        version_skew,
    ] {
        out.extend(rule(s));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Node, NodeState, PeerView};

    fn node(endpoint: &str, id: &str) -> Node {
        Node {
            endpoint: endpoint.into(),
            health: Health::Online,
            rtt_ms: Some(1.0),
            version: Some("0.1.0".into()),
            uptime_s: Some(10.0),
            // A catalogue is published, or `catalogue-silent` fires and no fixture is healthy.
            state: Some(NodeState {
                node_id: id.into(),
                serves: Some(vec!["qwen3:1.7b".into()]),
                ..Default::default()
            }),
            devices: vec![],
            placements: vec![],
            peers: vec![],
            energy_j: Some(1.0),
            errors: vec![],
        }
    }

    fn seeing(mut n: Node, ids: &[&str]) -> Node {
        n.peers = ids
            .iter()
            .map(|id| PeerView {
                node_id: (*id).into(),
                endpoint: None,
                alive: true,
                phi: None,
                rtt_ms: None,
                is_self: false,
                state: NodeState::default(),
            })
            .collect();
        n
    }

    #[test]
    fn a_partition_where_both_halves_answer_the_observer_is_found() {
        let a = seeing(node("http://a", "a"), &["a"]);
        let b = seeing(node("http://b", "b"), &["b"]);
        let s = ClusterSnapshot {
            nodes: vec![a, b],
            ..Default::default()
        };
        let found = asymmetric_membership(&s);
        assert_eq!(
            found.len(),
            2,
            "each half fails to see the other: {found:?}"
        );
    }

    /// The negative control: the rule must stay quiet on a cluster that is fine, or it says
    /// nothing when it fires.
    #[test]
    fn a_cluster_that_sees_itself_raises_nothing() {
        let a = seeing(node("http://a", "a"), &["a", "b"]);
        let b = seeing(node("http://b", "b"), &["a", "b"]);
        let s = ClusterSnapshot {
            nodes: vec![a, b],
            ..Default::default()
        };
        assert!(asymmetric_membership(&s).is_empty());
        assert!(all(&s).is_empty(), "{:?}", all(&s));
    }

    #[test]
    fn two_nodes_with_one_id_are_reported() {
        let s = ClusterSnapshot {
            nodes: vec![node("http://a", "same"), node("http://b", "same")],
            ..Default::default()
        };
        assert_eq!(duplicate_node_id(&s).len(), 1);
    }

    /// Silence about a catalogue is its own finding, not a skew against an empty one.
    #[test]
    fn a_silent_catalogue_is_not_a_skew() {
        let a = node("http://a", "a");
        let mut b = node("http://b", "b");
        b.state.as_mut().unwrap().serves = None;
        let s = ClusterSnapshot {
            nodes: vec![a, b],
            ..Default::default()
        };
        let found = catalogue_skew(&s);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].rule, "catalogue-silent");
    }

    #[test]
    fn energy_off_on_part_of_the_cluster_makes_the_total_partial() {
        let mut a = node("http://a", "a");
        a.energy_j = None;
        let s = ClusterSnapshot {
            nodes: vec![a, node("http://b", "b")],
            ..Default::default()
        };
        assert_eq!(partial_energy(&s).len(), 1);
    }

    /// All energy off everywhere is a choice, not an inconsistency.
    #[test]
    fn energy_off_everywhere_is_not_a_finding() {
        let mut a = node("http://a", "a");
        let mut b = node("http://b", "b");
        a.energy_j = None;
        b.energy_j = None;
        let s = ClusterSnapshot {
            nodes: vec![a, b],
            ..Default::default()
        };
        assert!(partial_energy(&s).is_empty());
    }

    #[test]
    fn a_foreign_announcement_is_reported_since_no_node_can() {
        let s = ClusterSnapshot {
            foreign: [("other".to_string(), "http://x:11435".to_string())].into(),
            ..Default::default()
        };
        assert_eq!(foreign_cluster(&s).len(), 1);
    }
}
