//! Polling the nodes.
//!
//! Cadences are staged and `/api/cluster/state` is the slow one, because the round trip of
//! that request is what the router uses to price a hand-over: a probe that hammers it inflates
//! the measured latency of the node it describes and biases routing away from a peer that is
//! fine. An observer that changes what it observes is not observing.
//!
//! `/api/stage_perf` is never polled: it is a global toggle that resets counters.
//! `/api/distributed/stats` and `/api/distributed/recommend` are never polled either; each
//! builds and initialises a fresh engine per call.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::model::{
    ClusterSnapshot, Device, Health, LayerTime, Node, PeerView, Placement, Segment,
};

/// How long a node may go unanswered before it stops being Online.
const DEGRADED_AFTER: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
struct HealthWire {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    uptime_seconds: Option<f64>,
}

#[derive(Deserialize)]
struct DevicesWire {
    #[serde(default)]
    devices: Vec<Device>,
    /// Absent when energy reporting is off on that node, which is not the same as zero.
    #[serde(default)]
    energy: Option<EnergyWire>,
}

#[derive(Deserialize)]
struct EnergyWire {
    #[serde(default)]
    total_j: f64,
}

#[derive(Deserialize)]
struct LoadedWire {
    #[serde(default)]
    models: Vec<LoadedModel>,
}

#[derive(Deserialize, Default)]
struct LayerPerfWire {
    #[serde(default)]
    measuring: bool,
    #[serde(default)]
    layers: Vec<LayerRowWire>,
}

#[derive(Deserialize)]
struct LayerRowWire {
    #[serde(default)]
    layer_idx: u32,
    #[serde(default)]
    device_type: String,
    #[serde(default)]
    model_name: String,
    #[serde(default)]
    avg_ms_per_token: f64,
    #[serde(default)]
    token_count: u64,
}

#[derive(Deserialize)]
struct LoadedModel {
    #[serde(default, alias = "model")]
    model_id: String,
    /// "loaded", or what a render says of itself, such as "rendering sound".
    #[serde(default)]
    status: String,
    /// The device the node names for the entry, for a model that reports no layers.
    #[serde(default)]
    device: Option<String>,
    #[serde(default, alias = "num_layers")]
    total_layers: u32,
    #[serde(default)]
    layer_distribution: Vec<LayerWire>,
}

#[derive(Deserialize)]
struct LayerWire {
    #[serde(default)]
    device_type: String,
    #[serde(default)]
    device_id: usize,
    /// `layer_start` and `layer_end`, the end INCLUSIVE, as the server writes them; the
    /// `[first, last]` pair of an older build is read the same way.
    #[serde(default)]
    layer_start: u32,
    #[serde(default)]
    layer_end: u32,
    #[serde(default)]
    layer_range: Option<(u32, u32)>,
    #[serde(default)]
    memory_bytes: u64,
}

impl LayerWire {
    fn range(&self) -> (u32, u32) {
        self.layer_range
            .unwrap_or((self.layer_start, self.layer_end))
    }
}

#[derive(Deserialize)]
struct PeersWire {
    #[serde(default)]
    peers: Vec<PeerView>,
}

/// Which endpoint answered, and how long the answer took.
struct Timed<T> {
    value: Option<T>,
    elapsed: Duration,
    error: Option<String>,
}

async fn get<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
) -> Timed<T> {
    // The round trip is taken on the request already being made, before deserialisation - a
    // separate ping would measure a path the router does not price, and counting parse time
    // would blame the network for the size of a payload.
    let started = Instant::now();
    let sent = client.get(url).timeout(timeout).send().await;
    let elapsed = started.elapsed();
    match sent {
        Ok(response) => match response.json::<T>().await {
            Ok(value) => Timed {
                value: Some(value),
                elapsed,
                error: None,
            },
            Err(e) => Timed {
                value: None,
                elapsed,
                error: Some(format!("{url}: {e}")),
            },
        },
        Err(e) => Timed {
            value: None,
            elapsed,
            error: Some(format!("{url}: {e}")),
        },
    }
}

