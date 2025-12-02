use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use crossterm::{execute, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table};
use ratatui::Terminal;
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use wtui_core::config::{Config, Preset};
use wtui_core::metrics::{
    cpu_usage_percent, read_batteries, read_cpu_times, read_disk_usage, read_net_snapshot,
    read_powercap, read_ram_usage, read_temperatures, NetSnapshot,
};
use wtui_core::timeutils::{duration_from_std, duration_to_std};
use wtui_core::{parse_range, Database, MetricPoint, MetricSeries, RangeSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Live,
    Historical,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "wtui viewer")]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    db: Option<PathBuf>,
    #[arg(long)]
    range: Option<String>,
    #[arg(long)]
    charts: Option<String>,
    #[arg(long)]
    preset: Option<String>,
    #[arg(long, default_value = "historical")]
    mode: String,
    #[arg(long)]
    csv: bool,
}

struct App {
    config: Config,
    db: Option<Database>,
    mode: Mode,
    range: Duration,
    metrics: Vec<String>,
    presets: Vec<(String, Preset)>,
    selected_preset: usize,
    series: Vec<MetricSeries>,
    status: String,
    filter: String,
    filter_mode: bool,
    live_cpu_prev: Option<wtui_core::metrics::CpuTimes>,
    live_net_prev: HashMap<String, NetSnapshot>,
}

impl App {
    fn new(config: Config, args: &Args) -> Result<Self> {
        let db_path = args
            .db
            .clone()
            .unwrap_or_else(|| config.database.path.clone());
        let db = if db_path.exists() {
            Some(Database::connect(&db_path)?)
        } else {
            None
        };
        let mut metrics: Vec<String> = Vec::new();
        if let Some(charts) = &args.charts {
            metrics = charts.split(',').map(|s| s.trim().to_string()).collect();
        }
        let presets: Vec<(String, Preset)> = config
            .presets
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let range = if let Some(r) = &args.range {
            duration_to_std(parse_range(r)?)
        } else {
            config.viewer.default_range
        };
        let mode = if args.mode.to_ascii_lowercase().starts_with('l') {
            Mode::Live
        } else {
            Mode::Historical
        };

        let mut app = Self {
            config,
            db,
            mode,
            range,
            metrics,
            presets,
            selected_preset: 0,
            series: Vec::new(),
            status: String::from("Press q to quit, arrows to choose presets, Enter to apply"),
            filter: String::new(),
            filter_mode: false,
            live_cpu_prev: None,
            live_net_prev: HashMap::new(),
        };

        if let Some(name) = &args.preset {
            app.apply_preset(name);
        }
        if app.metrics.is_empty() {
            app.metrics = vec!["cpu".into(), "ram".into()];
        }
        Ok(app)
    }

    fn apply_preset(&mut self, name: &str) {
        if let Some((idx, preset)) = self
            .presets
            .iter()
            .enumerate()
            .find(|(_, (k, _))| k == name)
        {
            self.selected_preset = idx;
            let preset = &preset.1;
            self.metrics = if !preset.metrics.is_empty() {
                preset.metrics.clone()
            } else if let Some(metric) = &preset.metric {
                vec![metric.clone()]
            } else {
                self.metrics.clone()
            };
            if let Some(range) = &preset.range {
                if let Ok(dur) = parse_range(range) {
                    self.range = duration_to_std(dur);
                }
            }
        }
    }

    fn refresh(&mut self) {
        let range_spec = RangeSpec::ending_now(duration_from_std(self.range));
        let result = match self.mode {
            Mode::Historical => self.load_from_db(range_spec),
            Mode::Live => self.load_live(),
        };
        if let Err(err) = result {
            self.status = format!("error: {err}");
        }
    }

