# wtui

A TUI monitoring tool made in rust that monitor computer metrics and allow querying them through graphs and reports. 

It's composed of two parts: a daemon that will monitor the data and store it into a SQLite database,
 and a TUI viewing utility that allow querying the data and displaying it through graphs and rows

## Daemon
command name: wtui-daemon

Metrics:
- temperature of all sensors
- battery capacity and health
- power consumption
- disk usage
- cpu usage
- ram usage
- network speed and data downloaded/uploaded

Configuration options include which metrics to monitor, interval of measurements, database location, etc.

## Viewer
command name: wtui

Two types of reports:
- charts (displayed as TUI elements)
- reports displaying data aggregated or raw (+ csv options)

Support for presets that can be defined through the configuration file (configuration file is acting on both the daemon and the viewing tool).

Examples of what a end-user would want to have as presets:
- graph of battery capacity over the last day
- graph of battery health over the last year
- report of the CPU and GPU temperature over the last hour (exportable in CSV)
- aggregated report of total data downloaded and uploaded for each day of the last week
- report of the evolution of disk usage over the last year
- etc.

## Installation

Can be installed with the included NixOS flake on NixOS.

Local development
```
nix develop
```

## Techincal implementation

For network-related stuff, wtui uses a similar way to fetch the network data than what vnstat does: vnStat works by periodically reading the kernel’s existing per-interface byte counters—found in /sys/class/net/<iface>/statistics/{rx_bytes,tx_bytes} or /proc/net/dev—and storing these cumulative values in a tiny SQLite (or older binary) database. It never sniffs packets; instead it computes traffic by subtracting successive counter snapshots, handling counter wrap/reset when values decrease. The daemon (vnstatd) polls at fixed intervals (usually hourly), and all reports are just queries over these stored deltas, with live mode reading counters directly instead of from the DB. The whole system is lightweight because it relies entirely on the kernel’s monotonically increasing stats rather than capturing any real traffic.
