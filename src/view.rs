//! Drawing a `ClusterSnapshot`.
//!
//! Behind the `gui` feature, because the core must build without a graphics stack. What is
//! drawn here is the whole cluster, not one machine: the view this replaces lived inside the
//! server's own GUI and could only ever show the host it ran on.
//!
//! Two rules run through it. Every icon is an SVG from [`crate::icons`], never a character:
//! a glyph resolves in whatever font the system finds and lands as a different weight, a
//! colour emoji, or a missing-glyph box. And an absent measurement is drawn as absent - a
//! temperature of zero is a reading, and a rate of zero means never measured.

use eframe::egui;
use egui::{Color32, RichText};

use crate::icons::Icon;
use crate::model::{ClusterSnapshot, Device, Health, Node};

pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xed, 0xf3);
pub const MUTED: Color32 = Color32::from_rgb(0x8b, 0x94, 0x9e);
pub const OK: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x50);
pub const WARN: Color32 = Color32::from_rgb(0xd2, 0x99, 0x22);
pub const BAD: Color32 = Color32::from_rgb(0xf8, 0x51, 0x49);
pub const LINE: Color32 = Color32::from_rgb(0x30, 0x36, 0x3d);

/// Nothing measured reads as `-`, everywhere, so a blank is never mistaken for a zero.
fn or_dash(value: Option<f32>, unit: &str) -> String {
    value.map_or_else(|| "-".to_string(), |v| format!("{v:.0}{unit}"))
}

fn health_colour(h: Health) -> Color32 {
    match h {
        Health::Online => OK,
        Health::Degraded => WARN,
        Health::Offline => BAD,
        Health::Unknown => MUTED,
    }
}

fn health_word(h: Health) -> &'static str {
    match h {
        Health::Online => "online",
        Health::Degraded => "degraded",
        Health::Offline => "offline",
        Health::Unknown => "not yet reached",
    }
}

fn section(ui: &mut egui::Ui, icon: Icon, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .stroke(egui::Stroke::new(1.0, LINE))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                icon.show(ui, 14.0, MUTED);
                ui.label(RichText::new(title).size(14.0).color(TEXT).strong());
            });
            ui.add_space(6.0);
            body(ui);
        });
}

/// A memory bar. Thresholds are the ones a reader already knows from the tool this replaces:
/// comfortable up to 70 percent, tight to 90, then red.
fn memory_bar(ui: &mut egui::Ui, device: &Device) {
    let Some(fraction) = device.used_fraction() else {
        ui.label(
            RichText::new("no capacity reported")
                .size(10.0)
                .color(MUTED)
                .italics(),
        );
        return;
    };
    let colour = match fraction {
        f if f > 0.90 => BAD,
        f if f > 0.70 => WARN,
        _ => OK,
    };
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 14.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, LINE);
    let mut filled = rect;
    filled.set_width(rect.width() * fraction);
    ui.painter().rect_filled(filled, 2.0, colour);
    const GB: f32 = (1024 * 1024 * 1024) as f32;
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!(
            "{:.1}/{:.1} GB ({:.0}%)",
            device.used_bytes().unwrap_or(0) as f32 / GB,
            device.memory_bytes as f32 / GB,
            fraction * 100.0
        ),
        egui::FontId::proportional(10.0),
        TEXT,
    );
}

fn device_row(ui: &mut egui::Ui, device: &Device) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&device.device_type)
                .size(9.0)
                .color(if device.available() { OK } else { MUTED }),
        );
        ui.label(
            RichText::new(format!("#{}", device.device_id))
                .size(10.0)
                .color(MUTED),
        );
        ui.label(RichText::new(&device.name).size(11.0).color(TEXT));
    });
    memory_bar(ui, device);
    ui.horizontal(|ui| {
        Icon::Chart.show(ui, 10.0, MUTED);
        ui.label(
            RichText::new(or_dash(device.utilization_gpu_percent, "%"))
                .size(10.0)
                .color(MUTED),
        );
        Icon::Thermometer.show(ui, 10.0, MUTED);
        ui.label(
            RichText::new(or_dash(device.temperature_c, "C"))
                .size(10.0)
                .color(MUTED),
        );
        Icon::Bolt.show(ui, 10.0, MUTED);
        ui.label(
            RichText::new(match (device.power_watts, device.power_limit_watts) {
                (Some(w), Some(cap)) => format!("{w:.0}/{cap:.0}W"),
                (Some(w), None) => format!("{w:.0}W"),
                _ => "-".to_string(),
            })
            .size(10.0)
            .color(MUTED),
        );
    });
}

