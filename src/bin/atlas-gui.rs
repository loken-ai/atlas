//! `atlas-gui` - the same cluster, drawn.
//!
//! A second view on one state, not a second tool: it renders the `ClusterSnapshot` the core
//! produces and shows what `doctor` reasons about, so what it displays and what the terminal
//! reports cannot drift apart.
//!
//! This binary resolves where the nodes are and keeps a polling loop fed; the window itself is
//! `atlas_core::app`, which is also what the documentation screenshot drives.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atlas_core::app::{Gui, Shared};
use clap::Parser;

#[derive(Parser)]
#[command(name = "atlas-gui", version, about = "Watch a loken cluster")]
struct Cli {
    /// A node to poll. Repeatable, and unioned with whatever the config file lists.
    #[arg(long = "node", value_name = "URL")]
    nodes: Vec<String>,

    /// A TOML file with a `[cluster]` block, same shape as a node's own config.
    #[arg(long, value_name = "PATH")]
    config: Option<std::path::PathBuf>,

    /// Do not listen for announcements. Needed when a node on this host already holds the
    /// discovery port, which is exactly the case when watching from a machine that serves.
    #[arg(long)]
    no_discovery: bool,

    /// Seconds between polls. `/api/cluster/state` is what the router prices a hand-over
    /// against, so a fast cadence distorts the thing being watched.
    #[arg(long, default_value_t = 5)]
    every: u64,
}

fn main() -> eframe::Result<()> {
    let cli = Cli::parse();
    let (mut endpoints, cluster) =
        match atlas_core::seeds::from_flags_and_file(&cli.nodes, cli.config.as_deref()) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        };

    let mut foreign = std::collections::BTreeMap::new();
    let mut note = None;
    if !cli.no_discovery {
        let (heard, problem) = atlas_core::seeds::add_heard(
            &mut endpoints,
            cluster.as_deref(),
            Duration::from_millis(1200),
        );
        foreign = heard;
        note = problem;
    }
    if endpoints.is_empty() {
        eprintln!("no nodes: pass --node URL, or --config with a [cluster] block");
        std::process::exit(2);
    }

    let shared = Arc::new(Mutex::new(Shared {
        note,
        ..Default::default()
    }));
    let refresh_now = Arc::new(AtomicBool::new(false));
    let every = Duration::from_secs(cli.every.max(1));

    eframe::run_native(
        "atlas",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 760.0]),
            ..Default::default()
        },
        Box::new({
            let shared = shared.clone();
            let refresh_now = refresh_now.clone();
            let endpoints_for_task = endpoints.clone();
            move |cc| {
                // Without these every SVG icon draws as the missing-image placeholder.
                egui_extras::install_image_loaders(&cc.egui_ctx);
                let ctx = cc.egui_ctx.clone();
                let shared_for_task = shared.clone();
                let refresh_for_task = refresh_now.clone();
                let cluster = cluster.clone();
                let foreign = foreign.clone();
                // Its own thread and runtime: the window must never wait on a node.
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("runtime");
                    rt.block_on(async move {
                        loop {
                            let mut snapshot =
                                atlas_core::collect::poll(&endpoints_for_task, cluster.clone())
                                    .await;
                            snapshot.foreign = foreign.clone();
                            let findings = atlas_core::doctor::all(&snapshot);
                            {
                                let mut shared = shared_for_task.lock().expect("shared state");
                                shared.snapshot = snapshot;
                                shared.findings = findings;
                                shared.polled_at = Some(Instant::now());
                            }
                            ctx.request_repaint();
                            // Wake early when Refresh is pressed rather than sleeping the
                            // whole cadence: the button is useless if it takes effect in five
                            // seconds.
                            let deadline = Instant::now() + every;
                            while Instant::now() < deadline {
                                if refresh_for_task
                                    .swap(false, std::sync::atomic::Ordering::Relaxed)
                                {
                                    break;
                                }
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }
                        }
                    });
                });
                Ok(Box::new(Gui {
                    shared,
                    endpoints,
                    every,
                    refresh_now,
                }))
            }
        }),
    )
}
