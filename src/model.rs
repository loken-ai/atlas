//! What one poll of a cluster produced.
//!
//! The wire types mirror what a loken node publishes on `/health`, `/api/cluster/state` and
//! `/api/cluster/peers`. Their optionality is the specification, not a convenience: a field
//! that is absent, null or zero means three different things here and displaying any of them
//! as another is the difference between a report and a guess.

use std::collections::BTreeMap;

use serde::Deserialize;

/// A rate a node publishes. `0.0` means NEVER MEASURED, not "zero tokens per second".
///
/// The server states this explicitly - when both nodes report zero every candidate ties at
/// infinity and the choice becomes arbitrary - so an observer that prints `0 tok/s` prints a
/// number the engine went out of its way not to mean.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate(pub f64);

impl Rate {
    pub fn measured(self) -> Option<f64> {
        (self.0 > 0.0).then_some(self.0)
    }
}

impl std::fmt::Display for Rate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.measured() {
            Some(v) => write!(f, "{v:.0}"),
            None => f.write_str("-"),
        }
    }
}

/// What a node says about itself.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NodeState {
    #[serde(default)]
    pub node_id: String,
    #[serde(default)]
    pub devices: Vec<String>,
    #[serde(default)]
    pub load: f32,
    #[serde(default)]
    pub busy: u32,
    /// Zero means the node never said - an older build - not a node that serves nothing.
    #[serde(default)]
    pub lanes: u32,
    #[serde(default)]
    pub models: Vec<String>,
    /// `None` and `Some(empty)` are different facts. None: the node did not say. Some(empty):
    /// it said, and the answer was nothing. Collapsing them makes a weightless node look like
    /// an unknown one.
    #[serde(default)]
    pub serves: Option<Vec<String>>,
    #[serde(default)]
    pub prefill_tok_per_s: f64,
    #[serde(default)]
    pub decode_tok_per_s: f64,
}

/// What a node says about a peer, from `/api/cluster/peers`.
#[derive(Debug, Clone, Deserialize)]
pub struct PeerView {
    pub node_id: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    pub alive: bool,
    #[serde(default)]
    pub phi: Option<f64>,
    #[serde(default)]
    pub rtt_ms: Option<f64>,
    #[serde(default)]
    pub is_self: bool,
    #[serde(default)]
    pub state: NodeState,
}

/// One compute device, as `/api/distributed/devices` describes it.
///
/// The live fields are `Option` because they come from NVML and only exist for CUDA cards.
/// `None` is not zero: a temperature of zero is a reading, and drawing one where there is no
/// sensor invents a measurement.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Device {
    #[serde(rename = "type", default)]
    pub device_type: String,
    #[serde(rename = "id", default)]
    pub device_id: usize,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub memory_bytes: u64,
    /// `Option`, because the server sends null for a device it cannot measure - the CPU row
    /// has no free-memory figure. `serde(default)` does not cover an explicit null, only an
    /// absent key, and taking null as zero would draw a full bar on a device nobody measured.
    #[serde(rename = "free_bytes", default)]
    pub available_memory_bytes: Option<u64>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub utilization_gpu_percent: Option<f32>,
    #[serde(default)]
    pub temperature_c: Option<f32>,
    #[serde(default)]
    pub power_watts: Option<f32>,
    #[serde(default)]
    pub power_limit_watts: Option<f32>,
}

impl Device {
    pub fn available(&self) -> bool {
        self.status == "available"
    }

    /// How much is in use, when both figures exist. `None` where the device reports no free
    /// memory: that is unmeasured, and subtracting from nothing would read as fully used.
    pub fn used_bytes(&self) -> Option<u64> {
        self.available_memory_bytes
            .map(|free| self.memory_bytes.saturating_sub(free))
    }

    /// Fraction of the device in use, or `None` where it reports no capacity or no free
    /// memory - a device nobody measured, not a device that is empty.
    pub fn used_fraction(&self) -> Option<f32> {
        match (self.memory_bytes, self.used_bytes()) {
            (0, _) | (_, None) => None,
            (total, Some(used)) => Some(used as f32 / total as f32),
        }
    }
}

/// Where one model's layers ended up.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Placement {
    #[serde(default)]
    pub model_id: String,
    /// What the node says of the entry: "loaded", or the render it runs.
    #[serde(default)]
    pub status: String,
    /// The device the node names for the entry, when it reports no layers.
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub total_layers: u32,
    #[serde(default)]
    pub segments: Vec<Segment>,
}

impl Placement {
    /// The line that names the entry: its layer count when it is placed, and what the
    /// node says of it when that is more than "loaded", which is how a render in
    /// progress describes itself (its phase and step follow the layer count).
    pub fn headline(&self) -> String {
        let told = !self.status.is_empty() && self.status != "loaded";
        match (self.total_layers, told, self.device.as_deref()) {
            (0, true, _) => format!("{} - {}", self.model_id, self.status),
            // A node that gave no layer count is not claiming zero of them.
            (0, false, Some(device)) => format!("{} - loaded on {}", self.model_id, device),
            (0, false, None) => format!("{} - loaded", self.model_id),
            (n, true, _) => format!("{} - {} layers - {}", self.model_id, n, self.status),
            (n, false, _) => format!("{} - {} layers", self.model_id, n),
        }
    }
}

