#![allow(dead_code, unused_imports)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use tokio::sync::{mpsc, watch, Notify};

mod algo;
mod animation;
mod benchmark;
mod config;
mod donation;
mod miner;
mod randomx;
mod scaling;
mod logging;
mod service;
mod stratum;

use config::cli::{Cli, Commands};
use config::{Config, PoolConfig};
use miner::MiningCoordinator;
use scaling::ActivityScaler;
use stratum::pool::PoolConnection;
use stratum::protocol::MiningJob;
use stratum::{ShareSubmission, StratumClient, StratumStats};

const BANNER_ANSI: &str = include_str!("../assets/banner.ansi");

fn print_banner() {
    println!();
    print!("{}", BANNER_ANSI);
    let letters = ['s', 'l', 'e', 'e', 'p', 'y', 'm', 'i', 'n', 'e', 'r'];
    let colors = [196, 202, 208, 220, 46, 51, 39, 27, 93, 129, 200];
    print!("   ");
    for (ch, col) in letters.iter().zip(colors.iter()) {
        print!("\x1b[1;38;5;{}m{}", col, ch);
    }
    println!(
        "\x1b[0m  \x1b[2;37mv{}\x1b[0m  \x1b[38;5;39mNative Monero miner for Apple Silicon\x1b[0m",
        env!("CARGO_PKG_VERSION")
    );
    println!();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    logging::init(cli.verbose);

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let command = cli.command.as_ref().unwrap_or(&Commands::Run);

    match command {
        Commands::Setup => run_setup(&cli).await,
        Commands::Benchmark => run_benchmark(&cli).await,
        Commands::InstallService => {
            let config_path = cli.resolve_config_path();
            service::install_service(&config_path.to_string_lossy())?;
            Ok(())
        }
        Commands::UninstallService => {
            service::uninstall_service()?;
            Ok(())
        }
        Commands::Run => run_miner(&cli).await,
    }
}

async fn run_miner(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    print_banner();

    let config_path = cli.resolve_config_path();
    let mut ran_wizard = false;
    let mut config = if config_path.exists() {
        Config::load(&config_path)?
    } else if cli.url.is_some() && cli.user.is_some() {
        let url = cli.url.as_deref().unwrap();
        let wallet = cli.user.as_deref().unwrap();
        config::generate_default_config(url, wallet, &cli.pass)
    } else {
        // First-run: nothing on disk and no -o/-u. Drop into the wizard.
        println!(
            "  \x1b[1;38;5;220mNo config found.\x1b[0m Let's set one up.\n"
        );
        run_wizard(&config_path).await?;
        ran_wizard = true;
        Config::load(&config_path)?
    };

    // Play the shooting star once, BEFORE any mining task spawns or log lines.
    // The wizard already played it on its way out, so don't double up.
    // Runs on a blocking thread so we don't stall the tokio runtime — and we
    // explicitly await it so the animation fully completes before any async
    // mining tasks are spawned below.
    if !ran_wizard {
        let _ = tokio::task::spawn_blocking(animation::shooting_star).await;
    }

    if let Some(ref url) = cli.url {
        if let Some(pool) = config.pools.first_mut() {
            pool.url = url.clone();
        }
    }
    if let Some(ref wallet) = cli.user {
        if let Some(pool) = config.pools.first_mut() {
            pool.wallet = wallet.clone();
        }
    }
    if let Some(threads) = cli.threads {
        config.threads = Some(threads);
    }
    config.donate_level = cli.donate_level.max(1);
    config.verbose = cli.verbose;

    let max_threads = config.max_threads();
    let active_pools = config.active_pools();
    if active_pools.is_empty() {
        return Err("No active pools configured.".into());
    }

    for p in &active_pools {
        log::info!("pool {}  algo {}", p.url, p.algo.name());
    }
    let w = &active_pools[0].wallet;
    log::info!(
        "wallet {}...{}",
        &w[..w.len().min(8)],
        &w[w.len().saturating_sub(6)..]
    );
    log::info!("threads {}  donate {}%", max_threads, config.donate_level);

    let user_wallet = active_pools[0].wallet.clone();

    let pool_connections: Vec<PoolConnection> = active_pools
        .iter()
        .map(|p| PoolConnection::from_config(p))
        .collect::<Result<Vec<_>, _>>()?;

    let (job_tx, job_rx) = watch::channel::<Option<MiningJob>>(None);
    let (submit_tx, submit_rx) = mpsc::channel::<ShareSubmission>(64);

    let fixed_threads = cli.threads.is_some() || config.threads.is_some();
    let initial_threads = if fixed_threads {
        max_threads
    } else {
        config.min_threads
    };
    let target_threads = Arc::new(AtomicUsize::new(initial_threads));

    let mut coordinator = MiningCoordinator::new(max_threads, target_threads.clone());
    let _worker_handles = coordinator.start(job_rx, submit_tx);

    let stats = Arc::new(tokio::sync::Mutex::new(StratumStats {
        accepted: 0,
        rejected: 0,
    }));

    let mut stratum = StratumClient::new(pool_connections, config.retries, config.retry_pause);

    // Donation wiring: attach a reconnect signal and grab the override handle.
    // The donation task will toggle the dev pool in/out of the override slot.
    let donation_signal = Arc::new(Notify::new());
    let override_handle = stratum.enable_donation_switching(donation_signal.clone());

    let stats_clone = stats.clone();
    tokio::spawn(async move {
        stratum.run(job_tx, submit_rx, stats_clone).await;
    });

    if !fixed_threads {
        let mut scaler = ActivityScaler::new(
            config.min_threads,
            max_threads,
            config.idle_threshold,
            config.ramp_up_speed,
            target_threads.clone(),
        );
        tokio::spawn(async move {
            scaler.run().await;
        });
    } else {
        log::info!("Auto-scaling disabled (fixed {} threads)", max_threads);
    }

    // Spawn donation time-slice task.
    {
        let manager = donation::DonationManager::new(config.donate_level, &user_wallet);
        let handle = override_handle.clone();
        let signal = donation_signal.clone();
        tokio::spawn(async move {
            donation::run_donation_loop(manager, handle, signal).await;
        });
    }

    let print_interval = config.print_interval;
    loop {
        tokio::time::sleep(Duration::from_secs(print_interval)).await;
        let s = stats.lock().await;
        miner::print_status(&coordinator, s.accepted, s.rejected);
    }
}

