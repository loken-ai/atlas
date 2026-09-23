//! The living map, in a terminal.
//!
//! The rendering is a pure function of a `ClusterSnapshot`, so it is tested on snapshots built
//! by hand and needs no terminal and no cluster. Only the loop below touches the screen.

use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Frame, Terminal};

use crate::doctor::Finding;
use crate::model::{ClusterSnapshot, Device, Health, Node};

const GB: f64 = (1024u64 * 1024 * 1024) as f64;

fn health_style(h: Health) -> Style {
    Style::default().fg(match h {
        Health::Online => Color::Green,
        Health::Degraded => Color::Yellow,
        Health::Offline => Color::Red,
        Health::Unknown => Color::DarkGray,
    })
}

fn health_word(h: Health) -> &'static str {
    match h {
        Health::Online => "online",
        Health::Degraded => "degraded",
        Health::Offline => "offline",
        Health::Unknown => "unknown",
    }
}

/// A measurement that does not exist prints as `-`, never as zero.
fn or_dash(v: Option<f32>, unit: &str) -> String {
    v.map_or_else(|| "-".into(), |x| format!("{x:.0}{unit}"))
}

/// The banner: who this cluster is, how much of it is up, what it has drawn.
pub fn header_line(snapshot: &ClusterSnapshot) -> String {
    let (joules, complete) = snapshot.energy_j();
    let energy = if joules > 0.0 {
        // A sum over nodes that do not all report is labelled, never printed bare.
        format!(
            "  {joules:.0} J{}",
            if complete { "" } else { " (partial)" }
        )
    } else {
        String::new()
    };
    format!(
        "{}  {} of {} online{energy}",
        snapshot.cluster.as_deref().unwrap_or("cluster"),
        snapshot.online(),
        snapshot.nodes.len()
    )
}

/// One line per device. Returned rather than drawn so it can be tested.
pub fn device_line(d: &Device) -> String {
    format!(
        "{}:{} {:<26} {:>9}/{:<5.1} GB  {:>5}  {:>5}  {:>6}",
        d.device_type,
        d.device_id,
        d.name,
        // Unmeasured stays a dash: a device that reports no free memory is not a full one.
        d.used_bytes()
            .map_or("-".into(), |u| format!("{:.1}", u as f64 / GB)),
        d.memory_bytes as f64 / GB,
        // Compute load on the device. A card holding weights but sitting near zero here is the
        // sign that decode is running off it, which a memory bar alone cannot show. Absent on
        // the CPU row, which the server does not measure, so it reads as a dash there.
        or_dash(d.utilization_gpu_percent, "%"),
        or_dash(d.temperature_c, "C"),
        or_dash(d.power_watts, "W"),
    )
}