/// Poll one node. Endpoints are asked in sequence rather than at once, so a node is never
/// given four simultaneous requests by something that claims to be watching it.
pub async fn poll_node(client: &reqwest::Client, endpoint: &str) -> Node {
    let base = endpoint.trim_end_matches('/');
    let mut errors = Vec::new();
    let short = Duration::from_secs(2);

    let health: Timed<HealthWire> = get(client, &format!("{base}/health"), short).await;
    let rtt_ms = health
        .value
        .is_some()
        .then_some(health.elapsed.as_secs_f64() * 1000.0);
    if let Some(e) = health.error {
        errors.push(e);
    }
    let (version, uptime_s) = match health.value {
        Some(h) => (h.version, h.uptime_seconds),
        None => (None, None),
    };

    // Nothing else is worth asking of a node that did not answer the cheapest question.
    if version.is_none() {
        return Node {
            endpoint: base.to_string(),
            health: Health::Offline,
            rtt_ms,
            version,
            uptime_s,
            state: None,
            devices: vec![],
            placements: vec![],
            peers: vec![],
            energy_j: None,
            errors,
            measuring: false,
            layer_times: vec![],
        };
    }

    let state: Timed<crate::model::NodeState> =
        get(client, &format!("{base}/api/cluster/state"), short).await;
    if let Some(e) = state.error {
        errors.push(e);
    }
    let peers: Timed<PeersWire> = get(client, &format!("{base}/api/cluster/peers"), short).await;
    // A build without the endpoint is not an error to report; it is a finding doctor makes.
    let peers = peers.value.map(|p| p.peers).unwrap_or_default();

    let devices: Timed<DevicesWire> =
        get(client, &format!("{base}/api/distributed/devices"), short).await;
    if let Some(e) = devices.error {
        errors.push(e);
    }
    let (device_list, energy_j) = match devices.value {
        Some(d) => (d.devices, d.energy.map(|e| e.total_j)),
        None => (vec![], None),
    };

    // A node predating the endpoint answers an error here; that is a missing panel, not a
    // node in poor health, so it is read and not counted.
    let perf: Timed<LayerPerfWire> = get(client, &format!("{base}/api/layer_perf"), short).await;
    let (measuring, layer_times) = perf
        .value
        .map(|p| {
            let times = p
                .layers
                .into_iter()
                .map(|l| LayerTime {
                    model: l.model_name,
                    layer: l.layer_idx,
                    device: l.device_type.replace("GPU #", "CUDA"),
                    ms_per_token: l.avg_ms_per_token,
                    tokens: l.token_count,
                })
                .collect();
            (p.measuring, times)
        })
        .unwrap_or_default();

    let loaded: Timed<LoadedWire> = get(client, &format!("{base}/api/models/loaded"), short).await;
    let placements = loaded
        .value
        .map(|l| {
            l.models
                .into_iter()
                .map(|m| Placement {
                    model_id: m.model_id,
                    status: m.status,
                    device: m.device,
                    total_layers: m.total_layers,
                    segments: m
                        .layer_distribution
                        .into_iter()
                        .map(|d| {
                            let (first, last) = d.range();
                            Segment {
                                device_type: d.device_type,
                                device_id: d.device_id,
                                first,
                                last,
                                memory_bytes: d.memory_bytes,
                            }
                        })
                        .collect(),
                })
                .collect()
        })
        .unwrap_or_default();

    // Answering, but not everything, is Degraded rather than Online: a node whose device
    // endpoint fails is not a node in good health, and calling it Online hides that.
    let health = if errors.is_empty() && health.elapsed < DEGRADED_AFTER {
        Health::Online
    } else {
        Health::Degraded
    };

    Node {
        endpoint: base.to_string(),
        health,
        rtt_ms,
        version,
        uptime_s,
        state: state.value,
        devices: device_list,
        placements,
        peers,
        energy_j,
        errors,
        measuring,
        layer_times,
    }
}

/// Switch layer-time measurement on or off on every node named. Measurement costs the
/// nodes a device synchronisation per stage, so it is switched on only while someone is
/// looking and off again when they leave.
pub async fn set_measuring(endpoints: &[String], on: bool) {
    let client = reqwest::Client::new();
    let flag = if on { "1" } else { "0" };
    for endpoint in endpoints {
        let url = format!(
            "{}/api/layer_perf?enable={flag}",
            endpoint.trim_end_matches('/')
        );
        let _ = client.get(url).timeout(Duration::from_secs(2)).send().await;
    }
}

