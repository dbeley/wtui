use wtui_core::metrics::{cpu_usage_percent, CpuTimes};

#[test]
fn cpu_usage_calculates_delta() {
    let prev = CpuTimes {
        user: 10,
        nice: 0,
        system: 10,
        idle: 30,
        iowait: 0,
        irq: 0,
        softirq: 0,
        steal: 0,
    };
    let curr = CpuTimes {
        user: 20,
        nice: 0,
        system: 20,
        idle: 40,
        iowait: 0,
        irq: 0,
        softirq: 0,
        steal: 0,
    };
    let usage = cpu_usage_percent(&prev, &curr).unwrap();
    assert!((usage - 66.6).abs() < 1.0);
}