fn node_card(ui: &mut egui::Ui, node: &Node) {
    section(ui, Icon::Server, &node.endpoint, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(health_word(node.health))
                    .size(10.0)
                    .color(health_colour(node.health)),
            );
            if let Some(rtt) = node.rtt_ms {
                ui.label(
                    RichText::new(format!("{rtt:.0} ms"))
                        .size(10.0)
                        .color(MUTED),
                );
            }
            if let Some(v) = &node.version {
                ui.label(RichText::new(v).size(10.0).color(MUTED));
            }
        });
        if let Some(state) = &node.state {
            ui.label(
                RichText::new(format!("{} in flight, {} lane(s)", state.busy, state.lanes))
                    .size(10.0)
                    .color(MUTED),
            );
        }
        ui.add_space(4.0);
        if node.devices.is_empty() {
            // Honest emptiness reads as breakage, so it is named rather than left blank.
            ui.label(
                RichText::new("no devices reported")
                    .size(10.0)
                    .color(MUTED)
                    .italics(),
            );
        }
        for device in &node.devices {
            device_row(ui, device);
            ui.add_space(4.0);
        }
        // Why a node is down is the whole reason to look at it. The terminal view names the
        // failure; leaving it out here turns an offline card into a card with nothing in it.
        for error in &node.errors {
            ui.horizontal(|ui| {
                Icon::Warning.show(ui, 10.0, BAD);
                ui.label(RichText::new(error).size(10.0).color(BAD));
            });
        }
        for placement in &node.placements {
            ui.label(
                RichText::new(format!(
                    "{} - {} layers",
                    placement.model_id, placement.total_layers
                ))
                .size(11.0)
                .color(TEXT),
            );
            for segment in &placement.segments {
                ui.label(
                    RichText::new(format!(
                        "    {}:{}  L{}-{}  ({} layers)",
                        segment.device_type,
                        segment.device_id,
                        segment.first,
                        segment.last,
                        segment.layers()
                    ))
                    .size(10.0)
                    .color(MUTED),
                );
            }
        }
    });
}

/// What doctor found, or that it found nothing.
///
/// The terminal view has carried this from the start and the drawn one did not, which left the
/// window showing the cluster without the one thing only an observer can say about it.
pub fn findings(ui: &mut egui::Ui, findings: &[crate::doctor::Finding]) {
    if findings.is_empty() {
        ui.horizontal(|ui| {
            Icon::Check.show(ui, 12.0, OK);
            ui.label(RichText::new("Nothing to report").size(11.0).color(OK));
        });
        return;
    }
    for finding in findings {
        ui.horizontal(|ui| {
            Icon::Warning.show(ui, 12.0, WARN);
            ui.label(
                RichText::new(finding.rule)
                    .size(11.0)
                    .color(WARN)
                    .monospace(),
            );
            ui.label(RichText::new(&finding.detail).size(11.0).color(TEXT));
        });
    }
}

