use anyhow::{Context, Result};
use clap::Parser;
use once_cell::sync::OnceCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tracing::{info, warn};
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::util::SubscriberInitExt;
use wtui_core::metrics::{
    cpu_usage_percent, read_batteries, read_cpu_times, read_disk_usage, read_net_snapshot,
    read_powercap, read_ram_usage, read_temperatures, CpuTimes, MetricKind, NetSnapshot,
};
use wtui_core::{Config, Database};

#[derive(Parser, Debug)]
#[command(author, version, about = "wtui-daemon: metrics collector")]
struct Args {
    /// Path to config TOML
    #[arg(long)]
    config: Option<PathBuf>,
    /// Override database path
    #[arg(long)]
    db: Option<PathBuf>,
    /// Override polling interval
    #[arg(long)]
    interval: Option<humantime::Duration>,
    /// Comma separated metrics list
    #[arg(long)]
    metrics: Option<String>,
}

struct DaemonState {
    prev_cpu: Option<CpuTimes>,
    prev_net: HashMap<String, NetSnapshot>,
    last_retention: Instant,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut config = Config::load(args.config.as_deref())?;
    apply_overrides(&mut config, &args);

    init_logging(&config)?;
    info!("starting wtui-daemon");

    let db = Database::connect(&config.database.path)?;
    let mut state = DaemonState {
        prev_cpu: None,
        prev_net: HashMap::new(),
        last_retention: Instant::now(),
    };

    let running = Arc::new(AtomicBool::new(true));
    let reload = Arc::new(AtomicBool::new(false));
    setup_signals(running.clone(), reload.clone());

    let interval = config.daemon.interval;
    let pid_guard = PidGuard::new(config.daemon.pid_file.clone())?;

    while running.load(Ordering::SeqCst) {
        if reload.swap(false, Ordering::SeqCst) {
            info!("reloading config");
            match Config::load(args.config.as_deref()) {
                Ok(mut new_cfg) => {
                    apply_overrides(&mut new_cfg, &args);
                    config = new_cfg;
                }
                Err(err) => warn!("failed to reload config: {err}",),
            }
        }

        let now = wtui_core::timeutils::now_utc();
        collect_cycle(&db, &config, &mut state, now);

        if let Some(days) = config.database.retention_days {
            if state.last_retention.elapsed() > Duration::from_secs(600) {
                let cutoff = now - time::Duration::days(days as i64);
                if let Err(err) = db.prune_older_than(cutoff) {
                    warn!("retention prune failed: {err}");
                }
                state.last_retention = Instant::now();
            }
        }

        thread::sleep(interval);
    }

    drop(pid_guard);
    info!("wtui-daemon stopped");
    Ok(())
}

fn collect_cycle(
    db: &Database,
    config: &Config,
    state: &mut DaemonState,
    now: time::OffsetDateTime,
) {
    let metrics = &config.daemon.metrics;

    if metrics.contains(&MetricKind::Cpu) {
        match read_cpu_times() {
            Ok(current) => {
                if let Some(prev) = &state.prev_cpu {
                    if let Some(usage) = cpu_usage_percent(prev, &current) {
                        if let Err(err) = db.insert_cpu_usage(now, usage, Some("total")) {
                            warn!("failed to write cpu sample: {err}");
                        }
                    }
                }
                state.prev_cpu = Some(current);
            }
            Err(err) => warn!("cpu read failed: {err}"),
        }
    }

    if metrics.contains(&MetricKind::Ram) {
        match read_ram_usage() {
            Ok(ram) => {
                let used = ram.total_bytes.saturating_sub(ram.available_bytes);
                if let Err(err) = db.insert_ram_usage(now, used, ram.total_bytes) {
                    warn!("failed to write ram sample: {err}");
                }
            }
            Err(err) => warn!("ram read failed: {err}"),
        }
    }

    if metrics.contains(&MetricKind::Net) {
        let interfaces = desired_interfaces(&config.daemon.net_interfaces);
        for iface in interfaces {
            match read_net_snapshot(&iface) {
                Ok(snapshot) => {
                    let prev = state.prev_net.insert(iface.clone(), snapshot);
                    let mut delta = None;
                    let mut reset = false;
                    if let Some(prev) = prev {
                        let rx_delta = snapshot.rx_bytes as i64 - prev.rx_bytes as i64;
                        let tx_delta = snapshot.tx_bytes as i64 - prev.tx_bytes as i64;
                        if rx_delta < 0 || tx_delta < 0 {
                            reset = true;
                        } else {
                            delta = Some((rx_delta, tx_delta));
                        }
                    }
                    if let Err(err) = db.insert_net_sample(now, &iface, snapshot, delta, reset) {
                        warn!("failed to write net sample for {iface}: {err}");
                    }
                }
                Err(err) => warn!("net read failed for {iface}: {err}"),
            }
        }
    }

    if metrics.contains(&MetricKind::Battery) {
        match read_batteries() {
            Ok(batteries) => {
                for b in batteries {
                    if let Err(err) = db.insert_battery_sample(
                        now,
                        &b.name,
                        b.capacity,
                        b.health,
                        b.energy_now_uw,
                    ) {
                        warn!("failed to write battery sample: {err}");
                    }
                }
            }
            Err(err) => warn!("battery read failed: {err}"),
        }
    }

    if metrics.contains(&MetricKind::Temps) {
        match read_temperatures() {
            Ok(temps) => {
                for t in temps {
                    if let Err(err) = db.insert_temp_sample(now, &t.sensor, t.value_c) {
                        warn!("failed to write temp sample: {err}");
                    }
                }
            }
            Err(err) => warn!("temp read failed: {err}"),
        }
    }

    if metrics.contains(&MetricKind::Disk) {
        let mounts = if config.daemon.disk_devices.is_empty() {
            vec!["/".into()]
        } else {
            config.daemon.disk_devices.clone()
        };
        for mount in mounts {
            match read_disk_usage(&mount) {
                Ok(usage) => {
                    let used = usage.total_bytes.saturating_sub(usage.available_bytes);
                    if let Err(err) = db.insert_disk_sample(now, &mount, used, usage.total_bytes) {
                        warn!("failed to write disk sample for {mount}: {err}");
                    }
                }
                Err(err) => warn!("disk read failed for {mount}: {err}"),
            }
        }
    }

    if metrics.contains(&MetricKind::Power) {
        match read_powercap() {
            Ok(domains) => {
                for d in domains {
                    if let Err(err) = db.insert_power_sample(now, &d.domain, d.draw_mw) {
                        warn!("failed to write power sample: {err}");
                    }
                }
            }
            Err(err) => warn!("power read failed: {err}"),
        }
    }
}

