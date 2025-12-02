use crate::metrics::MetricKind;
use crate::timeutils::{duration_from_std, parse_range};
use anyhow::{Context, Result};
use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use time::Duration as TimeDuration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub database: DatabaseConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub viewer: ViewerConfig,
    #[serde(default)]
    pub presets: HashMap<String, Preset>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database: DatabaseConfig::default(),
            daemon: DaemonConfig::default(),
            logging: LoggingConfig::default(),
            viewer: ViewerConfig::default(),
            presets: Preset::default_presets(),
        }
    }
}

impl Config {
    pub fn default_path() -> Result<PathBuf> {
        let dirs =
            ProjectDirs::from("dev", "wtui", "wtui").context("cannot locate config directory")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    pub fn load(path: Option<&Path>) -> Result<Self> {
        let path = path.map(PathBuf::from).unwrap_or_else(|| {
            Config::default_path().unwrap_or_else(|_| PathBuf::from("./config.toml"))
        });
        if path.exists() {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("reading config at {:?}", path))?;
            let mut cfg: Config = toml::from_str(&content).context("parsing config")?;
            cfg.expand_paths();
            Ok(cfg)
        } else {
            let mut cfg = Config::default();
            cfg.expand_paths();
            Ok(cfg)
        }
    }

    pub fn expand_paths(&mut self) {
        self.database.path = expand_tilde(&self.database.path);
        if let Some(file) = &self.logging.file {
            self.logging.file = Some(expand_tilde(file));
        }
        if let Some(pid) = &self.daemon.pid_file {
            self.daemon.pid_file = Some(expand_tilde(pid));
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub path: PathBuf,
    pub retention_days: Option<u32>,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("~/.local/share/wtui/data.db"),
            retention_days: Some(365),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "DaemonConfig::default_interval", with = "humantime_serde")]
    pub interval: Duration,
    #[serde(default = "DaemonConfig::default_metrics")]
    pub metrics: Vec<MetricKind>,
    #[serde(default)]
    pub disk_devices: Vec<String>,
    #[serde(default)]
    pub net_interfaces: Vec<String>,
    #[serde(default = "DaemonConfig::default_pid_file")]
    pub pid_file: Option<PathBuf>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            interval: Self::default_interval(),
            metrics: Self::default_metrics(),
            disk_devices: vec!["/".into()],
            net_interfaces: vec![],
            pid_file: Some(PathBuf::from("~/.local/state/wtui/wtui-daemon.pid")),
        }
    }
}

impl DaemonConfig {
    fn default_interval() -> Duration {
        Duration::from_secs(30)
    }

    fn default_pid_file() -> Option<PathBuf> {
        Some(PathBuf::from("~/.local/state/wtui/wtui-daemon.pid"))
    }

    fn default_metrics() -> Vec<MetricKind> {
        vec![
            MetricKind::Cpu,
            MetricKind::Ram,
            MetricKind::Net,
            MetricKind::Battery,
            MetricKind::Temps,
            MetricKind::Disk,
            MetricKind::Power,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "LoggingConfig::default_level")]
    pub level: String,
    pub file: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            file: Some(PathBuf::from("~/.local/state/wtui/daemon.log")),
        }
    }
}

impl LoggingConfig {
    fn default_level() -> String {
        "info".into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewerConfig {
    #[serde(default = "ViewerConfig::default_range", with = "humantime_serde")]
    pub default_range: Duration,
}

impl Default for ViewerConfig {
    fn default() -> Self {
        Self {
            default_range: ViewerConfig::default_range(),
        }
    }
}

impl ViewerConfig {
    fn default_range() -> Duration {
        Duration::from_secs(3600)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresetKind {
    Chart,
    Report,
    Aggregate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub kind: PresetKind,
    #[serde(default)]
    pub metrics: Vec<String>,
    #[serde(default)]
    pub metric: Option<String>,
    #[serde(default)]
    pub group_by: Option<String>,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub csv: Option<bool>,
}

impl Preset {
    pub fn default_presets() -> HashMap<String, Preset> {
        let mut map = HashMap::new();
        map.insert(
            "battery_day".into(),
            Preset {
                kind: PresetKind::Chart,
                metrics: vec!["battery_capacity".into()],
                metric: None,
                group_by: None,
                range: Some("1d".into()),
                csv: Some(false),
            },
        );
        map.insert(
            "battery_year".into(),
            Preset {
                kind: PresetKind::Chart,
                metrics: vec!["battery_health".into()],
                metric: None,
                group_by: None,
                range: Some("365d".into()),
                csv: Some(false),
            },
        );
        map.insert(
            "cpu_gpu_hour".into(),
            Preset {
                kind: PresetKind::Report,
                metrics: vec!["cpu_temp".into(), "gpu_temp".into()],
                metric: None,
                group_by: None,
                range: Some("1h".into()),
                csv: Some(true),
            },
        );
        map.insert(
            "net_week".into(),
            Preset {
                kind: PresetKind::Aggregate,
                metrics: vec![],
                metric: Some("net_bytes".into()),
                group_by: Some("day".into()),
                range: Some("7d".into()),
                csv: Some(true),
            },
        );
        map.insert(
            "disk_year".into(),
            Preset {
                kind: PresetKind::Chart,
                metrics: vec!["disk_usage".into()],
                metric: None,
                group_by: None,
                range: Some("365d".into()),
                csv: Some(false),
            },
        );
        map
    }

    pub fn range(&self, default: Duration) -> Result<TimeDuration> {
        if let Some(r) = &self.range {
            Ok(parse_range(r)?)
        } else {
            Ok(duration_from_std(default))
        }
    }
}

fn expand_tilde(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    if !path_str.starts_with('~') {
        return path.to_path_buf();
    }

    let home = BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    if path_str == "~" {
        home
    } else {
        let mut expanded = home;
        expanded.push(path_str.trim_start_matches("~/"));
        expanded
    }
}
