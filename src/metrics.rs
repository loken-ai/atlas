//! OpenMetrics for the facts no single node holds.
//!
//! A node can export what it knows about itself, and loken does behind its `metrics` feature.
//! What is missing there is everything that only exists between nodes: the round trips as the
//! observer measures them, whether membership is symmetric, whether the versions match, and
//! what `doctor` concluded. Re-exporting a node's own counters would be a second copy of a
//! number that already has a home.
//!
//! Encoded by hand. It is text, and the alternative is a dependency for `format!`.

use crate::doctor::Finding;
use crate::model::{ClusterSnapshot, Health};

fn label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn family(out: &mut String, name: &str, kind: &str, help: &str, samples: &[(String, f64)]) {
    if samples.is_empty() {
        return;
    }
    out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} {kind}\n"));
    for (labels, value) in samples {
        if labels.is_empty() {
            out.push_str(&format!("{name} {value}\n"));
        } else {
            out.push_str(&format!("{name}{{{labels}}} {value}\n"));
        }
    }
}

/// Every series is labelled with the cluster AND the node: two clusters may share a network by
/// design, so a node id alone is ambiguous.
pub fn encode(snapshot: &ClusterSnapshot, findings: &[Finding]) -> String {
    let cluster = snapshot.cluster.as_deref().unwrap_or("unknown");
    let l = |node: &str| format!("cluster=\"{}\",node=\"{}\"", label(cluster), label(node));
    let mut out = String::with_capacity(1024);

    let up: Vec<(String, f64)> = snapshot
        .nodes
        .iter()
        .map(|n| (l(&n.endpoint), f64::from(n.health == Health::Online)))
        .collect();
    family(
        &mut out,
        "atlas_node_up",
        "gauge",
        "1 when the observer reached the node.",
        &up,
    );

    // The observer's own view of the fabric, which is the thing no node measures.
    let rtt: Vec<(String, f64)> = snapshot
        .nodes
        .iter()
        .filter_map(|n| n.rtt_ms.map(|v| (l(&n.endpoint), v)))
        .collect();
    family(
        &mut out,
        "atlas_node_rtt_ms",
        "gauge",
        "Round trip on the request already being made, as the observer measures it.",
        &rtt,
    );

    let busy: Vec<(String, f64)> = snapshot
        .nodes
        .iter()
        .filter_map(|n| {
            n.state
                .as_ref()
                .map(|s| (l(&n.endpoint), f64::from(s.busy)))
        })
        .collect();
    family(
        &mut out,
        "atlas_node_busy",
        "gauge",
        "Generations the node reports in flight.",
        &busy,
    );

    // Absent rather than zero where a node does not report energy: a zero reads as a machine
    // drawing no power, and the cluster total built on it would be wrong without saying so.
    let joules: Vec<(String, f64)> = snapshot
        .nodes
        .iter()
        .filter_map(|n| n.energy_j.map(|v| (l(&n.endpoint), v)))
        .collect();
    family(
        &mut out,
        "atlas_node_energy_joules_total",
        "counter",
        "Energy the node attributes to requests, where it reports any.",
        &joules,
    );

    let (total, complete) = snapshot.energy_j();
    if complete {
        family(
            &mut out,
            "atlas_cluster_energy_joules_total",
            "counter",
            "Cluster energy. Emitted only when every node reports, so it is never a partial sum.",
            &[(format!("cluster=\"{}\"", label(cluster)), total)],
        );
    }

    // One series per rule, always present so a rule that stops firing reads as zero rather
    // than as a series that vanished - which a scraper cannot tell from a broken exporter.
    let mut counts: std::collections::BTreeMap<&str, f64> = [
        ("foreign-cluster", 0.0),
        ("duplicate-node-id", 0.0),
        ("asymmetric-membership", 0.0),
        ("no-peer-view", 0.0),
        ("catalogue-silent", 0.0),
        ("partial-energy", 0.0),
        ("not-clustered", 0.0),
        ("version-skew", 0.0),
    ]
    .into();
    for f in findings {
        *counts.entry(f.rule).or_insert(0.0) += 1.0;
    }
    let rules: Vec<(String, f64)> = counts
        .into_iter()
        .map(|(rule, n)| (format!("cluster=\"{}\",rule=\"{rule}\"", label(cluster)), n))
        .collect();
    family(
        &mut out,
        "atlas_doctor_findings",
        "gauge",
        "Inconsistencies, by rule.",
        &rules,
    );

    out.push_str("# EOF\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Node, NodeState};

    fn node(endpoint: &str, energy: Option<f64>) -> Node {
        Node {
            endpoint: endpoint.into(),
            health: Health::Online,
            rtt_ms: Some(2.5),
            version: Some("0.1.0".into()),
            uptime_s: Some(1.0),
            state: Some(NodeState {
                busy: 3,
                ..Default::default()
            }),
            devices: vec![],
            placements: vec![],
            peers: vec![],
            energy_j: energy,
            errors: vec![],
        }
    }

    /// A rule that stops firing must read as zero. A series that disappears looks exactly like
    /// an exporter that broke, and an alert on it cannot tell the two apart.
    #[test]
    fn every_rule_has_a_series_even_at_zero() {
        let snap = ClusterSnapshot {
            nodes: vec![node("http://a", None)],
            ..Default::default()
        };
        let text = encode(&snap, &[]);
        for rule in ["foreign-cluster", "duplicate-node-id", "version-skew"] {
            assert!(
                text.contains(&format!("rule=\"{rule}\"}} 0")),
                "{rule} missing from:\n{text}"
            );
        }
    }

    /// A cluster total is emitted only when every node contributed to it.
    #[test]
    fn a_partial_energy_total_is_not_emitted() {
        let partial = ClusterSnapshot {
            nodes: vec![node("http://a", Some(9.0)), node("http://b", None)],
            ..Default::default()
        };
        assert!(!encode(&partial, &[]).contains("atlas_cluster_energy_joules_total"));
        let whole = ClusterSnapshot {
            nodes: vec![node("http://a", Some(9.0)), node("http://b", Some(1.0))],
            ..Default::default()
        };
        assert!(encode(&whole, &[])
            .contains("atlas_cluster_energy_joules_total{cluster=\"unknown\"} 10"));
    }

    /// Two clusters can share a network by design, so a node label alone is ambiguous.
    #[test]
    fn every_series_carries_the_cluster_name() {
        let snap = ClusterSnapshot {
            cluster: Some("home".into()),
            nodes: vec![node("http://a", None)],
            ..Default::default()
        };
        let text = encode(&snap, &[]);
        for line in text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            assert!(line.contains("cluster=\"home\""), "unlabelled: {line}");
        }
    }
}
