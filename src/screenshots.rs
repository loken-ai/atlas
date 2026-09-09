//! The screenshots the documentation shows, drawn by the code that draws the application.
//!
//! `cargo test --release screenshots -- --ignored` writes them under `docs/img/`. Ignored by
//! default: it writes files and asks for a graphics adapter, where the rest of the suite is a
//! pure check that runs anywhere.
//!
//! A capture taken by hand drifts from the build the moment either moves, and it carries
//! whatever happened to be on screen - a node's address, a model someone was running. This
//! renders the real layout from a snapshot written below, so a screenshot cannot show anything
//! this file did not put in it. The snapshot is a working cluster rather than an empty one: a
//! default state hides everything the view exists for, including whatever is broken in it.

#![cfg(all(test, feature = "gui"))]

use crate::model::{ClusterSnapshot, Device, Health, Node, NodeState, Placement, Segment};

const OUT: &str = "docs/img";
const GB: u64 = 1024 * 1024 * 1024;

fn gpu(id: usize, name: &str, free: u64, temp: f32, watts: f32, cap: f32) -> Device {
    Device {
        device_type: "CUDA".into(),
        device_id: id,
        name: name.into(),
        memory_bytes: 16 * GB,
        available_memory_bytes: Some(free),
        status: "available".into(),
        utilization_gpu_percent: Some(if id == 0 { 71.0 } else { 4.0 }),
        temperature_c: Some(temp),
        power_watts: Some(watts),
        power_limit_watts: Some(cap),
    }
}

fn node(endpoint: &str, health: Health) -> Node {
    Node {
        endpoint: endpoint.into(),
        health,
        rtt_ms: Some(if health == Health::Online { 3.0 } else { 228.0 }),
        version: (health == Health::Online).then(|| "0.1.0".to_string()),
        uptime_s: Some(9826.0),
        state: Some(NodeState {
            node_id: "desktop".into(),
            busy: 3,
            lanes: 1,
            serves: Some(vec!["qwen3:8b".into(), "llama3.2:1b".into()]),
            ..Default::default()
        }),
        devices: vec![],
        placements: vec![],
        peers: vec![],
        energy_j: Some(3290.8),
        errors: vec![],
        measuring: false,
        layer_times: vec![],
    }
}

/// A two-node cluster with one node down, a model split across two cards, and a CPU row that
/// reports no free memory - the three things the view has to draw honestly.
fn cluster() -> ClusterSnapshot {
    let mut desktop = node("http://192.0.2.10:11435", Health::Online);
    desktop.devices = vec![
        gpu(0, "NVIDIA GeForce RTX 5070 Ti", 6 * GB, 61.0, 214.0, 300.0),
        gpu(1, "NVIDIA GeForce RTX 5060 Ti", 15 * GB, 38.0, 12.0, 180.0),
        Device {
            device_type: "CPU".into(),
            device_id: 2,
            name: "CPU (20 cores)".into(),
            memory_bytes: 62 * GB,
            // No free-memory reading. It must draw as unmeasured, not as a full machine.
            available_memory_bytes: None,
            status: "available".into(),
            ..Default::default()
        },
    ];
    desktop.placements = vec![Placement {
        model_id: "qwen3:8b".into(),
        status: "loaded".into(),
        device: Some("CUDA".into()),
        total_layers: 36,
        segments: vec![
            Segment {
                device_type: "CUDA".into(),
                device_id: 0,
                first: 0,
                last: 23,
                memory_bytes: 3 * GB,
            },
            Segment {
                device_type: "CUDA".into(),
                device_id: 1,
                first: 24,
                last: 35,
                memory_bytes: GB + GB / 2,
            },
        ],
    }];

    let mut laptop = node("http://192.0.2.11:11435", Health::Offline);
    laptop.state = None;
    // No energy from this one, which makes the cluster total partial and says so.
    laptop.energy_j = None;
    laptop.errors = vec!["/health: error sending request".into()];

    ClusterSnapshot {
        cluster: Some("home".into()),
        nodes: vec![desktop, laptop],
        foreign: [("staging".to_string(), "http://192.0.2.40:11435".to_string())].into(),
    }
}

