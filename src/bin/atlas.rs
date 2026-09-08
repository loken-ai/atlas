//! `atlas` - watch a loken cluster, or ask it what is wrong, or export what only an observer
//! can measure.
//!
//! Node addresses come from the command line, from a config file, or from listening. None of
//! them are compiled in: this tool exists partly because the harness it replaces curled two
//! hardcoded addresses that would rot in a repository.

use std::process::ExitCode;
use std::time::Duration;

use atlas_core::{doctor, metrics};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "atlas", version, about = "Watch a loken cluster")]
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

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// The living map. Default when no command is given.
    Watch {
        /// Seconds between polls. `/api/cluster/state` is what the router prices a hand-over
        /// against, so a fast cadence distorts the thing being watched.
        #[arg(long, default_value_t = 5)]
        every: u64,
    },
    /// Report inconsistencies. Exits with the number of rules that fired.
    Doctor,
    /// Serve OpenMetrics for what no single node can measure.
    Export {
        #[arg(long, default_value = "127.0.0.1:9800")]
        listen: String,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let (endpoints, cluster) =
        match atlas_core::seeds::from_flags_and_file(&cli.nodes, cli.config.as_deref()) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(2);
            }
        };
    // Listening, never announcing: a probe that joins the group becomes a peer the router can
    // hand work to. It is also the only way to see a cluster that is not this one.
    let mut endpoints = endpoints;
    let mut foreign = std::collections::BTreeMap::new();
    if !cli.no_discovery {
        let (heard, problem) = atlas_core::seeds::add_heard(
            &mut endpoints,
            cluster.as_deref(),
            Duration::from_millis(1200),
        );
        foreign = heard;
        // A node on this host already holds the port on the machine most likely to be watched
        // from. Say so once and carry on with the seeds.
        if let Some(problem) = problem {
            eprintln!("{problem}");
        }
    }
    if endpoints.is_empty() {
        eprintln!("no nodes: pass --node URL, or --config with a [cluster] block");
        return ExitCode::from(2);
    }

    match cli.command.unwrap_or(Command::Watch { every: 5 }) {
        Command::Watch { every } => {
            return match atlas_core::tui::run(
                endpoints,
                cluster,
                Duration::from_secs(every.max(1)),
                foreign,
            )
            .await
            {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::from(74)
                }
            };
        }
        Command::Doctor => {
            let mut snapshot = atlas_core::collect::poll(&endpoints, cluster).await;
            snapshot.foreign = foreign;
            let findings = doctor::all(&snapshot);
            for f in &findings {
                println!("{}: {}", doctor::title(f.rule), f.detail);
            }
            // The exit status is the number of rules that fired, as `preflight.sh` does.
            ExitCode::from(u8::try_from(findings.len()).unwrap_or(u8::MAX))
        }
        Command::Export { listen } => {
            let _ = listen;
            let mut snapshot = atlas_core::collect::poll(&endpoints, cluster).await;
            snapshot.foreign = foreign;
            print!("{}", metrics::encode(&snapshot, &doctor::all(&snapshot)));
            ExitCode::SUCCESS
        }
    }
}
