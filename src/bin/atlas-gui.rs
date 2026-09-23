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
    let (seeds, cluster) =
        match atlas_core::seeds::from_flags_and_file(&cli.nodes, cli.config.as_deref()) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        };
    if seeds.is_empty() && cli.no_discovery {
        eprintln!(
            "no nodes: pass --node URL, --config with a [cluster] block, or drop --no-discovery"
        );
        std::process::exit(2);
    }

    let shared = Arc::new(Mutex::new(Shared {
        endpoints: seeds.clone(),
        ..Default::default()
    }));
    let refresh_now = Arc::new(AtomicBool::new(false));
    let every = Duration::from_secs(cli.every.max(1));
    let discover = !cli.no_discovery;

    eframe::run_native(
        "atlas",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 760.0]),
            ..Default::default()
        },
        Box::new({
            let shared = shared.clone();
            let refresh_now = refresh_now.clone();
            move |cc| {
                // Without these every SVG icon draws as the missing-image placeholder.
                egui_extras::install_image_loaders(&cc.egui_ctx);
                let ctx = cc.egui_ctx.clone();
                let shared_for_task = shared.clone();
                let refresh_for_task = refresh_now.clone();
                let measure = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let measure_for_task = measure.clone();
                let mut measuring = false;
                let mut roster =
                    atlas_core::seeds::Roster::new(seeds, cluster.clone(), discover, every);
                // Its own thread and runtime: the window must never wait on a node.
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("runtime");
                    rt.block_on(async move {
                        let mut previous = atlas_core::model::ClusterSnapshot::default();
                        loop {
                            let (joined, heard) = roster.refresh(&previous);
                            let endpoints = roster.endpoints().to_vec();
                            let wanted =
                                measure_for_task.load(std::sync::atomic::Ordering::Relaxed);
                            if wanted != measuring {
                                atlas_core::collect::set_measuring(&endpoints, wanted).await;
                                measuring = wanted;
                            } else if measuring && !joined.is_empty() {
                                atlas_core::collect::set_measuring(&joined, true).await;
                            }
                            let mut snapshot =
                                atlas_core::collect::poll(&endpoints, cluster.clone()).await;
                            snapshot.foreign = heard.foreign;
                            previous = snapshot.clone();
                            let findings = atlas_core::doctor::all(&snapshot);
                            {
                                let mut shared = shared_for_task.lock().expect("shared state");
                                shared.snapshot = snapshot;
                                shared.findings = findings;
                                shared.polled_at = Some(Instant::now());
                                shared.note = heard.problem;
                                shared.endpoints = endpoints;
                            }
                            ctx.request_repaint();
                            // Wake early when Refresh is pressed rather than sleeping the
                            // whole cadence: the button is useless if it takes effect in five
                            // seconds. A node that just announced itself wakes it too.
                            let deadline = Instant::now() + every;
                            while Instant::now() < deadline {
                                if refresh_for_task
                                    .swap(false, std::sync::atomic::Ordering::Relaxed)
                                    || roster.newcomer()
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
                    every,
                    refresh_now,
                    measure,
                }))
            }
        }),
    )
}