    fn load_from_db(&mut self, range: RangeSpec) -> Result<()> {
        let db = self
            .db
            .as_ref()
            .context("no database available for historical mode")?;
        let mut series = Vec::new();
        for metric in &self.metrics {
            if metric == "net_bytes" {
                let rows = db.aggregate_net(range.since, "day")?;
                let mut s = MetricSeries::new("net_bytes", Some("bytes"));
                for row in rows {
                    s.push(MetricPoint {
                        timestamp: row.timestamp,
                        value: row.value,
                        label: row.label,
                    });
                }
                series.push(s);
                continue;
            }
            if let Some(table) = table_for_metric(metric) {
                let rows = db.fetch_series(table, range.since)?;
                let mut s = MetricSeries::new(metric, None);
                for row in rows {
                    s.push(MetricPoint {
                        timestamp: row.timestamp,
                        value: row.value,
                        label: row.label,
                    });
                }
                series.push(s);
            }
        }
        self.series = series;
        Ok(())
    }

    fn load_live(&mut self) -> Result<()> {
        let now = OffsetDateTime::now_utc();
        let mut series = Vec::new();
        for metric in &self.metrics {
            match metric.as_str() {
                "cpu" => {
                    if let Some(point) = live_cpu_sample(&mut self.live_cpu_prev)? {
                        let mut s = MetricSeries::new("cpu", Some("%"));
                        s.push(point);
                        series.push(s);
                    }
                }
                "ram" => {
                    if let Ok(ram) = read_ram_usage() {
                        let used = ram.total_bytes.saturating_sub(ram.available_bytes) as f64;
                        let pct = if ram.total_bytes > 0 {
                            used / ram.total_bytes as f64 * 100.0
                        } else {
                            0.0
                        };
                        let mut s = MetricSeries::new("ram", Some("%"));
                        s.push(MetricPoint {
                            timestamp: now,
                            value: pct,
                            label: None,
                        });
                        series.push(s);
                    }
                }
                "net" | "net_bytes" => {
                    let mut s = MetricSeries::new("net", Some("bytes/s"));
                    for iface in desired_interfaces(&self.config.daemon.net_interfaces) {
                        if let Ok(snapshot) = read_net_snapshot(&iface) {
                            let prev = self.live_net_prev.insert(iface.clone(), snapshot);
                            if let Some(prev) = prev {
                                let rx = snapshot.rx_bytes.saturating_sub(prev.rx_bytes);
                                let tx = snapshot.tx_bytes.saturating_sub(prev.tx_bytes);
                                let delta = rx + tx;
                                s.push(MetricPoint {
                                    timestamp: now,
                                    value: delta as f64,
                                    label: Some(iface.clone()),
                                });
                            }
                        }
                    }
                    if !s.points.is_empty() {
                        series.push(s);
                    }
                }
                m if m.starts_with("battery") => {
                    if let Ok(batts) = read_batteries() {
                        let mut s = MetricSeries::new("battery", Some("%"));
                        for b in batts {
                            if let Some(cap) = b.capacity {
                                s.push(MetricPoint {
                                    timestamp: now,
                                    value: cap,
                                    label: Some(b.name.clone()),
                                });
                            }
                        }
                        if !s.points.is_empty() {
                            series.push(s);
                        }
                    }
                }
                m if m.contains("temp") || m == "temps" => {
                    if let Ok(temps) = read_temperatures() {
                        let mut s = MetricSeries::new("temps", Some("C"));
                        for t in temps {
                            s.push(MetricPoint {
                                timestamp: now,
                                value: t.value_c,
                                label: Some(t.sensor.clone()),
                            });
                        }
                        if !s.points.is_empty() {
                            series.push(s);
                        }
                    }
                }
                m if m.contains("disk") => {
                    let mounts = if self.config.daemon.disk_devices.is_empty() {
                        vec!["/".into()]
                    } else {
                        self.config.daemon.disk_devices.clone()
                    };
                    let mut s = MetricSeries::new("disk", Some("%"));
                    for mount in mounts {
                        if let Ok(usage) = read_disk_usage(&mount) {
                            let used = usage.total_bytes.saturating_sub(usage.available_bytes);
                            let pct = if usage.total_bytes > 0 {
                                used as f64 / usage.total_bytes as f64 * 100.0
                            } else {
                                0.0
                            };
                            s.push(MetricPoint {
                                timestamp: now,
                                value: pct,
                                label: Some(mount.clone()),
                            });
                        }
                    }
                    if !s.points.is_empty() {
                        series.push(s);
                    }
                }
                m if m.contains("power") => {
                    if let Ok(power) = read_powercap() {
                        let mut s = MetricSeries::new("power", Some("mW"));
                        for p in power {
                            s.push(MetricPoint {
                                timestamp: now,
                                value: p.draw_mw,
                                label: Some(p.domain),
                            });
                        }
                        if !s.points.is_empty() {
                            series.push(s);
                        }
                    }
                }
                _ => {}
            }
        }
        self.series = series;
        Ok(())
    }