fn apply_overrides(config: &mut Config, args: &Args) {
    if let Some(db) = &args.db {
        config.database.path = db.clone();
    }
    if let Some(interval) = args.interval {
        config.daemon.interval = *interval;
    }
    if let Some(metrics) = &args.metrics {
        let kinds: Vec<MetricKind> = metrics
            .split(',')
            .filter_map(|m| m.trim().parse().ok())
            .collect();
        if !kinds.is_empty() {
            config.daemon.metrics = kinds;
        }
    }
}

fn desired_interfaces(user: &[String]) -> Vec<String> {
    if !user.is_empty() {
        return user.to_vec();
    }
    let base = std::path::PathBuf::from("/sys/class/net");
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "lo" {
                continue;
            }
            found.push(name);
        }
    }
    if found.is_empty() {
        vec!["eth0".into()]
    } else {
        found
    }
}

fn setup_signals(running: Arc<AtomicBool>, reload: Arc<AtomicBool>) {
    let r1 = running.clone();
    ctrlc::set_handler(move || {
        r1.store(false, Ordering::SeqCst);
    })
    .expect("failed to set ctrlc handler");

    let r2 = running.clone();
    let reload_flag = reload.clone();
    let _ = signal_hook::flag::register(signal_hook::consts::SIGHUP, reload_flag);
    let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, r2.clone());
}

fn init_logging(config: &Config) -> Result<()> {
    let writer: BoxMakeWriter = if let Some(path) = &config.logging.file {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("opening log file at {:?}", path))?;
        let (writer, guard) = tracing_appender::non_blocking(file);
        static LOG_GUARD: OnceCell<tracing_appender::non_blocking::WorkerGuard> = OnceCell::new();
        let _ = LOG_GUARD.set(guard);
        BoxMakeWriter::new(writer)
    } else {
        BoxMakeWriter::new(std::io::stderr)
    };

    tracing_subscriber::fmt()
        .with_env_filter(config.logging.level.clone())
        .with_ansi(atty::is(atty::Stream::Stderr))
        .with_target(false)
        .with_thread_ids(false)
        .with_level(true)
        .with_writer(writer)
        .finish()
        .try_init()
        .ok();
    Ok(())
}

struct PidGuard {
    path: Option<PathBuf>,
}

impl PidGuard {
    fn new(path: Option<PathBuf>) -> Result<Self> {
        if let Some(path) = &path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if let Ok(pid_str) = std::fs::read_to_string(path) {
                if let Ok(pid) = pid_str.trim().parse::<u32>() {
                    if std::path::Path::new(&format!("/proc/{pid}")).exists() {
                        anyhow::bail!("another wtui-daemon seems to be running with pid {pid}");
                    }
                }
            }
            std::fs::write(path, format!("{}\n", std::process::id()))?;
        }
        Ok(Self { path })
    }
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
    }
}
