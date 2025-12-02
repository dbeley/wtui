use tempfile::NamedTempFile;
use time::OffsetDateTime;
use wtui_core::metrics::NetSnapshot;
use wtui_core::Database;

#[test]
fn inserts_and_reads_cpu() {
    let tmp = NamedTempFile::new().unwrap();
    let db = Database::connect(tmp.path()).unwrap();
    let now = OffsetDateTime::now_utc();
    db.insert_cpu_usage(now, 42.0, Some("total")).unwrap();
    let rows = db.fetch_series("cpu_samples", None).unwrap();
    assert_eq!(rows.len(), 1);
    assert!((rows[0].value - 42.0).abs() < f64::EPSILON);
}

#[test]
fn aggregates_network_bytes() {
    let tmp = NamedTempFile::new().unwrap();
    let db = Database::connect(tmp.path()).unwrap();
    let now = OffsetDateTime::now_utc();
    let snap = NetSnapshot {
        rx_bytes: 1000,
        tx_bytes: 2000,
    };
    db.insert_net_sample(now, "eth0", snap, Some((100, 200)), false)
        .unwrap();
    let rows = db.aggregate_net(None, "day").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].value as i64, 300);
}
