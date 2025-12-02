use anyhow::{Context, Result};
use std::time::Duration as StdDuration;
use time::{Duration, OffsetDateTime};

pub fn now_utc() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

pub fn utc_from_timestamp(ts: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(ts).unwrap_or_else(|_| OffsetDateTime::now_utc())
}

pub fn parse_range(spec: &str) -> Result<Duration> {
    let std = humantime::parse_duration(spec).context("invalid duration format")?;
    Ok(duration_from_std(std))
}

pub fn duration_from_std(std: StdDuration) -> Duration {
    Duration::new(std.as_secs() as i64, std.subsec_nanos() as i32)
}

pub fn duration_to_std(duration: Duration) -> StdDuration {
    if duration.is_negative() {
        StdDuration::from_secs(0)
    } else {
        StdDuration::new(
            duration.whole_seconds() as u64,
            duration.subsec_nanoseconds() as u32,
        )
    }
}
