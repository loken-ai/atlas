//! Watch a loken cluster from outside it.
//!
//! A node answers "what am I" and, since it publishes a peer view, "who do I see". Neither
//! answers "do the nodes agree", and no node can report a cluster it was built to ignore. That
//! is what an observer is for.
//!
//! The core produces a [`ClusterSnapshot`] and knows nothing about how it is drawn. The TUI and
//! the GUI render the same value, and [`doctor`] reasons over exactly what both show - which is
//! what keeps three views from becoming three models of the cluster.
//!
//! ## Where it is careful
//!
//! **atlas never announces itself.** A probe that joins the multicast group becomes a peer the
//! router can hand work to.
//!
//! **Poll rates are staged, and `/api/cluster/state` is the slow one.** The round trip of that
//! request is what the router uses to price a hand-over, so hammering it inflates the measured
//! latency of the node it describes and biases routing away from a peer that is fine. An
//! observer that changes what it observes is not observing.
//!
//! **`0.0` is unknown, not zero.** Every published rate uses zero to mean never measured; see
//! [`model::Rate`]. Printing `0 tok/s` states a number the server went out of its way not to
//! mean.
//!
//! **Two endpoints are never polled at once per node**, and the round trip is taken on the
//! request already being made rather than on a separate ping, which would measure a path the
//! router does not price.

#[cfg(feature = "gui")]
pub mod app;
pub mod collect;
pub mod discovery;
pub mod doctor;
#[cfg(feature = "gui")]
pub mod icons;
pub mod metrics;
pub mod model;
#[cfg(all(test, feature = "gui"))]
mod screenshots;
pub mod seeds;
pub mod tui;
#[cfg(feature = "gui")]
pub mod view;

pub use model::{
    ClusterSnapshot, Device, Health, Node, NodeState, PeerView, Placement, Rate, Segment,
};