/// A run of layers on one device. `last` is INCLUSIVE, which is where an off-by-one once lost
/// the final layer of every segment.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Segment {
    #[serde(default)]
    pub device_type: String,
    #[serde(default)]
    pub device_id: usize,
    #[serde(default)]
    pub first: u32,
    #[serde(default)]
    pub last: u32,
    #[serde(default)]
    pub memory_bytes: u64,
}

impl Segment {
    pub fn layers(&self) -> u32 {
        self.last.saturating_sub(self.first) + 1
    }
}

/// How a node looks to the observer, which is not how it looks to its peers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// Answered within the last two poll periods.
    Online,
    /// Answering, but not everything, or not recently enough.
    Degraded,
    /// Past the threshold with no answer.
    Offline,
    /// Announced or configured, never yet reached. Not the same as Offline: nothing has
    /// failed, the observer simply has not asked yet.
    Unknown,
}

/// One node as the observer found it.
#[derive(Debug, Clone)]
pub struct Node {
    pub endpoint: String,
    pub health: Health,
    /// Measured on the request already being made, never on a separate ping: a ping measures
    /// a path the router does not price.
    pub rtt_ms: Option<f64>,
    pub version: Option<String>,
    pub uptime_s: Option<f64>,
    pub state: Option<NodeState>,
    /// The cards this node can place work on, with what NVML says about them.
    pub devices: Vec<Device>,
    /// Where each resident model's layers sit.
    pub placements: Vec<Placement>,
    /// What this node believes about the others. Empty when the build predates
    /// `/api/cluster/peers`, which is why an asymmetric partition is invisible without it.
    pub peers: Vec<PeerView>,
    /// `None` when energy reporting is off on that node, so any cluster total built from
    /// these is partial and has to say so.
    pub energy_j: Option<f64>,
    pub errors: Vec<String>,
}

/// One poll of the whole cluster.
#[derive(Debug, Clone, Default)]
pub struct ClusterSnapshot {
    pub cluster: Option<String>,
    pub nodes: Vec<Node>,
    /// Announcements heard from a cluster that is not this one. No node can report these:
    /// discovery drops a name mismatch without a word, so only a passive listener sees them.
    pub foreign: BTreeMap<String, String>,
}

impl ClusterSnapshot {
    pub fn online(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.health == Health::Online)
            .count()
    }

    /// Total joules, and whether every node contributed. A sum over nodes that do not all
    /// report is a partial sum, and printing it bare invites it to be read as the total.
    pub fn energy_j(&self) -> (f64, bool) {
        let reporting: Vec<f64> = self.nodes.iter().filter_map(|n| n.energy_j).collect();
        (reporting.iter().sum(), reporting.len() == self.nodes.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The distinction the server draws, kept here: a rate of zero was never measured.
    #[test]
    fn a_zero_rate_is_unknown_and_prints_as_such() {
        assert_eq!(Rate(0.0).measured(), None);
        assert_eq!(Rate(0.0).to_string(), "-");
        assert_eq!(Rate(412.0).measured(), Some(412.0));
        assert_eq!(Rate(412.0).to_string(), "412");
    }

    /// `serves: null` is a build that did not say; `serves: []` is a node that serves nothing.
    #[test]
    fn an_absent_catalogue_is_not_an_empty_one() {
        let silent: NodeState = serde_json::from_str("{}").unwrap();
        let empty: NodeState = serde_json::from_str(r#"{"serves":[]}"#).unwrap();
        assert!(
            silent.serves.is_none(),
            "no catalogue field means no answer"
        );
        assert_eq!(
            empty.serves.as_deref(),
            Some(&[][..]),
            "an answer of nothing"
        );
    }

    /// A total over nodes that do not all report energy is partial, and says so.
    #[test]
    fn a_partial_energy_total_is_flagged() {
        let node = |e| Node {
            endpoint: "x".into(),
            health: Health::Online,
            rtt_ms: None,
            version: None,
            uptime_s: None,
            state: None,
            devices: vec![],
            placements: vec![],
            peers: vec![],
            energy_j: e,
            errors: vec![],
        };
        let snap = ClusterSnapshot {
            nodes: vec![node(Some(10.0)), node(None)],
            ..Default::default()
        };
        assert_eq!(snap.energy_j(), (10.0, false));
        let all = ClusterSnapshot {
            nodes: vec![node(Some(10.0)), node(Some(5.0))],
            ..Default::default()
        };
        assert_eq!(all.energy_j(), (15.0, true));
    }
}