#[test]
#[ignore = "writes docs/img and needs a graphics adapter"]
fn cluster_window() {
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use crate::app::{Gui, Shared};

    let snapshot = cluster();
    let findings = crate::doctor::all(&snapshot);
    let shared = Arc::new(Mutex::new(Shared {
        snapshot,
        findings,
        // A reading that has just landed, so the age in the bar reads as a live window rather
        // than one that never polled.
        polled_at: Some(Instant::now()),
        note: None,
    }));

    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1100.0, 760.0))
        .build_eframe(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Gui {
                measure: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                shared,
                endpoints: vec![
                    "http://192.0.2.10:11435".to_string(),
                    "http://192.0.2.11:11435".to_string(),
                ],
                every: Duration::from_secs(5),
                refresh_now: Arc::new(AtomicBool::new(false)),
            }
        });
    egui_extras::install_image_loaders(&harness.ctx);
    harness.run_steps(4);
    std::fs::create_dir_all(OUT).expect("docs/img");
    harness
        .render()
        .expect("a graphics adapter")
        .save(format!("{OUT}/atlas-gui.png"))
        .unwrap_or_else(|e| panic!("write atlas-gui.png: {e}"));
}

/// The terminal view, rendered through ratatui's test backend: no terminal, no escape codes, and
/// the same `draw` the binary calls. Written as SVG so the text stays text - a terminal
/// screenshot as pixels is unsearchable and unreadable when the page is scaled.
#[test]
#[ignore = "writes docs/img"]
fn tui_view() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let snapshot = cluster();
    let findings = crate::doctor::all(&snapshot);
    let mut terminal = Terminal::new(TestBackend::new(96, 24)).expect("test backend");
    terminal
        .draw(|f| crate::tui::draw_for_test(f, &snapshot, &findings))
        .expect("draw");

    let buffer = terminal.backend().buffer().clone();
    let (w, h) = (96usize, 24usize);
    let (cw, ch) = (8.4, 17.0);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.0}\" height=\"{:.0}\" \
         viewBox=\"0 0 {:.0} {:.0}\" font-family=\"DejaVu Sans Mono, monospace\" \
         font-size=\"13\">\n<rect width=\"100%\" height=\"100%\" fill=\"#0d1117\"/>\n",
        w as f64 * cw,
        h as f64 * ch,
        w as f64 * cw,
        h as f64 * ch
    );
    for y in 0..h {
        // One <text> per run of equal colour, so the SVG stays small and the text stays
        // selectable rather than becoming one span per character.
        let mut x = 0usize;
        while x < w {
            let colour = svg_colour(buffer[(x as u16, y as u16)].fg);
            let start = x;
            let mut run = String::new();
            while x < w && svg_colour(buffer[(x as u16, y as u16)].fg) == colour {
                run.push_str(buffer[(x as u16, y as u16)].symbol());
                x += 1;
            }
            if !run.trim().is_empty() {
                // textLength pins the run to the character grid. Without it the box-drawing
                // borders drift by whatever the renderer's advance width happens to be, and
                // the frame comes apart a few columns in.
                svg.push_str(&format!(
                    "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"{colour}\" textLength=\"{:.1}\" \
                     lengthAdjust=\"spacingAndGlyphs\" xml:space=\"preserve\">{}</text>\n",
                    start as f64 * cw,
                    (y + 1) as f64 * ch - 4.0,
                    (x - start) as f64 * cw,
                    run.replace('&', "&amp;").replace('<', "&lt;")
                ));
            }
        }
    }
    svg.push_str("</svg>\n");
    std::fs::create_dir_all(OUT).expect("docs/img");
    std::fs::write(format!("{OUT}/atlas-tui.svg"), svg).expect("write atlas-tui.svg");
}

fn svg_colour(c: ratatui::style::Color) -> &'static str {
    use ratatui::style::Color;
    match c {
        Color::Green => "#3fb950",
        Color::Yellow => "#d29922",
        Color::Red => "#f85149",
        Color::Cyan => "#39c5cf",
        Color::DarkGray => "#8b949e",
        _ => "#e6edf3",
    }
}
