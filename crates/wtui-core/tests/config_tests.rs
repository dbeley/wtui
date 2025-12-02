use wtui_core::{parse_range, Config};

#[test]
fn defaults_expand_paths() {
    let cfg = Config::load(None).expect("load default config");
    assert!(
        !cfg.database.path.to_string_lossy().contains('~'),
        "database path should be expanded"
    );
}

#[test]
fn parse_range_supports_shortcuts() {
    let dur = parse_range("1h").expect("parse duration");
    assert_eq!(dur.whole_hours(), 1);
    let dur2 = parse_range("30s").expect("parse duration");
    assert_eq!(dur2.whole_seconds(), 30);
}
