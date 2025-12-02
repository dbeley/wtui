use crate::metrics::NetSnapshot;
use crate::timeutils::utc_from_timestamp;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaVersion {
    V1 = 1,
}

#[derive(Debug)]
pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn connect(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating directory {parent:?}"))?;
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        conn.pragma_update(None, "journal_mode", &"WAL")
            .context("enabling WAL mode")?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn install_v1(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS cpu_samples (
                timestamp INTEGER NOT NULL,
                usage REAL NOT NULL,
                source TEXT
            );

            CREATE TABLE IF NOT EXISTS ram_samples (
                timestamp INTEGER NOT NULL,
                used_bytes INTEGER NOT NULL,
                total_bytes INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS net_samples (
                timestamp INTEGER NOT NULL,
                interface TEXT NOT NULL,
                rx_bytes INTEGER NOT NULL,
                tx_bytes INTEGER NOT NULL,
                rx_delta INTEGER,
                tx_delta INTEGER,
                reset INTEGER DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS battery_samples (
                timestamp INTEGER NOT NULL,
                name TEXT NOT NULL,
                capacity REAL,
                health REAL,
                power_mw REAL
            );

            CREATE TABLE IF NOT EXISTS temp_samples (
                timestamp INTEGER NOT NULL,
                sensor TEXT NOT NULL,
                value REAL NOT NULL
            );

            CREATE TABLE IF NOT EXISTS disk_samples (
                timestamp INTEGER NOT NULL,
                mount TEXT NOT NULL,
                used_bytes INTEGER NOT NULL,
                total_bytes INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS power_samples (
                timestamp INTEGER NOT NULL,
                domain TEXT NOT NULL,
                draw_mw REAL NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_cpu_ts ON cpu_samples(timestamp);
            CREATE INDEX IF NOT EXISTS idx_ram_ts ON ram_samples(timestamp);
            CREATE INDEX IF NOT EXISTS idx_net_ts ON net_samples(timestamp);
            CREATE INDEX IF NOT EXISTS idx_battery_ts ON battery_samples(timestamp);
            CREATE INDEX IF NOT EXISTS idx_temp_ts ON temp_samples(timestamp);
            CREATE INDEX IF NOT EXISTS idx_disk_ts ON disk_samples(timestamp);
            CREATE INDEX IF NOT EXISTS idx_power_ts ON power_samples(timestamp);
            "#,
        )?;
        Ok(())
    }

    pub fn insert_cpu_usage(
        &self,
        timestamp: OffsetDateTime,
        usage: f64,
        source: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO cpu_samples(timestamp, usage, source) VALUES (?1, ?2, ?3)",
            params![timestamp.unix_timestamp(), usage, source],
        )?;
        Ok(())
    }

    pub fn insert_ram_usage(&self, timestamp: OffsetDateTime, used: u64, total: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO ram_samples(timestamp, used_bytes, total_bytes) VALUES (?1, ?2, ?3)",
            params![timestamp.unix_timestamp(), used as i64, total as i64],
        )?;
        Ok(())
    }

    pub fn insert_net_sample(
        &self,
        timestamp: OffsetDateTime,
        interface: &str,
        snapshot: NetSnapshot,
        delta: Option<(i64, i64)>,
        reset: bool,
    ) -> Result<()> {
        let (rx_delta, tx_delta) = delta.unwrap_or((0, 0));
        self.conn.execute(
            "INSERT INTO net_samples(timestamp, interface, rx_bytes, tx_bytes, rx_delta, tx_delta, reset) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![timestamp.unix_timestamp(), interface, snapshot.rx_bytes as i64, snapshot.tx_bytes as i64, rx_delta, tx_delta, reset as i32],
        )?;
        Ok(())
    }

    pub fn insert_battery_sample(
        &self,
        timestamp: OffsetDateTime,
        name: &str,
        capacity: Option<f64>,
        health: Option<f64>,
        power_mw: Option<f64>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO battery_samples(timestamp, name, capacity, health, power_mw) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![timestamp.unix_timestamp(), name, capacity, health, power_mw],
        )?;
        Ok(())
    }

    pub fn insert_temp_sample(
        &self,
        timestamp: OffsetDateTime,
        sensor: &str,
        value: f64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO temp_samples(timestamp, sensor, value) VALUES (?1, ?2, ?3)",
            params![timestamp.unix_timestamp(), sensor, value],
        )?;
        Ok(())
    }

    pub fn insert_disk_sample(
        &self,
        timestamp: OffsetDateTime,
        mount: &str,
        used: u64,
        total: u64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO disk_samples(timestamp, mount, used_bytes, total_bytes) VALUES (?1, ?2, ?3, ?4)",
            params![timestamp.unix_timestamp(), mount, used as i64, total as i64],
        )?;
        Ok(())
    }

    pub fn insert_power_sample(
        &self,
        timestamp: OffsetDateTime,
        domain: &str,
        draw_mw: f64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO power_samples(timestamp, domain, draw_mw) VALUES (?1, ?2, ?3)",
            params![timestamp.unix_timestamp(), domain, draw_mw],
        )?;
        Ok(())
    }

    pub fn prune_older_than(&self, cutoff: OffsetDateTime) -> Result<()> {
        let ts = cutoff.unix_timestamp();
        for table in [
            "cpu_samples",
            "ram_samples",
            "net_samples",
            "battery_samples",
            "temp_samples",
            "disk_samples",
            "power_samples",
        ] {
            self.conn.execute(
                &format!("DELETE FROM {table} WHERE timestamp < ?1"),
                params![ts],
            )?;
        }
        Ok(())
    }

    pub fn fetch_series(
        &self,
        table: &str,
        since: Option<OffsetDateTime>,
    ) -> Result<Vec<MetricRow>> {
        if let Some(range) = since {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT timestamp, value, label FROM {table}_view WHERE timestamp >= ?1 ORDER BY timestamp"
            ))?;
            let rows = stmt
                .query_map(params![range.unix_timestamp()], |row| {
                    Ok(MetricRow {
                        timestamp: utc_from_timestamp(row.get(0)?),
                        value: row.get(1)?,
                        label: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        } else {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT timestamp, value, label FROM {table}_view ORDER BY timestamp"
            ))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(MetricRow {
                        timestamp: utc_from_timestamp(row.get(0)?),
                        value: row.get(1)?,
                        label: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        }
    }

    pub fn latest_net_snapshots(&self) -> Result<HashMap<String, NetSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT interface, rx_bytes, tx_bytes FROM net_samples WHERE timestamp = (SELECT MAX(timestamp) FROM net_samples ns WHERE ns.interface = net_samples.interface)",
        )?;
        let map = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    NetSnapshot {
                        rx_bytes: row.get::<_, i64>(1)? as u64,
                        tx_bytes: row.get::<_, i64>(2)? as u64,
                    },
                ))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(map)
    }

    pub fn aggregate_net(
        &self,
        since: Option<OffsetDateTime>,
        group_by: &str,
    ) -> Result<Vec<MetricRow>> {
        let grouping = match group_by {
            "day" => "%Y-%m-%d",
            "hour" => "%Y-%m-%d %H:00",
            _ => "%Y-%m-%d",
        };
        let mut stmt = self.conn.prepare(
            "SELECT strftime(?1, datetime(timestamp, 'unixepoch')) AS bucket, SUM(rx_delta + tx_delta) as total
             FROM net_samples
             WHERE (?2 IS NULL OR timestamp >= ?2)
             GROUP BY bucket
             ORDER BY bucket",
        )?;
        let rows = stmt
            .query_map(
                params![grouping, since.map(|s| s.unix_timestamp())],
                |row| {
                    let bucket: String = row.get(0)?;
                    let value: f64 = row.get::<_, f64>(1)?;
                    let ts = parse_bucket(&bucket).unwrap_or_else(|| OffsetDateTime::now_utc());
                    Ok(MetricRow {
                        timestamp: ts,
                        value,
                        label: Some(bucket),
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[derive(Debug, Clone)]
pub struct MetricRow {
    pub timestamp: OffsetDateTime,
    pub value: f64,
    pub label: Option<String>,
}

/// SQLite views that normalize table schemas for the viewer.
/// These are defined in the initial migration so the viewer can query without
/// knowing the backing table details.
fn ensure_views(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE VIEW IF NOT EXISTS cpu_samples_view AS
        SELECT timestamp, usage AS value, source AS label FROM cpu_samples;

        CREATE VIEW IF NOT EXISTS ram_samples_view AS
        SELECT timestamp, (CAST(used_bytes AS REAL) / CAST(total_bytes AS REAL)) * 100.0 AS value, NULL AS label FROM ram_samples;

        CREATE VIEW IF NOT EXISTS net_samples_view AS
        SELECT timestamp, (rx_delta + tx_delta) AS value, interface AS label FROM net_samples;

        CREATE VIEW IF NOT EXISTS battery_samples_view AS
        SELECT timestamp, capacity AS value, name AS label FROM battery_samples;

        CREATE VIEW IF NOT EXISTS temp_samples_view AS
        SELECT timestamp, value, sensor AS label FROM temp_samples;

        CREATE VIEW IF NOT EXISTS disk_samples_view AS
        SELECT timestamp, (CAST(used_bytes AS REAL) / CAST(total_bytes AS REAL)) * 100.0 AS value, mount AS label FROM disk_samples;

        CREATE VIEW IF NOT EXISTS power_samples_view AS
        SELECT timestamp, draw_mw AS value, domain AS label FROM power_samples;
    "#,
    )?;
    Ok(())
}

fn parse_bucket(bucket: &str) -> Option<OffsetDateTime> {
    // Attempt to parse YYYY-MM-DD or YYYY-MM-DD HH:MM formats
    if let Ok(date) = time::Date::parse(
        bucket,
        &time::macros::format_description!("[year]-[month]-[day]"),
    ) {
        return date.with_hms(0, 0, 0).ok().map(|dt| dt.assume_utc());
    }
    if let Ok(dt) = OffsetDateTime::parse(
        bucket,
        &time::macros::format_description!("[year]-[month]-[day] [hour]:00"),
    ) {
        return Some(dt);
    }
    None
}

// create views after install
impl Database {
    fn install_views(&self) -> Result<()> {
        ensure_views(&self.conn)
    }
}

// ensure views after migration run
impl Database {
    fn migrate(&self) -> Result<()> {
        let version: i32 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version == 0 {
            self.install_v1()?;
            self.conn
                .pragma_update(None, "user_version", &(SchemaVersion::V1 as i32))?;
        }
        self.install_views()?;
        Ok(())
    }
}