fn node_lines(node: &Node) -> Vec<Line<'static>> {
    // Name the node the way its operator does - "desktop", "laptop" - not by the URL a reader
    // has to decode. The endpoint follows, dimmed, for whoever needs to reach it. A node that
    // never gave an id falls back to its endpoint as the name.
    let name = node
        .state
        .as_ref()
        .map(|s| s.node_id.as_str())
        .filter(|id| !id.is_empty())
        .unwrap_or(node.endpoint.as_str());
    let mut out = vec![Line::from(vec![
        Span::styled(
            format!("{name:<16}"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<30}", node.endpoint),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("{:<9}", health_word(node.health)),
            health_style(node.health),
        ),
        Span::raw(format!(
            "{:>7}  {}",
            node.rtt_ms.map_or("-".into(), |v| format!("{v:.0}ms")),
            node.version.as_deref().unwrap_or("-")
        )),
    ])];
    if let Some(state) = &node.state {
        out.push(Line::styled(
            format!("    {} in flight, {} lane(s)", state.busy, state.lanes),
            Style::default().fg(Color::DarkGray),
        ));
        // Throughput the node has measured. Zero is never-measured, not zero speed, so an
        // unmeasured rate is a dash. Prefill and decode differ by an order of magnitude on a
        // streamed model, so they are named apart rather than summed.
        let rate = |v: f64| {
            if v > 0.0 {
                format!("{v:.1}")
            } else {
                "-".to_string()
            }
        };
        if state.decode_tok_per_s > 0.0 || state.prefill_tok_per_s > 0.0 {
            out.push(Line::styled(
                format!(
                    "    {} tok/s decode, {} tok/s prefill",
                    rate(state.decode_tok_per_s),
                    rate(state.prefill_tok_per_s)
                ),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    // What a busy node is actually running: the in-flight requests grouped by model and state,
    // so the model loading or answering is named rather than hidden behind a count.
    if !node.in_flight.is_empty() {
        let mut by: std::collections::BTreeMap<(String, String), usize> =
            std::collections::BTreeMap::new();
        for r in &node.in_flight {
            let model = if r.model.is_empty() {
                "unknown".to_string()
            } else {
                r.model.clone()
            };
            let state = if r.state.is_empty() {
                "running".to_string()
            } else {
                r.state.clone()
            };
            *by.entry((state, model)).or_default() += 1;
        }
        for ((state, model), n) in by {
            let line = if n > 1 {
                format!("    {state} {model} (x{n})")
            } else {
                format!("    {state} {model}")
            };
            out.push(Line::styled(line, Style::default().fg(Color::Cyan)));
        }
    }
    // A node with no device says so; a block that stops after its header reads as a node
    // with nothing wrong.
    if node.devices.is_empty() {
        out.push(Line::styled(
            "    no devices reported",
            Style::default().fg(Color::DarkGray),
        ));
    }
    for d in &node.devices {
        out.push(Line::raw(format!("    {}", device_line(d))));
    }
    for p in &node.placements {
        out.push(Line::styled(
            format!("    {}", p.headline()),
            Style::default().fg(Color::Cyan),
        ));
        for s in &p.segments {
            // The bytes a device holds. On a streamed model every entry spans all layers and
            // only the bytes tell the cards apart from each other; on a layer-split model they
            // confirm where the weight went. Absent (zero) stays off the line.
            let mem = if s.memory_bytes > 0 {
                format!("  {:.1} GB", s.memory_bytes as f64 / GB)
            } else {
                String::new()
            };
            out.push(Line::styled(
                format!(
                    "        {}:{} L{}-{} ({} layers){}",
                    s.device_type,
                    s.device_id,
                    s.first,
                    s.last,
                    s.layers(),
                    mem
                ),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    for e in &node.errors {
        out.push(Line::styled(
            format!("    {e}"),
            Style::default().fg(Color::Red),
        ));
    }
    for (model, layers) in node.layer_times_by_model() {
        out.push(Line::styled(
            format!("    {model}: ms to issue each layer, per token"),
            Style::default().fg(Color::Cyan),
        ));
        for row in layers.chunks(LAYER_CELLS_PER_LINE) {
            let cells: Vec<String> = row.iter().map(|t| layer_cell(t)).collect();
            out.push(Line::raw(format!("        {}", cells.join("  "))));
        }
    }
    if node.measuring && node.layer_times.iter().all(|t| t.tokens == 0) {
        out.push(Line::styled(
            "    measuring layer time: nothing decoded yet",
            Style::default().fg(Color::DarkGray),
        ));
    }
    out.push(Line::raw(""));
    out
}

/// How many layer cells share one line: a stack of forty layers is seven lines, not forty.
const LAYER_CELLS_PER_LINE: usize = 6;

/// One layer as a fixed-width cell, so the columns line up across the lines.
fn layer_cell(t: &crate::model::LayerTime) -> String {
    format!("L{:02} {:<5} {:>7.3}", t.layer, t.device, t.ms_per_token)
}

/// The same drawing the loop calls, reachable from a test so the documentation image comes from
/// the code rather than from a terminal someone happened to photograph.
#[cfg(test)]
pub fn draw_for_test(frame: &mut Frame, snapshot: &ClusterSnapshot, findings: &[Finding]) {
    draw(frame, snapshot, findings, None);
}

/// `note` says why discovery cannot listen, while it cannot.
fn draw(frame: &mut Frame, snapshot: &ClusterSnapshot, findings: &[Finding], note: Option<&str>) {
    // Both panels are composed before the layout, so each is sized from the lines it draws:
    // an empty verdict is one line of words, never a frame around nothing.
    let mut node_body = Vec::new();
    if let Some(note) = note {
        node_body.push(Line::styled(
            note.to_string(),
            Style::default().fg(Color::Yellow),
        ));
    }
    // A view with no seed runs only with discovery on, so an empty list is one still
    // waiting for an announcement.
    if snapshot.nodes.is_empty() {
        node_body.push(Line::styled(
            "listening for announcements",
            Style::default().fg(Color::DarkGray),
        ));
    }
    for node in &snapshot.nodes {
        node_body.extend(node_lines(node));
    }

    let verdict: Vec<Line> = if findings.is_empty() {
        vec![Line::styled(
            "nothing to report",
            Style::default().fg(Color::Green),
        )]
    } else {
        findings
            .iter()
            .map(|f| {
                Line::styled(
                    format!("{}: {}", crate::doctor::title(f.rule), f.detail),
                    Style::default().fg(Color::Yellow),
                )
            })
            .collect()
    };

    // A bordered panel spends two rows on its own frame; the cap bounds what a long verdict
    // may take from the nodes, which keep the remainder.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length((verdict.len() as u16 + 2).min(10)),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(header_line(snapshot))
            .block(Block::default().borders(Borders::ALL).title(" atlas ")),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(node_body).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" nodes   q quit  r refresh  m layer time "),
        ),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(verdict).block(Block::default().borders(Borders::ALL).title(" doctor ")),
        rows[2],
    );
}

/// What the draw loop asks of the background poller.
enum Cmd {
    Refresh,
    Measure(bool),
}

/// Run until `q` or Escape. The window paints from the first frame with the seeds drawn as
/// nodes not yet reached, and a background task polls and sends snapshots, so a node's latency
/// never holds the screen. With `discover`, a listener runs for the life of the view: every
/// round polls the seeds and whatever still announces, a node that announces between rounds
/// starts the next one at once, and a socket that cannot be opened is retried. What it hears
/// from another cluster rides in every snapshot's `foreign`, since no node reports a
/// neighbouring cluster and the `foreign-cluster` rule needs it here as much as `doctor` does.
pub async fn run(
    seeds: Vec<String>,
    cluster: Option<String>,
    every: Duration,
    discover: bool,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    crossterm::execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(ratatui::backend::CrosstermBackend::new(out))?;

    let mut snapshot = ClusterSnapshot {
        cluster: cluster.clone(),
        nodes: seeds.iter().map(|e| Node::unreached(e)).collect(),
        foreign: BTreeMap::new(),
    };
    let mut findings = crate::doctor::all(&snapshot);
    let mut note: Option<String> = None;

    let (snap_tx, snap_rx) = std::sync::mpsc::channel::<(ClusterSnapshot, Option<String>)>();
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Cmd>();
    // The endpoints of the latest round, kept by the draw loop for a synchronous measurement
    // cleanup on quit: the process exits when this returns, before the background task would
    // run its own.
    let polled = std::sync::Arc::new(std::sync::Mutex::new(seeds.clone()));
    {
        let cluster = cluster.clone();
        let polled = polled.clone();
        let mut previous = snapshot.clone();
        tokio::spawn(async move {
            let mut roster = crate::seeds::Roster::new(seeds, cluster.clone(), discover, every);
            let mut measuring = false;
            loop {
                let (joined, heard) = roster.refresh(&previous);
                let endpoints = roster.endpoints().to_vec();
                *polled.lock().unwrap_or_else(|e| e.into_inner()) = endpoints.clone();
                if measuring && !joined.is_empty() {
                    crate::collect::set_measuring(&joined, true).await;
                }
                let mut snap = crate::collect::poll(&endpoints, cluster.clone()).await;
                snap.foreign = heard.foreign;
                previous = snap.clone();
                if snap_tx.send((snap, heard.problem)).is_err() {
                    return;
                }
                // Wait `every`, but wake early on a command so a refresh or the measure toggle
                // is not held for a whole period, and on a node that just announced itself.
                let deadline = std::time::Instant::now() + every;
                loop {
                    match cmd_rx.try_recv() {
                        Ok(Cmd::Refresh) => break,
                        Ok(Cmd::Measure(on)) => {
                            crate::collect::set_measuring(&endpoints, on).await;
                            measuring = on;
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    }
                    if std::time::Instant::now() >= deadline || roster.newcomer() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        });
    }

    // Whether this view switched measurement on; what it switched on, it switches off.
    let mut measuring = false;
    let result = loop {
        if let Err(e) = terminal.draw(|f| draw(f, &snapshot, &findings, note.as_deref())) {
            break Err(e);
        }
        // Take the newest snapshot the poller produced, dropping any older ones behind it.
        let mut latest = None;
        while let Ok(s) = snap_rx.try_recv() {
            latest = Some(s);
        }
        if let Some((s, n)) = latest {
            snapshot = s;
            note = n;
            findings = crate::doctor::all(&snapshot);
        }
        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                    KeyCode::Char('r') => {
                        let _ = cmd_tx.send(Cmd::Refresh);
                    }
                    KeyCode::Char('m') => {
                        measuring = !measuring;
                        let _ = cmd_tx.send(Cmd::Measure(measuring));
                    }
                    _ => {}
                }
            }
        }
    };

    if measuring {
        let endpoints = polled.lock().unwrap_or_else(|e| e.into_inner()).clone();
        crate::collect::set_measuring(&endpoints, false).await;
    }
    disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeState;
    use ratatui::backend::TestBackend;

    /// What the terminal shows, borders and all: the layout sizes each panel, so only the
    /// drawn buffer says whether a composed line reaches the operator.
    fn rendered(snapshot: &ClusterSnapshot, findings: &[Finding]) -> String {
        let mut terminal = Terminal::new(TestBackend::new(96, 24)).expect("test backend");
        terminal
            .draw(|f| draw_for_test(f, snapshot, findings))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn node() -> Node {
        Node {
            endpoint: "http://192.0.2.1:11435".into(),
            health: Health::Online,
            rtt_ms: Some(3.0),
            version: Some("0.1.0".into()),
            uptime_s: Some(10.0),
            state: Some(NodeState {
                busy: 2,
                lanes: 1,
                ..Default::default()
            }),
            devices: vec![],
            placements: vec![],
            in_flight: vec![],
            peers: vec![],
            energy_j: Some(10.0),
            errors: vec![],
            measuring: false,
            layer_times: vec![],
        }
    }

    /// A device with no free-memory reading prints a dash, not `0.0`, which would draw a card
    /// entirely in use.
    #[test]
    fn an_unmeasured_device_prints_a_dash() {
        let cpu = Device {
            device_type: "CPU".into(),
            name: "CPU (20 cores)".into(),
            memory_bytes: 67 * 1024 * 1024 * 1024,
            available_memory_bytes: None,
            ..Default::default()
        };
        let line = device_line(&cpu);
        assert!(line.contains("-/"), "{line}");
        assert!(
            line.contains("     -"),
            "no temperature reads as a dash: {line}"
        );
    }

    /// An energy total that not every node contributed to says so.
    #[test]
    fn a_partial_energy_total_is_labelled_in_the_header() {
        let mut quiet = node();
        quiet.energy_j = None;
        let snap = ClusterSnapshot {
            cluster: Some("home".into()),
            nodes: vec![node(), quiet],
            ..Default::default()
        };
        let header = header_line(&snap);
        assert!(header.contains("(partial)"), "{header}");
        let whole = ClusterSnapshot {
            cluster: Some("home".into()),
            nodes: vec![node(), node()],
            ..Default::default()
        };
        assert!(!header_line(&whole).contains("partial"));
    }

    /// A resident model is drawn with its name, its layer count and each run of layers on
    /// a device, the last layer included.
    #[test]
    fn a_resident_model_is_drawn_with_its_layers_per_device() {
        let mut n = node();
        n.placements = vec![crate::model::Placement {
            model_id: "qwen3:0.6b".into(),
            status: "loaded".into(),
            device: None,
            total_layers: 28,
            segments: vec![crate::model::Segment {
                device_type: "CUDA".into(),
                device_id: 0,
                first: 0,
                last: 27,
                memory_bytes: 522_640_096,
            }],
        }];
        let text: String = node_lines(&n)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(text.contains("qwen3:0.6b - 28 layers"), "{text}");
        assert!(text.contains("CUDA:0 L0-27 (28 layers)"), "{text}");
    }

    /// A node that failed shows the reason, rather than an empty card that reads as healthy.
    #[test]
    fn an_error_is_shown_on_the_node_it_belongs_to() {
        let mut broken = node();
        broken.health = Health::Degraded;
        broken.errors = vec!["/api/distributed/devices: decode".into()];
        let text: String = node_lines(&broken)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(text.contains("degraded"));
        assert!(text.contains("decode"));
    }

    /// With no node yet, the view says it is waiting for one rather than that none exists.
    #[test]
    fn an_empty_cluster_is_shown_as_listening() {
        let screen = rendered(&ClusterSnapshot::default(), &[]);
        assert!(
            screen.contains("listening for announcements"),
            "no word on the empty panel:\n{screen}"
        );
    }

    /// A clean cluster is told so in words, in the panel and not clipped out of it.
    #[test]
    fn an_empty_verdict_is_worded_rather_than_drawn_as_an_empty_box() {
        let snapshot = ClusterSnapshot {
            cluster: Some("home".into()),
            nodes: vec![node()],
            ..Default::default()
        };
        let screen = rendered(&snapshot, &[]);
        assert!(
            screen.contains("nothing to report"),
            "the verdict is composed but never drawn:\n{screen}"
        );
    }

    /// A verdict that exists is drawn whole.
    #[test]
    fn a_finding_reaches_the_screen() {
        let snapshot = ClusterSnapshot {
            cluster: Some("home".into()),
            nodes: vec![node()],
            ..Default::default()
        };
        let findings = vec![Finding {
            rule: "version-skew",
            detail: "0.1.0 and 0.2.0".into(),
        }];
        let screen = rendered(&snapshot, &findings);
        assert!(
            screen.contains(crate::doctor::title("version-skew")),
            "{screen}"
        );
        assert!(screen.contains("0.1.0 and 0.2.0"), "{screen}");
    }

    /// A node that reports no device says so.
    #[test]
    fn a_node_without_devices_says_so_rather_than_leaving_a_blank() {
        let mut bare = node();
        bare.devices = vec![];
        let text: String = node_lines(&bare)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(text.contains("no devices reported"), "{text}");
    }

    /// A foreign cluster reaches the terminal's verdict: only the passive listener hears one,
    /// so the loop has to carry what it heard into every snapshot.
    #[test]
    fn a_foreign_cluster_reaches_the_verdict() {
        let mut foreign = BTreeMap::new();
        foreign.insert("lab".to_string(), "http://192.0.2.9:11435".to_string());
        let snapshot = ClusterSnapshot {
            cluster: Some("home".into()),
            nodes: vec![node()],
            foreign,
        };
        let findings = crate::doctor::all(&snapshot);
        assert!(
            findings.iter().any(|f| f.rule == "foreign-cluster"),
            "the rule fires on the snapshot the loop now builds: {findings:?}"
        );
        let screen = rendered(&snapshot, &findings);
        assert!(
            screen.contains(crate::doctor::title("foreign-cluster")),
            "{screen}"
        );
    }
}

#[cfg(test)]
mod headline_tests {
    use crate::model::Placement;

    #[test]
    fn a_render_in_progress_shows_its_status_rather_than_zero_layers() {
        let rendering = Placement {
            model_id: "ace-step".into(),
            status: "rendering sound".into(),
            device: None,
            total_layers: 0,
            segments: vec![],
        };
        assert_eq!(rendering.headline(), "ace-step - rendering sound");
        let part = Placement {
            model_id: "ace-step (lm)".into(),
            status: "rendering sound: codes 12/300".into(),
            device: Some("CUDA".into()),
            total_layers: 36,
            segments: vec![],
        };
        assert_eq!(
            part.headline(),
            "ace-step (lm) - 36 layers - rendering sound: codes 12/300"
        );
        let placed = Placement {
            model_id: "qwen3:8b".into(),
            status: "loaded".into(),
            device: Some("CUDA".into()),
            total_layers: 36,
            segments: vec![],
        };
        assert_eq!(placed.headline(), "qwen3:8b - 36 layers");
        // A node that names a device and no layers is not claiming zero of them.
        let whole = Placement {
            model_id: "kyutai-default".into(),
            status: "loaded".into(),
            device: Some("CUDA".into()),
            total_layers: 0,
            segments: vec![],
        };
        assert_eq!(whole.headline(), "kyutai-default - loaded on CUDA");
        let old_node = Placement {
            model_id: "kyutai-default".into(),
            status: String::new(),
            device: None,
            total_layers: 0,
            segments: vec![],
        };
        assert_eq!(old_node.headline(), "kyutai-default - loaded");
    }
}

#[cfg(test)]
mod layer_time_tests {
    use crate::model::{Health, LayerTime, Node};

    fn measured() -> Node {
        let at = |layer: u32, device: &str, ms: f64, tokens: u64| LayerTime {
            model: "qwen3:8b".into(),
            layer,
            device: device.into(),
            ms_per_token: ms,
            tokens,
        };
        Node {
            endpoint: "http://192.0.2.10:11435".into(),
            health: Health::Online,
            rtt_ms: Some(1.0),
            version: Some("0.1.0".into()),
            uptime_s: Some(10.0),
            state: None,
            devices: vec![],
            placements: vec![],
            in_flight: vec![],
            peers: vec![],
            energy_j: None,
            errors: vec![],
            measuring: true,
            layer_times: vec![
                at(1, "CPU", 4.5, 12),
                at(0, "CUDA0", 0.041, 12),
                at(2, "CUDA0", 0.0, 0),
            ],
        }
    }

    fn text(node: &Node) -> String {
        super::node_lines(node)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn measured_layers_are_listed_in_order_and_unmeasured_ones_are_not() {
        let text = text(&measured());
        assert!(text.contains("qwen3:8b: ms to issue each layer, per token"));
        assert!(text.contains("L00 CUDA0   0.041  L01 CPU     4.500"));
        assert!(!text.contains("L02"));
    }

    #[test]
    fn a_node_measuring_with_nothing_decoded_says_so() {
        let mut node = measured();
        node.layer_times.iter_mut().for_each(|t| t.tokens = 0);
        assert!(text(&node).contains("measuring layer time: nothing decoded yet"));
        node.measuring = false;
        assert!(!text(&node).contains("measuring"));
    }
}
