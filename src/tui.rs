//! The living map, in a terminal.
//!
//! The rendering is a pure function of a `ClusterSnapshot`, so it is tested on snapshots built
//! by hand and needs no terminal and no cluster. Only the loop below touches the screen.

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
        "{}:{} {:<26} {:>9}/{:<5.1} GB  {:>5}  {:>6}",
        d.device_type,
        d.device_id,
        d.name,
        // Unmeasured stays a dash: a device that reports no free memory is not a full one.
        d.used_bytes()
            .map_or("-".into(), |u| format!("{:.1}", u as f64 / GB)),
        d.memory_bytes as f64 / GB,
        or_dash(d.temperature_c, "C"),
        or_dash(d.power_watts, "W"),
    )
}

fn node_lines(node: &Node) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(vec![
        Span::styled(
            format!("{:<30}", node.endpoint),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<10}", health_word(node.health)),
            health_style(node.health),
        ),
        Span::raw(format!(
            "{:>8}  {}",
            node.rtt_ms.map_or("-".into(), |v| format!("{v:.0}ms")),
            node.version.as_deref().unwrap_or("-")
        )),
    ])];
    if let Some(state) = &node.state {
        out.push(Line::styled(
            format!("    {} in flight, {} lane(s)", state.busy, state.lanes),
            Style::default().fg(Color::DarkGray),
        ));
    }
    for d in &node.devices {
        out.push(Line::raw(format!("    {}", device_line(d))));
    }
    for p in &node.placements {
        out.push(Line::styled(
            format!("    {} - {} layers", p.model_id, p.total_layers),
            Style::default().fg(Color::Cyan),
        ));
        for s in &p.segments {
            out.push(Line::styled(
                format!(
                    "        {}:{} L{}-{} ({} layers)",
                    s.device_type,
                    s.device_id,
                    s.first,
                    s.last,
                    s.layers()
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
    out.push(Line::raw(""));
    out
}

/// The same drawing the loop calls, reachable from a test so the documentation image comes from
/// the code rather than from a terminal someone happened to photograph.
#[cfg(test)]
pub fn draw_for_test(frame: &mut Frame, snapshot: &ClusterSnapshot, findings: &[Finding]) {
    draw(frame, snapshot, findings);
}

fn draw(frame: &mut Frame, snapshot: &ClusterSnapshot, findings: &[Finding]) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length((findings.len() as u16 + 2).min(10)),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(header_line(snapshot))
            .block(Block::default().borders(Borders::ALL).title(" atlas ")),
        rows[0],
    );

    let mut lines = Vec::new();
    if snapshot.nodes.is_empty() {
        lines.push(Line::styled(
            "no node contacted",
            Style::default().fg(Color::DarkGray),
        ));
    }
    for node in &snapshot.nodes {
        lines.extend(node_lines(node));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" nodes ")),
        rows[1],
    );

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
                    format!("{:<24} {}", f.rule, f.detail),
                    Style::default().fg(Color::Yellow),
                )
            })
            .collect()
    };
    frame.render_widget(
        Paragraph::new(verdict).block(Block::default().borders(Borders::ALL).title(" doctor ")),
        rows[2],
    );
}

/// Run until `q` or Escape. Polls on `every`, and repaints between polls so the terminal stays
/// responsive without asking the nodes anything more often.
pub async fn run(
    endpoints: Vec<String>,
    cluster: Option<String>,
    every: Duration,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    crossterm::execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(ratatui::backend::CrosstermBackend::new(out))?;

    let mut snapshot = crate::collect::poll(&endpoints, cluster.clone()).await;
    let mut findings = crate::doctor::all(&snapshot);
    let mut last = std::time::Instant::now();

    let result = loop {
        if let Err(e) = terminal.draw(|f| draw(f, &snapshot, &findings)) {
            break Err(e);
        }
        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    break Ok(());
                }
                if matches!(key.code, KeyCode::Char('r')) {
                    last = std::time::Instant::now() - every;
                }
            }
        }
        if last.elapsed() >= every {
            snapshot = crate::collect::poll(&endpoints, cluster.clone()).await;
            findings = crate::doctor::all(&snapshot);
            last = std::time::Instant::now();
        }
    };

    disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeState;

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
            peers: vec![],
            energy_j: Some(10.0),
            errors: vec![],
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
}
