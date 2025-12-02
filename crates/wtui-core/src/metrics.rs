use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use time::OffsetDateTime;

use crate::timeutils::now_utc;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum MetricKind {
    Cpu,
    Ram,
    Net,
    Battery,
    Temps,
    Disk,
    Power,
}

impl FromStr for MetricKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "cpu" => Ok(MetricKind::Cpu),
            "ram" => Ok(MetricKind::Ram),
            "net" => Ok(MetricKind::Net),
            "battery" => Ok(MetricKind::Battery),
            "temps" | "temp" | "temperature" => Ok(MetricKind::Temps),
            "disk" => Ok(MetricKind::Disk),
            "power" => Ok(MetricKind::Power),
            _ => anyhow::bail!("unknown metric kind: {s}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MetricReading {
    pub timestamp: OffsetDateTime,
    pub label: String,
    pub value: f64,
    pub unit: &'static str,
    pub kind: MetricKind,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuTimes {
    pub fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    pub fn idle_total(&self) -> u64 {
        self.idle + self.iowait
    }
}

pub fn read_cpu_times() -> Result<CpuTimes> {
    let file = fs::File::open("/proc/stat").context("opening /proc/stat")?;
    let mut lines = io::BufReader::new(file).lines();
    if let Some(Ok(first)) = lines.next() {
        let parts: Vec<&str> = first.split_whitespace().collect();
        if parts.len() < 8 {
            anyhow::bail!("unexpected /proc/stat format");
        }
        let nums: Vec<u64> = parts[1..]
            .iter()
            .take(8)
            .map(|v| v.parse::<u64>().unwrap_or(0))
            .collect();
        Ok(CpuTimes {
            user: nums[0],
            nice: nums[1],
            system: nums[2],
            idle: nums[3],
            iowait: nums[4],
            irq: nums[5],
            softirq: nums[6],
            steal: nums[7],
        })
    } else {
        anyhow::bail!("no contents in /proc/stat")
    }
}

pub fn cpu_usage_percent(prev: &CpuTimes, current: &CpuTimes) -> Option<f64> {
    let prev_idle = prev.idle_total();
    let idle = current.idle_total();
    let prev_total = prev.total();
    let total = current.total();
    let totald = total.checked_sub(prev_total)?;
    let idled = idle.checked_sub(prev_idle)?;
    if totald == 0 {
        return None;
    }
    let usage = (totald.saturating_sub(idled)) as f64 / totald as f64 * 100.0;
    Some(usage)
}

#[derive(Debug, Clone, Copy)]
pub struct RamUsage {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

pub fn read_ram_usage() -> Result<RamUsage> {
    let content = fs::read_to_string("/proc/meminfo").context("reading /proc/meminfo")?;
    let mut total = 0u64;
    let mut available = 0u64;
    for line in content.lines() {
        if line.starts_with("MemTotal:") {
            total = parse_kib_value(line)? * 1024;
        } else if line.starts_with("MemAvailable:") {
            available = parse_kib_value(line)? * 1024;
        }
    }
    if total == 0 {
        anyhow::bail!("missing MemTotal in /proc/meminfo")
    }
    if available == 0 {
        available = total;
    }
    Ok(RamUsage {
        total_bytes: total,
        available_bytes: available,
    })
}

fn parse_kib_value(line: &str) -> Result<u64> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    parts
        .get(1)
        .context("no numeric value")?
        .parse::<u64>()
        .map_err(|e| e.into())
}

#[derive(Debug, Clone, Copy)]
pub struct NetSnapshot {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

pub fn read_net_snapshot(interface: &str) -> Result<NetSnapshot> {
    let rx_path = format!("/sys/class/net/{interface}/statistics/rx_bytes");
    let tx_path = format!("/sys/class/net/{interface}/statistics/tx_bytes");
    let rx = fs::read_to_string(&rx_path)
        .with_context(|| format!("reading {rx_path}"))?
        .trim()
        .parse::<u64>()?;
    let tx = fs::read_to_string(&tx_path)
        .with_context(|| format!("reading {tx_path}"))?
        .trim()
        .parse::<u64>()?;
    Ok(NetSnapshot {
        rx_bytes: rx,
        tx_bytes: tx,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct DiskUsage {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

pub fn read_disk_usage<P: AsRef<Path>>(path: P) -> Result<DiskUsage> {
    let stats = nix::sys::statvfs::statvfs(path.as_ref())?;
    let total = stats.blocks() * stats.block_size();
    let avail = stats.blocks_available() * stats.block_size();
    Ok(DiskUsage {
        total_bytes: total,
        available_bytes: avail,
    })
}

#[derive(Debug, Clone)]
pub struct TempReading {
    pub sensor: String,
    pub value_c: f64,
}

pub fn read_temperatures() -> Result<Vec<TempReading>> {
    let mut readings = Vec::new();
    let hwmon_root = PathBuf::from("/sys/class/hwmon");
    if !hwmon_root.exists() {
        return Ok(readings);
    }

    for entry in fs::read_dir(hwmon_root)? {
        let entry = entry?;
        let path = entry.path();
        let name = fs::read_to_string(path.join("name")).unwrap_or_else(|_| "hwmon".into());
        for file in fs::read_dir(&path)? {
            let file = file?;
            let fname = file.file_name();
            let fname_str = fname.to_string_lossy();
            if fname_str.starts_with("temp") && fname_str.ends_with("_input") {
                let label_path = path.join(fname_str.replace("_input", "_label"));
                let label = fs::read_to_string(&label_path)
                    .unwrap_or_else(|_| fname_str.replace("_input", ""));
                let raw = fs::read_to_string(file.path())?.trim().to_string();
                if let Ok(value) = raw.parse::<f64>() {
                    let mut c = value;
                    if c > 1000.0 {
                        c = c / 1000.0;
                    }
                    readings.push(TempReading {
                        sensor: format!("{}:{}", name.trim(), label.trim()),
                        value_c: c,
                    });
                }
            }
        }
    }
    Ok(readings)
}

#[derive(Debug, Clone)]
pub struct BatteryReading {
    pub name: String,
    pub capacity: Option<f64>,
    pub health: Option<f64>,
    pub energy_now_uw: Option<f64>,
}

pub fn read_batteries() -> Result<Vec<BatteryReading>> {
    let mut readings = Vec::new();
    let base = PathBuf::from("/sys/class/power_supply");
    if !base.exists() {
        return Ok(readings);
    }

    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let path = entry.path();
        let ty = fs::read_to_string(path.join("type")).unwrap_or_default();
        if !ty.to_lowercase().contains("battery") {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let capacity = read_f64(path.join("capacity"));
        let health = read_health_percent(&path);
        let energy_now_uw = read_power_now(&path).or_else(|| read_current_voltage_power(&path));
        readings.push(BatteryReading {
            name,
            capacity,
            health,
            energy_now_uw,
        });
    }
    Ok(readings)
}

fn read_health_percent(path: &Path) -> Option<f64> {
    if let Some(value) = read_f64(path.join("health")) {
        return Some(value);
    }
    let full = read_f64(path.join("energy_full"));
    let design = read_f64(path.join("energy_full_design"));
    match (full, design) {
        (Some(f), Some(d)) if d > 0.0 => Some(f / d * 100.0),
        _ => None,
    }
}

fn read_power_now(path: &Path) -> Option<f64> {
    let p_now = read_f64(path.join("power_now"));
    p_now.map(|p| p / 1000.0)
}

fn read_current_voltage_power(path: &Path) -> Option<f64> {
    let current = read_f64(path.join("current_now"));
    let voltage = read_f64(path.join("voltage_now"));
    match (current, voltage) {
        (Some(c), Some(v)) => Some(c * v / 1_000_000.0),
        _ => None,
    }
}

fn read_f64<P: AsRef<Path>>(path: P) -> Option<f64> {
    let content = fs::read_to_string(path).ok()?;
    content.trim().parse::<f64>().ok()
}

#[derive(Debug, Clone)]
pub struct PowerReading {
    pub domain: String,
    pub draw_mw: f64,
}

pub fn read_powercap() -> Result<Vec<PowerReading>> {
    let mut readings = Vec::new();
    let root = PathBuf::from("/sys/class/powercap");
    if !root.exists() {
        return Ok(readings);
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let name = fs::read_to_string(path.join("name"))
            .unwrap_or_else(|_| entry.file_name().to_string_lossy().to_string());
        let power = read_f64(path.join("power_uw"))
            .or_else(|| read_f64(path.join("energy_uj")))
            .or_else(|| read_f64(path.join("max_energy_range_uj")));
        if let Some(p) = power {
            readings.push(PowerReading {
                domain: name.trim().into(),
                draw_mw: p / 1000.0,
            });
        }
    }
    Ok(readings)
}

pub fn now() -> OffsetDateTime {
    now_utc()
}
