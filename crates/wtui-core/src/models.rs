use time::{Duration, OffsetDateTime};

#[derive(Debug, Clone, PartialEq)]
pub struct MetricPoint {
    pub timestamp: OffsetDateTime,
    pub value: f64,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricSeries {
    pub name: String,
    pub unit: Option<String>,
    pub points: Vec<MetricPoint>,
}

impl MetricSeries {
    pub fn new<N: Into<String>>(name: N, unit: Option<&str>) -> Self {
        Self {
            name: name.into(),
            unit: unit.map(|u| u.to_string()),
            points: Vec::new(),
        }
    }

    pub fn push(&mut self, point: MetricPoint) {
        self.points.push(point);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeSpec {
    pub since: Option<OffsetDateTime>,
    pub until: OffsetDateTime,
}

impl RangeSpec {
    pub fn ending_now(duration: Duration) -> Self {
        let until = OffsetDateTime::now_utc();
        let since = until.checked_sub(duration);
        Self { since, until }
    }

    pub fn all_time() -> Self {
        Self {
            since: None,
            until: OffsetDateTime::now_utc(),
        }
    }
}
