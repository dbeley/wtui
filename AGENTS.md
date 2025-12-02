# wtui agent notes

- Workspace layout: `crates/wtui-core` (shared types, config, db, metrics), `crates/wtui-daemon` (collector), `crates/wtui` (viewer TUI).
- Config defaults live under `~/.config/wtui/config.toml`; paths expand `~`. Daemon PID at `~/.local/state/wtui/wtui-daemon.pid`.
- SQLite schema uses per-metric tables plus `_view` helpers that normalize values for the viewer. Migration v1 is automatic on connect.
- Viewer metrics are mapped to views via `table_for_metric` in `crates/wtui/src/main.rs`.
- CSV export path from the TUI is `./wtui-export.csv`; CLI `--csv` writes to stdout.
- Retention pruning runs every 10 minutes when enabled.
