#[macro_use]
extern crate anyhow;
#[macro_use]
extern crate log;

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tokio::sync::Notify;

use crate::cmd::{CommandRun, Executable};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod cmd;
mod config;

#[derive(Parser)]
#[command(name = "Capybara")]
#[command(author = "Jeffsky <jjeffcaii@outlook.com>")]
#[command(version = "0.0.1")]
#[command(about = "A modern, simple and fast reserve proxy inspired by Envoy.", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a capybara proxy from a configuration file
    Run {
        #[arg(short, long, value_name = "FILE")]
        config: PathBuf,
    },
}

fn init() {
    pretty_env_logger::try_init_timed().ok();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init();

    let cli = Cli::parse();

    let shutdown = Arc::new(Notify::new());
    let stopped = Arc::new(Notify::new());

    match cli.command {
        Commands::Run { config } => {
            capybara_core::setup().await;

            let c = CommandRun::new(config);
            let shutdown = Clone::clone(&shutdown);
            let stopped = Clone::clone(&stopped);
            tokio::spawn(async move {
                match c.execute(shutdown).await {
                    Ok(()) => info!("capybara is stopped"),
                    Err(e) => error!("failed to execute command 'run': {}", e),
                }
                stopped.notify_waiters();
            });
        }
    }

    tokio::select! {
        _ = stopped.notified() => (),
        _ = tokio::signal::ctrl_c() => {
            info!("received signal ctrl-c, wait for graceful shutdown...");
            shutdown.notify_waiters();
            stopped.notified().await;
        }
    }

    Ok(())
}