/// Rewritten setup subcommand: delegates to the shared wizard.
async fn run_setup(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    print_banner();
    let config_path = cli.resolve_config_path();
    run_wizard(&config_path).await?;
    println!(
        "\n\x1b[1;38;5;46m✓\x1b[0m Setup complete. Run \x1b[1msleepyminer\x1b[0m to start mining."
    );
    Ok(())
}

/// Interactive first-run/setup wizard. Writes a complete config to
/// `config_path` on success.
async fn run_wizard(config_path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;

    println!("\x1b[1;38;5;51m══ Sleepyminer Setup ══\x1b[0m");
    println!();
    println!("  Sleepyminer is a native RandomX (Monero) miner built for Apple Silicon.");
    println!("  It mines while your Mac is idle and automatically backs off when you");
    println!("  start using the machine, so you won't feel it running.");
    println!();
    println!("  Pick a path:");
    println!(
        "    \x1b[1;38;5;220m1)\x1b[0m \x1b[1mTraditional Monero\x1b[0m — paid in XMR directly to your Monero wallet."
    );
    println!(
        "    \x1b[1;38;5;220m2)\x1b[0m \x1b[1mNiceHash\x1b[0m           — paid in BTC to your NiceHash account."
    );
    println!();
    print!("  Choice [1]: ");
    std::io::stdout().flush()?;

    let mut choice = String::new();
    std::io::stdin().read_line(&mut choice)?;
    let is_nicehash = choice.trim() == "2";

    let (wallet, url, nicehash_flag, tls_flag, default_password) = if is_nicehash {
        println!();
        println!(
            "  NiceHash uses your BTC payout address as the \"wallet\" (append `.workername`"
        );
        println!("  to label this rig, e.g. \x1b[2m33nYY...CTgE.macmini\x1b[0m).");
        print!("  BTC address (with optional .worker): ");
        std::io::stdout().flush()?;
        let mut w = String::new();
        std::io::stdin().read_line(&mut w)?;
        let w = w.trim().to_string();
        if w.is_empty() {
            return Err("BTC address is required.".into());
        }
        (
            w,
            "randomxmonero.auto.nicehash.com:443".to_string(),
            true,
            true,
            "x".to_string(),
        )
    } else {
        println!();
        println!("  Enter your \x1b[1mMonero wallet address\x1b[0m (starts with 4 or 8).");
        print!("  Wallet: ");
        std::io::stdout().flush()?;
        let mut w = String::new();
        std::io::stdin().read_line(&mut w)?;
        let w = w.trim().to_string();
        if w.is_empty() {
            return Err("Monero wallet address is required.".into());
        }

        println!();
        println!("  Pick a pool (or enter your own):");
        println!(
            "    \x1b[1;38;5;220m1)\x1b[0m MoneroOcean  — gulf.moneroocean.stream:10128  (default)"
        );
        println!(
            "    \x1b[1;38;5;220m2)\x1b[0m C3Pool       — mine.c3pool.com:13333"
        );
        println!(
            "    \x1b[1;38;5;220m3)\x1b[0m SupportXMR   — pool.supportxmr.com:3333"
        );
        println!("    \x1b[1;38;5;220m4)\x1b[0m Custom (enter host:port)");
        print!("  Choice [1]: ");
        std::io::stdout().flush()?;

        let mut pool_choice = String::new();
        std::io::stdin().read_line(&mut pool_choice)?;
        let url = match pool_choice.trim() {
            "2" => "mine.c3pool.com:13333".to_string(),
            "3" => "pool.supportxmr.com:3333".to_string(),
            "4" => {
                print!("  Pool host:port: ");
                std::io::stdout().flush()?;
                let mut u = String::new();
                std::io::stdin().read_line(&mut u)?;
                let u = u.trim().to_string();
                if u.is_empty() {
                    return Err("Pool URL is required.".into());
                }
                u
            }
            _ => "gulf.moneroocean.stream:10128".to_string(),
        };
        (w, url, false, false, "sleepyminer".to_string())
    };

    println!();
    println!("  \x1b[1mThread strategy\x1b[0m:");
    println!("    \x1b[1;38;5;220m1)\x1b[0m Adaptive — scale with idle time (recommended, default)");
    println!("    \x1b[1;38;5;220m2)\x1b[0m Fixed    — always use N threads");
    print!("  Choice [1]: ");
    std::io::stdout().flush()?;

    let mut thread_choice = String::new();
    std::io::stdin().read_line(&mut thread_choice)?;
    let threads: Option<usize> = if thread_choice.trim() == "2" {
        let max = num_cpus::get();
        print!(
            "  Number of threads (1-{}) [{}]: ",
            max,
            max.saturating_sub(1).max(1)
        );
        std::io::stdout().flush()?;
        let mut n = String::new();
        std::io::stdin().read_line(&mut n)?;
        let parsed: usize = n
            .trim()
            .parse()
            .unwrap_or_else(|_| max.saturating_sub(1).max(1));
        Some(parsed.clamp(1, max))
    } else {
        None
    };

    let config = Config {
        pools: vec![PoolConfig {
            url,
            wallet,
            password: default_password,
            rig_id: None,
            nicehash: nicehash_flag,
            tls: tls_flag,
            keepalive: true,
            enabled: true,
            algo: algo::Algorithm::RandomX,
        }],
        threads,
        ..Default::default()
    };

    config.save(config_path)?;
    println!();
    println!(
        "  \x1b[1;38;5;46m✓\x1b[0m Config saved to \x1b[2m{}\x1b[0m",
        config_path.display()
    );

    // Celebrate!
    animation::shooting_star();

    Ok(())
}

async fn run_benchmark(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    print_banner();
    println!("  Sleepyminer Benchmark\n");

    let max_threads = cli.threads.unwrap_or_else(num_cpus::get);
    let seed_hash = [0u8; 32];

    let (optimal, _results) =
        benchmark::find_optimal_threads(&seed_hash, max_threads, Duration::from_secs(15))?;

    let config_path = cli.resolve_config_path();
    if config_path.exists() {
        let mut config = Config::load(&config_path)?;
        config.threads = Some(optimal);
        config.save(&config_path)?;
        println!("\nOptimal thread count ({}) saved to config.", optimal);
    }

    Ok(())
}