/// The whole cluster, one card per node.
pub fn cluster(ui: &mut egui::Ui, snapshot: &ClusterSnapshot) {
    ui.horizontal(|ui| {
        Icon::Globe.show(ui, 18.0, TEXT);
        ui.label(
            RichText::new(snapshot.cluster.as_deref().unwrap_or("cluster"))
                .size(18.0)
                .color(TEXT)
                .strong(),
        );
        ui.label(
            RichText::new(format!(
                "{} of {} online",
                snapshot.online(),
                snapshot.nodes.len()
            ))
            .size(11.0)
            .color(MUTED),
        );
        let (joules, complete) = snapshot.energy_j();
        if joules > 0.0 {
            Icon::Bolt.show(ui, 12.0, MUTED);
            // A sum over nodes that do not all report is labelled, never printed bare.
            ui.label(
                RichText::new(format!(
                    "{joules:.0} J{}",
                    if complete { "" } else { " (partial)" }
                ))
                .size(11.0)
                .color(if complete { MUTED } else { WARN }),
            );
        }
    });
    ui.add_space(8.0);

    if !snapshot.foreign.is_empty() {
        section(ui, Icon::Warning, "Another cluster on this network", |ui| {
            for (name, endpoint) in &snapshot.foreign {
                ui.label(
                    RichText::new(format!("{name} at {endpoint}"))
                        .size(11.0)
                        .color(WARN),
                );
            }
            ui.label(
                RichText::new("No node reports this: discovery drops a name mismatch in silence.")
                    .size(10.0)
                    .color(MUTED)
                    .italics(),
            );
        });
        ui.add_space(8.0);
    }

    if snapshot.nodes.is_empty() {
        ui.label(
            RichText::new("No node contacted yet.")
                .size(11.0)
                .color(MUTED)
                .italics(),
        );
    }
    for node in &snapshot.nodes {
        node_card(ui, node);
        ui.add_space(8.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unmeasured value prints as `-`, not as zero. A zero here would be read as a card
    /// running at ambient temperature or drawing no power.
    #[test]
    fn an_absent_reading_is_drawn_absent() {
        assert_eq!(or_dash(None, "C"), "-");
        assert_eq!(or_dash(Some(0.0), "C"), "0C");
        assert_eq!(or_dash(Some(61.4), "C"), "61C");
    }

    /// A device reporting no capacity has no fraction, rather than a fraction of zero.
    #[test]
    fn a_device_without_capacity_has_no_fraction() {
        let empty = Device::default();
        assert_eq!(empty.used_fraction(), None);
        let card = Device {
            memory_bytes: 100,
            available_memory_bytes: Some(25),
            ..Default::default()
        };
        assert_eq!(card.used_fraction(), Some(0.75));
        assert_eq!(card.used_bytes(), Some(75));
        // The CPU row: capacity known, free memory not. Unmeasured, not full.
        let cpu = Device {
            memory_bytes: 100,
            available_memory_bytes: None,
            ..Default::default()
        };
        assert_eq!(cpu.used_bytes(), None);
        assert_eq!(cpu.used_fraction(), None);
    }

    /// An offline node draws the reason it is offline. Without it the card is empty, which
    /// reads as a node with nothing on it rather than a node that could not be reached.
    #[test]
    fn a_failed_node_carries_its_reason_into_the_drawing() {
        use egui_kittest::kittest::Queryable;

        fn drawn(errors: Vec<String>) -> bool {
            let node = Node {
                endpoint: "http://192.0.2.11:11435".into(),
                health: Health::Offline,
                rtt_ms: None,
                version: None,
                uptime_s: None,
                state: None,
                devices: vec![],
                placements: vec![],
                peers: vec![],
                energy_j: None,
                errors,
            };
            let mut harness = egui_kittest::Harness::new_ui(move |ui| node_card(ui, &node));
            harness.run();
            harness
                .query_by_label_contains("error sending request")
                .is_some()
        }
        assert!(drawn(vec!["/health: error sending request".into()]));
        // The same call with nothing to say must fail to find it, or the test proves nothing.
        assert!(!drawn(vec![]));
    }

    /// `last` is inclusive: a segment from 24 to 35 holds twelve layers, not eleven.
    #[test]
    fn a_segment_counts_its_last_layer() {
        let s = crate::model::Segment {
            first: 24,
            last: 35,
            ..Default::default()
        };
        assert_eq!(s.layers(), 12);
        let one = crate::model::Segment {
            first: 7,
            last: 7,
            ..Default::default()
        };
        assert_eq!(one.layers(), 1);
    }
}
