use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// Gateway for a public satd node.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// The configuration file.
    #[arg(
        long,
        short,
        env = "SATD_SITE_CONFIG",
        default_value = "satd-site.toml"
    )]
    config: PathBuf,
    /// Check the configuration and exit.
    #[arg(long)]
    check: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let config = match satd_site::config::Config::load(&args.config) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("satd-site: {e}");
            return ExitCode::from(2);
        }
    };
    if args.check {
        println!("{}: ok", args.config.display());
        return ExitCode::SUCCESS;
    }
    let running = match satd_site::start(&config).await {
        Ok(running) => running,
        Err(e) => {
            eprintln!("satd-site: {e}");
            return ExitCode::FAILURE;
        }
    };
    let networks: Vec<_> = config.networks.iter().map(|n| n.name.as_str()).collect();
    tracing::info!(addr = %running.addr, ?networks, "listening");
    match running.server.await {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(e)) => {
            eprintln!("satd-site: {e}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("satd-site: server task failed: {e}");
            ExitCode::FAILURE
        }
    }
}