    fn export_csv<W: Write>(&self, mut writer: W) -> Result<()> {
        let mut csv_writer = csv::Writer::from_writer(&mut writer);
        csv_writer.write_record(["metric", "label", "timestamp", "value"])?;
        for s in &self.series {
            for p in &s.points {
                csv_writer.write_record([
                    &s.name,
                    p.label.as_deref().unwrap_or(""),
                    &p.timestamp.unix_timestamp().to_string(),
                    &format!("{:.2}", p.value),
                ])?;
            }
        }
        csv_writer.flush()?;
        Ok(())
    }
}

fn table_for_metric(metric: &str) -> Option<&str> {
    match metric {
        "cpu" | "cpu_usage" => Some("cpu_samples"),
        "ram" | "ram_usage" => Some("ram_samples"),
        "net" | "net_bytes" => Some("net_samples"),
        m if m.starts_with("battery") => Some("battery_samples"),
        m if m.contains("temp") || m == "temps" => Some("temp_samples"),
        m if m.contains("disk") => Some("disk_samples"),
        m if m.contains("power") => Some("power_samples"),
        _ => None,
    }
}

fn live_cpu_sample(prev: &mut Option<wtui_core::metrics::CpuTimes>) -> Result<Option<MetricPoint>> {
    let current = read_cpu_times()?;
    let now = OffsetDateTime::now_utc();
    if let Some(prev_times) = prev {
        if let Some(usage) = cpu_usage_percent(prev_times, &current) {
            *prev = Some(current);
            return Ok(Some(MetricPoint {
                timestamp: now,
                value: usage,
                label: Some("total".into()),
            }));
        }
    }
    *prev = Some(current);
    Ok(None)
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

fn draw_ui(frame: &mut ratatui::Frame<'_>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(frame.size());

    // Header
    let header_text = format!(
        "Mode: {:?} | Range: {:?} | Metrics: {}",
        app.mode,
        humantime::format_duration(app.range),
        app.metrics.join(",")
    );
    let header =
        Paragraph::new(header_text).block(Block::default().borders(Borders::ALL).title("wtui"));
    frame.render_widget(header, chunks[0]);

    // Body layout
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(chunks[1]);

    let preset_items: Vec<ListItem> = app
        .presets
        .iter()
        .enumerate()
        .filter(|(_, (name, _))| app.filter.is_empty() || name.contains(&app.filter))
        .map(|(idx, (name, _))| {
            let mut item = ListItem::new(name.clone());
            if idx == app.selected_preset {
                item = item.style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                );
            }
            item
        })
        .collect();
    let presets = List::new(preset_items)
        .block(Block::default().borders(Borders::ALL).title("Presets"))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    frame.render_widget(presets, body[0]);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(body[1]);

    let mut rows = Vec::new();
    for s in &app.series {
        if let Some(last) = s.points.last() {
            rows.push(Row::new(vec![
                s.name.clone(),
                last.label.clone().unwrap_or_else(|| "-".into()),
                format!("{:.2}", last.value),
                last.timestamp
                    .format(&time::macros::format_description!("%H:%M:%S"))
                    .unwrap_or_else(|_| "".into()),
            ]));
        }
    }
    if rows.is_empty() {
        rows.push(Row::new(vec![
            Cell::from("no data"),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
        ]));
    }

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(16),
            Constraint::Length(12),
            Constraint::Length(12),
        ],
    )
    .header(
        Row::new(vec!["Metric", "Label", "Value", "Time"])
            .style(Style::default().fg(Color::Yellow)),
    )
    .block(Block::default().borders(Borders::ALL).title("Data"));
    frame.render_widget(table, right_chunks[0]);

    let footer = Paragraph::new(app.status.clone())
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .style(Style::default().fg(Color::White));
    frame.render_widget(footer, chunks[2]);
}

