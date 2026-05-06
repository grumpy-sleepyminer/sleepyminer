use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "sleepyminer")]
#[command(about = "Native Monero CPU miner for Apple Silicon with adaptive thread scaling")]
#[command(version)]
pub struct Cli {
    /// Config file path
    #[arg(short, long, default_value = "~/.sleepyminer/config.json")]
    pub config: String,

    /// Pool URL (overrides config)
    #[arg(short = 'o', long)]
    pub url: Option<String>,

    /// Wallet address (overrides config)
    #[arg(short = 'u', long)]
    pub user: Option<String>,

    /// Pool password
    #[arg(short = 'p', long, default_value = "x")]
    pub pass: String,

    /// Thread count (overrides config)
    #[arg(short = 't', long)]
    pub threads: Option<usize>,

    /// Dev donation percentage (min 1)
    #[arg(long, default_value = "1")]
    pub donate_level: u8,

    /// Verbose logging
    #[arg(short, long)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run the miner (default)
    Run,
    /// Interactive setup + config optimization
    Setup,
    /// Quick hashrate benchmark
    Benchmark,
    /// Install macOS launchd service for login startup
    InstallService,
    /// Remove macOS launchd service
    UninstallService,
}

impl Cli {
    pub fn resolve_config_path(&self) -> std::path::PathBuf {
        let path = self
            .config
            .replace("~", &std::env::var("HOME").unwrap_or_default());
        std::path::PathBuf::from(path)
    }
}