/// Poll every node named, in parallel across nodes and in sequence within each one.
pub async fn poll(endpoints: &[String], cluster: Option<String>) -> ClusterSnapshot {
    let client = reqwest::Client::builder()
        .user_agent(concat!("atlas/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default();
    let mut nodes: Vec<Node> = Vec::with_capacity(endpoints.len());
    for endpoint in endpoints {
        let node = poll_node(&client, endpoint).await;
        // The same daemon reached at two addresses - localhost and the one it advertises -
        // is one node. Its id says so; the first address that reached it is kept.
        let dup = node.state.as_ref().is_some_and(|s| {
            nodes
                .iter()
                .any(|n| n.state.as_ref().is_some_and(|t| t.node_id == s.node_id))
        });
        if !dup {
            nodes.push(node);
        }
    }
    ClusterSnapshot {
        cluster,
        nodes,
        foreign: BTreeMap::new().into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server names its device fields `type`, `id` and `free_bytes`. Renaming them here is
    /// the kind of hand-copy that drifts, so the mapping is pinned against a real payload.
    #[test]
    fn a_device_parses_from_the_server_s_own_names() {
        let wire = r#"{"devices":[{"id":1,"type":"CUDA","name":"RTX 5060 Ti",
            "memory_bytes":17094934528,"free_bytes":16277110784,"status":"available",
            "utilization_gpu_percent":4.0,"temperature_c":38.0,"power_watts":12.0,
            "power_limit_watts":180.0}],"energy":{"total_j":3290.7}}"#;
        let parsed: DevicesWire = serde_json::from_str(wire).expect("parses");
        let d = &parsed.devices[0];
        assert_eq!(d.device_id, 1);
        assert_eq!(d.device_type, "CUDA");
        assert!(d.available());
        assert_eq!(d.used_bytes(), Some(17094934528 - 16277110784));
        assert_eq!(parsed.energy.map(|e| e.total_j), Some(3290.7));
    }

    /// The payload a running node actually returns, kept verbatim. A hand-written fixture
    /// tests the parser against what its author imagined; this one tests it against the
    /// server.
    #[test]
    fn the_real_devices_payload_parses() {
        let wire = include_str!("../tests/devices.json");
        let parsed: DevicesWire = match serde_json::from_str(wire) {
            Ok(v) => v,
            Err(e) => panic!("{e}"),
        };
        assert_eq!(parsed.devices.len(), 3, "two cards and the host");
        assert!(parsed.energy.is_some());
    }

    /// Energy absent is not energy zero: the node has reporting switched off.
    #[test]
    fn a_node_without_energy_reporting_yields_none() {
        let parsed: DevicesWire = serde_json::from_str(r#"{"devices":[]}"#).expect("parses");
        assert!(parsed.energy.is_none());
    }

    /// The payload as the server writes it today: `model`, `num_layers`, and a run of
    /// `layer_start`..`layer_end`, the end inclusive.
    #[test]
    fn the_server_s_loaded_payload_names_its_model_and_layers() {
        let wire = r#"{"models":[{"model":"qwen3:0.6b","status":"loaded","device":"cuda",
            "size_bytes":522640096,"num_layers":28,"layer_distribution":[{"device_type":"CUDA",
            "device_id":0,"layer_start":0,"layer_end":27,"memory_bytes":522640096}]}]}"#;
        let parsed: LoadedWire = serde_json::from_str(wire).expect("parses");
        let m = &parsed.models[0];
        assert_eq!(m.model_id, "qwen3:0.6b");
        assert_eq!(m.total_layers, 28);
        assert_eq!(m.layer_distribution[0].range(), (0, 27));
    }

    /// `layer_range` is inclusive at both ends, and the segment must keep it that way.
    #[test]
    fn a_layer_range_keeps_its_last_layer() {
        let wire = r#"{"models":[{"model_id":"qwen3:8b","total_layers":36,
            "layer_distribution":[{"device_type":"CUDA","device_id":1,"layer_range":[24,35],
            "memory_bytes":1610612736}]}]}"#;
        let parsed: LoadedWire = serde_json::from_str(wire).expect("parses");
        let d = &parsed.models[0].layer_distribution[0];
        let segment = Segment {
            device_type: d.device_type.clone(),
            device_id: d.device_id,
            first: d.range().0,
            last: d.range().1,
            memory_bytes: d.memory_bytes,
        };
        assert_eq!(segment.layers(), 12);
    }
}