fn run_tui(mut app: App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let tick_rate = Duration::from_millis(1000);
    let mut last_tick = Instant::now();
    app.refresh();

    loop {
        terminal.draw(|f| draw_ui(f, &app))?;
        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if app.filter_mode {
                        match key.code {
                            KeyCode::Enter => app.filter_mode = false,
                            KeyCode::Char(c) => app.filter.push(c),
                            KeyCode::Backspace => {
                                app.filter.pop();
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Down => {
                            if app.selected_preset + 1 < app.presets.len() {
                                app.selected_preset += 1;
                            }
                        }
                        KeyCode::Up => {
                            if app.selected_preset > 0 {
                                app.selected_preset -= 1;
                            }
                        }
                        KeyCode::Enter => {
                            if let Some((name, _)) = app.presets.get(app.selected_preset).cloned() {
                                app.apply_preset(&name);
                                app.status = format!("applied preset {name}");
                                app.refresh();
                            }
                        }
                        KeyCode::Char('l') => {
                            app.mode = Mode::Live;
                            app.status = "live mode".into();
                        }
                        KeyCode::Char('h') => {
                            app.mode = Mode::Historical;
                            app.status = "historical mode".into();
                        }
                        KeyCode::Char('/') => {
                            app.filter_mode = true;
                            app.filter.clear();
                            app.status = "filter: type text and press Enter".into();
                        }
                        KeyCode::Char('c') => {
                            let path = "wtui-export.csv";
                            if let Ok(file) = std::fs::File::create(path) {
                                if let Err(err) = app.export_csv(file) {
                                    app.status = format!("csv export failed: {err}");
                                } else {
                                    app.status = format!("csv exported to {path}");
                                }
                            } else {
                                app.status = "unable to write csv".into();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.refresh();
            last_tick = Instant::now();
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), terminal::LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = Config::load(args.config.as_deref())?;
    let mut app = App::new(config, &args)?;
    app.refresh();

    if args.csv {
        let stdout = io::stdout();
        let handle = stdout.lock();
        app.export_csv(handle)?;
        return Ok(());
    }

    run_tui(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_mapping_works() {
        assert_eq!(table_for_metric("cpu"), Some("cpu_samples"));
        assert_eq!(table_for_metric("ram"), Some("ram_samples"));
        assert_eq!(table_for_metric("net_bytes"), Some("net_samples"));
        assert_eq!(
            table_for_metric("battery_capacity"),
            Some("battery_samples")
        );
    }

    #[test]
    fn csv_export_writes_rows() {
        let config = Config::default();
        let app = App {
            config,
            db: None,
            mode: Mode::Live,
            range: Duration::from_secs(60),
            metrics: vec!["cpu".into()],
            presets: Vec::new(),
            selected_preset: 0,
            series: vec![{
                let mut s = MetricSeries::new("cpu", Some("%"));
                s.push(MetricPoint {
                    timestamp: OffsetDateTime::now_utc(),
                    value: 12.3,
                    label: Some("total".into()),
                });
                s
            }],
            status: String::new(),
            filter: String::new(),
            filter_mode: false,
            live_cpu_prev: None,
            live_net_prev: HashMap::new(),
        };

        let mut buf = Vec::new();
        app.export_csv(&mut buf).unwrap();
        let content = String::from_utf8(buf).unwrap();
        assert!(content.contains("cpu"));
        assert!(content.contains("12.3"));
    }
}
