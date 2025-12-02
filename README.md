# wtui

wtui is a Rust TUI monitoring tool with two binaries: a daemon that collects system metrics into SQLite, and a viewer that queries and visualizes them via charts and reports (with CSV export).

## What it does

### Daemon (wtui-daemon)
- Collects temperatures, battery capacity/health, power draw, disk usage, CPU usage, RAM usage, and network throughput.
- Polls at a configurable interval and writes to SQLite.
- Configurable: which metrics to collect, interval, DB path, retention, included disks/interfaces/sensors.

### Viewer (wtui)
- Reads live data (directly from kernel counters) or historical data (from SQLite).
- Two report modes: charts and tabular reports (raw or aggregated), optionally exported as CSV.
- Presets defined in the config for quick recall of common views.

Example preset ideas:
- Battery capacity over the last day (chart)
- Battery health over the last year (chart)
- CPU and GPU temperature over the last hour (report, CSV)
- Total data downloaded/uploaded per day over the last week (aggregate)
- Disk usage trend over the last year (chart)

## Installation

- NixOS: `nix run .#wtui` and `nix run .#wtui-daemon` (flake outputs).
- Non-Nix: `cargo install --path crates/wtui-daemon` and `cargo install --path crates/wtui`.

Workspace layout:
- `crates/wtui-core`: shared config, SQLite schema/migrations, metric readers, time utilities.
- `crates/wtui-daemon`: headless sampler that writes to SQLite, handles SIGHUP reloads, PID guard, retention pruning.
- `crates/wtui`: TUI viewer with CSV export and preset picker.

Local development shell:
```
nix develop
```
(or use your system Rust toolchain)

## Usage

### Daemon
- Defaults: `wtui-daemon --config ~/.config/wtui/config.toml`
- Custom DB + interval: `wtui-daemon --db ~/.local/share/wtui/data.db --interval 30s`
- Select metrics: `wtui-daemon --metrics cpu,ram,net`

### Viewer
- Live view last hour CPU/RAM: `wtui --range 1h --charts cpu,ram`
- Use a preset: `wtui --preset battery_day`
- CSV export: `wtui --report net_daily --csv > net.csv`
- In the TUI, press `c` to write the current view to `./wtui-export.csv`; `h` switches to historical (SQLite) and `l` to live mode.

## Configuration

- Format: TOML, shared by daemon and viewer.
- Default path: `~/.config/wtui/config.toml` (override with `--config`).
- Presets define reusable queries for the viewer.
- Retention policy to prune older samples.

Example config:
```toml
[database]
path = "~/.local/share/wtui/data.db"
retention_days = 365

[daemon]
interval = "30s"
metrics = ["cpu", "ram", "net", "battery", "temps", "disk", "power"]
disk_devices = ["/", "/home"]
net_interfaces = ["eth0", "wlan0"]

[logging]
level = "info"
file = "~/.local/state/wtui/daemon.log"

[viewer]
default_range = "1h"

[presets]
battery_day = { kind = "chart", metrics = ["battery_capacity"], range = "1d" }
battery_year = { kind = "chart", metrics = ["battery_health"], range = "365d" }
cpu_gpu_hour = { kind = "report", metrics = ["cpu_temp", "gpu_temp"], range = "1h", csv = true }
net_week = { kind = "aggregate", metric = "net_bytes", group_by = "day", range = "7d", csv = true }
disk_year = { kind = "chart", metrics = ["disk_usage"], range = "365d" }
```

## Data model (SQLite)

- Tables per metric family (cpu, ram, net, battery, temps, disk, power) with UTC timestamp, value, source (iface/sensor/device), and units.
- Network: store raw counters and computed deltas; handle wrap/reset by discarding negative deltas and recording a reset event.
- Time: store timestamps in UTC; viewer may display in local time.
- Migrations: versioned schema; daemon migrates on startup when needed.

## Metric sources (Linux)

- Network: `/sys/class/net/<iface>/statistics/{rx_bytes,tx_bytes}` or `/proc/net/dev` (no packet sniffing).
- CPU/RAM: `/proc/stat`, `/proc/meminfo`.
- Disk usage: `statvfs`/`df`-style via libc on mounted filesystems.
- Temperatures: `/sys/class/hwmon/**/temp*_input`.
- Battery: `/sys/class/power_supply/*/` (`capacity`, `health`, `energy_now`, `energy_full`).
- Power draw: `/sys/class/powercap` or power_supply `current_now`/`voltage_now` when available.
- Permissions: intended for unprivileged users; no `CAP_NET_ADMIN` required.

## TUI experience

- Layout: header with range selector; left pane for presets; main pane for charts/reports; footer for status/hints.
- Navigation: arrow keys/hjkl to move focus; `Enter` apply preset; `/` filter presets; `q` quit; `c` toggle CSV export for current report.
- Modes: live (direct kernel counters) vs historical (SQLite); switch with `L`/`H` or flag.
- Charts: line/stacked for CPU/RAM/net; gauges for battery/power; tables for reports.

## Daemon lifecycle

- Suitable for a systemd (user) service; single-instance via PID file at `~/.local/state/wtui/wtui-daemon.pid`.
- On restart: detects counter resets; resumes writing to same DB.
- Config reload: `SIGHUP` to re-read config (interval/metrics). DB path changes require restart.

## Logging and errors

- Log levels: error, warn, info, debug.
- Default to stderr; optional log file via config.
- Missing metric sources log a warning and continue collecting available metrics.

## Development

- Run daemon: `cargo run -p wtui-daemon -- --config ./example.config.toml`
- Run viewer: `cargo run -p wtui -- --range 1h --charts cpu,ram`
- Lint/format: `cargo fmt && cargo clippy --all-targets --all-features`
- Tests: `cargo test --workspace`
- Mock data: include a small seed DB in `./fixtures` for TUI work without the daemon.

Nix development shell: `nix develop` (includes Rust toolchain, pkg-config, SQLite headers). The flake also exposes `packages.wtui` and `packages.wtui-daemon` for the two binaries.

## Technical implementation

For network-related stuff, wtui uses a similar way to fetch the network data than what vnstat does: vnStat works by periodically reading the kernel’s existing per-interface byte counters—found in /sys/class/net/<iface>/statistics/{rx_bytes,tx_bytes} or /proc/net/dev—and storing these cumulative values in a tiny SQLite (or older binary) database. It never sniffs packets; instead it computes traffic by subtracting successive counter snapshots, handling counter wrap/reset when values decrease. The daemon (vnstatd) polls at fixed intervals (usually hourly), and all reports are just queries over these stored deltas, with live mode reading counters directly instead of from the DB. The whole system is lightweight because it relies entirely on the kernel’s monotonically increasing stats rather than capturing any real traffic.
